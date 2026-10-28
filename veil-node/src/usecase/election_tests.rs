use super::regression_tests::{cluster, elect, fixture_dir};
use super::*;
use crate::domain::{term_support_payload, TermCertificate};
use crate::infra::wal::FileWal;

fn heartbeat(term: u64, leader: &str) -> ConsensusRpc {
    ConsensusRpc::AppendEntries {
        term,
        leader_id: leader.into(),
        prev_log_index: 0,
        prev_log_term: 0,
        prev_log_hash: [0; 32],
        entries: vec![],
        leader_commit: 0,
        commit_qc: None,
    }
}

fn support_round(nodes: &mut [ConsensusState], candidate: usize) -> SignedConsensusRpc {
    let pre = nodes[candidate].start_pre_vote().unwrap();
    for i in 0..nodes.len() {
        if i == candidate {
            continue;
        }
        if let Some(reply) = nodes[i].handle_signed_rpc(pre.clone()).unwrap() {
            let signed = nodes[i].sign_rpc(reply);
            nodes[candidate].handle_signed_rpc(signed).unwrap();
        }
    }
    nodes[candidate].finish_pre_vote().unwrap().unwrap()
}

#[test]
fn one_member_high_terms_never_mutate_wal_vote_role_or_timer() {
    let mut nodes = cluster();
    elect(&mut nodes);
    let path = fixture_dir().join("victim.wal");
    nodes[1]
        .enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
        .unwrap();
    let before = nodes[1].wal_bytes().unwrap();
    let activity = nodes[1].election_activity();
    for term in [2, 100, MAX_SAFE_TERM - 2, MAX_SAFE_TERM - 1] {
        let requests = [
            ConsensusRpc::RequestVote {
                term,
                candidate_id: "a".into(),
                last_log_index: 0,
                last_log_term: 0,
            },
            ConsensusRpc::RequestVoteResponse {
                term,
                voter_id: "a".into(),
                vote_granted: false,
            },
            heartbeat(term, "a"),
            ConsensusRpc::AppendEntriesResponse {
                term,
                follower_id: "a".into(),
                success: false,
                match_index: 0,
                entry_hash: [0; 32],
                qc_vote_signature: vec![],
            },
            ConsensusRpc::TimeoutVote {
                term,
                voter_id: "a".into(),
                highest_qc: None,
                tc_vote_signature: nodes[0].identity.sign_timeout_vote(
                    nodes[0].signing_context(),
                    term,
                    0,
                    0,
                ),
            },
        ];
        for rpc in requests {
            let signed = nodes[0].sign_rpc(rpc);
            assert!(nodes[1].handle_signed_rpc(signed).is_err());
        }
        let pre = nodes[0].sign_rpc(ConsensusRpc::PreVote {
            term,
            candidate_id: "a".into(),
            last_log_index: 0,
            last_log_term: 0,
        });
        nodes[1].handle_signed_rpc(pre).unwrap();
        assert_eq!(nodes[1].wal_bytes().unwrap(), before);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(nodes[1].election_activity(), activity);
        assert_eq!(nodes[1].current_term, 1);
        assert_eq!(nodes[1].voted_for.as_deref(), Some("a"));
    }
    // The active leader also ignores a single peer's inflated response.
    let before = nodes[0].wal_bytes().unwrap();
    let rpc = nodes[1].sign_rpc(ConsensusRpc::RequestVoteResponse {
        term: 900,
        voter_id: "b".into(),
        vote_granted: false,
    });
    assert!(nodes[0].handle_signed_rpc(rpc).is_err());
    assert_eq!(nodes[0].role, Role::Leader);
    assert_eq!(nodes[0].wal_bytes().unwrap(), before);
}

#[test]
fn pre_vote_needs_distinct_context_bound_quorum_and_minority_does_not_inflate_terms() {
    let mut nodes = cluster();
    elect(&mut nodes);
    let before = nodes[1].wal_bytes().unwrap();
    for _ in 0..8 {
        let pre = nodes[1].start_pre_vote().unwrap();
        let reply = nodes[2].handle_signed_rpc(pre).unwrap().unwrap();
        let signed = nodes[2].sign_rpc(reply);
        for _ in 0..5 {
            nodes[1].handle_signed_rpc(signed.clone()).unwrap();
        }
        assert!(nodes[1].finish_pre_vote().unwrap().is_none());
        assert_eq!(nodes[1].wal_bytes().unwrap(), before);
    }
    let term = MAX_SAFE_TERM - 2;
    let rpc = heartbeat(term, "a");
    let signature = nodes[0]
        .identity
        .sign_term_support(nodes[0].signing_context(), term);
    let mut votes = BTreeMap::from([
        ("a".into(), signature.clone()),
        ("b".into(), signature.clone()),
        ("c".into(), signature),
    ]);
    let wrap = |votes| ConsensusRpc::TermCertified {
        term,
        sender_id: "a".into(),
        certificate: TermCertificate::PreVote { term, votes },
        rpc_bytes: bincode::serialize(&rpc).unwrap(),
    };
    let signed = nodes[0].sign_rpc(wrap(votes.clone()));
    assert!(nodes[1].handle_signed_rpc(signed).is_err());
    let foreign = SigningContext::new("other", 1).unwrap();
    for n in &nodes[..3] {
        votes.insert(
            n.node_id.clone(),
            n.identity.sign_term_support(&foreign, term),
        );
    }
    let signed = nodes[0].sign_rpc(wrap(votes));
    assert!(nodes[1].handle_signed_rpc(signed).is_err());
    assert_eq!(nodes[1].wal_bytes().unwrap(), before);
    assert_ne!(
        term_support_payload(nodes[0].signing_context(), 2),
        term_support_payload(nodes[0].signing_context(), 3)
    );
}

#[test]
fn replayed_heartbeat_vote_and_ack_cannot_renew_election_timeout() {
    let mut nodes = cluster();
    let vote = nodes[0].start_signed_election().unwrap();
    nodes[1].handle_signed_rpc(vote.clone()).unwrap();
    let first = nodes[1].election_activity();
    for _ in 0..20 {
        nodes[1].handle_signed_rpc(vote.clone()).unwrap();
    }
    assert_eq!(nodes[1].election_activity(), first);
    // Finish the actual election, then accept a quiet (unchanged) heartbeat.
    for i in 1..4 {
        let reply = nodes[i].handle_signed_rpc(vote.clone()).unwrap().unwrap();
        let reply = nodes[i].sign_rpc(reply);
        nodes[0].handle_signed_rpc(reply).unwrap();
    }
    let hb = nodes[0].sign_rpc(heartbeat(1, "a"));
    for _ in 0..30 {
        nodes[1].handle_signed_rpc(hb.clone()).unwrap();
    }
    assert_eq!(nodes[1].election_activity(), first);
    let (_, probe) = nodes[1].make_liveness_probe().unwrap().unwrap();
    let ack = nodes[0].handle_signed_rpc(probe).unwrap().unwrap();
    let ack = nodes[0].sign_rpc(ack);
    nodes[1].handle_signed_rpc(ack.clone()).unwrap();
    assert_eq!(nodes[1].election_activity(), first + 1);
    for _ in 0..30 {
        nodes[1].handle_signed_rpc(ack.clone()).unwrap();
    }
    assert_eq!(nodes[1].election_activity(), first + 1);
    let (_, fresh) = nodes[1].make_liveness_probe().unwrap().unwrap();
    nodes[1].handle_signed_rpc(ack).unwrap();
    assert_eq!(nodes[1].election_activity(), first + 1);
    let fresh_ack = nodes[0].handle_signed_rpc(fresh).unwrap().unwrap();
    let fresh_ack = nodes[0].sign_rpc(fresh_ack);
    // An otherwise valid ack is useless once its receiver's challenge expires.
    nodes[1].election.probe.as_mut().unwrap().3 =
        std::time::Instant::now() - std::time::Duration::from_secs(1);
    nodes[1].handle_signed_rpc(fresh_ack).unwrap();
    assert_eq!(nodes[1].election_activity(), first + 1);
    // A recent fresh leader contact blocks next-term pre-vote support.
    let pre = nodes[2].start_pre_vote().unwrap();
    assert!(nodes[1].handle_signed_rpc(pre).unwrap().is_none());
}

#[test]
fn quorum_term_catchup_and_restart_preserve_votes_without_persisting_ephemeral_probes() {
    let mut nodes = cluster();
    elect(&mut nodes);
    // a stops. b,c,d can authorize term 2 and elect b; a misses both rounds.
    let pre = nodes[1].start_pre_vote().unwrap();
    for i in [2, 3] {
        let reply = nodes[i].handle_signed_rpc(pre.clone()).unwrap().unwrap();
        let reply = nodes[i].sign_rpc(reply);
        nodes[1].handle_signed_rpc(reply).unwrap();
    }
    let vote = nodes[1].finish_pre_vote().unwrap().unwrap();
    for i in [2, 3] {
        let reply = nodes[i].handle_signed_rpc(vote.clone()).unwrap().unwrap();
        let reply = nodes[i].sign_rpc(reply);
        nodes[1].handle_signed_rpc(reply).unwrap();
    }
    assert_eq!(nodes[1].role, Role::Leader);
    let hb = nodes[1].sign_rpc(heartbeat(2, "b"));
    nodes[0].handle_signed_rpc(hb).unwrap();
    assert_eq!(nodes[0].current_term, 2);
    // Reload every WAL; term evidence survives, but challenges do not.
    let dir = fixture_dir();
    for n in &mut nodes {
        let path = dir.join(format!("{}.wal", n.node_id));
        n.save_wal(&path).unwrap();
        let vote = n.voted_for.clone();
        n.load_wal(&path).unwrap();
        assert_eq!(n.voted_for, vote);
        assert!(n.election.probe.is_none());
        assert_eq!(n.election.certificate.as_ref().unwrap().term(), 2);
    }
    let vote = support_round(&mut nodes, 0);
    assert_eq!(nodes[0].current_term, 3);
    for i in 1..4 {
        let reply = nodes[i].handle_signed_rpc(vote.clone()).unwrap().unwrap();
        let reply = nodes[i].sign_rpc(reply);
        nodes[0].handle_signed_rpc(reply).unwrap();
    }
    assert_eq!(nodes[0].role, Role::Leader);
    nodes[0].propose(MeshCommand::NoOp).unwrap();
    for _ in 0..2 {
        for i in 1..4 {
            let rpc = nodes[0].make_append_entries(&nodes[i].node_id).unwrap();
            let signed = nodes[0].sign_rpc(rpc);
            assert!(
                bincode::serialized_size(&crate::adapter::WireMessage::Consensus(Box::new(
                    signed.clone()
                )))
                .unwrap()
                    <= MAX_WIRE_MESSAGE_BYTES as u64
            );
            let reply = nodes[i].handle_signed_rpc(signed).unwrap().unwrap();
            let reply = nodes[i].sign_rpc(reply);
            nodes[0].handle_signed_rpc(reply).unwrap();
        }
    }
    assert!(nodes.iter().all(|n| n.commit_index == 1));
}

#[test]
fn certified_message_nesting_and_inner_identity_substitution_are_rejected() {
    let mut nodes = cluster();
    let votes = nodes[..3]
        .iter()
        .map(|n| {
            (
                n.node_id.clone(),
                n.identity.sign_term_support(n.signing_context(), 2),
            )
        })
        .collect();
    let cert = TermCertificate::PreVote { term: 2, votes };
    let wrap = |rpc: ConsensusRpc| ConsensusRpc::TermCertified {
        term: 2,
        sender_id: "a".into(),
        certificate: cert.clone(),
        rpc_bytes: bincode::serialize(&rpc).unwrap(),
    };
    for rpc in [wrap(heartbeat(2, "b")), wrap(wrap(heartbeat(2, "a")))] {
        let signed = nodes[0].sign_rpc(rpc);
        assert!(nodes[1].handle_signed_rpc(signed).is_err());
        assert_eq!(nodes[1].current_term, 0);
    }
}

#[test]
fn old_certified_heartbeat_cannot_erase_prepared_next_term() {
    let mut nodes = cluster();
    let vote = support_round(&mut nodes, 0);
    for i in 1..4 {
        let reply = nodes[i].handle_signed_rpc(vote.clone()).unwrap().unwrap();
        let signed = nodes[i].sign_rpc(reply);
        nodes[0].handle_signed_rpc(signed).unwrap();
    }
    let hb = nodes[0].sign_rpc(heartbeat(1, "a"));
    let pre = nodes[1].start_pre_vote().unwrap();
    for i in [2, 3] {
        let reply = nodes[i].handle_signed_rpc(pre.clone()).unwrap().unwrap();
        let signed = nodes[i].sign_rpc(reply);
        nodes[1].handle_signed_rpc(signed).unwrap();
    }
    for _ in 0..100 {
        nodes[1].handle_signed_rpc(hb.clone()).unwrap();
    }
    assert!(nodes[1].finish_pre_vote().unwrap().is_some());
    assert_eq!(nodes[1].current_term, 2);
}

#[test]
fn wal_v4_upgrade_preserves_vote_and_current_evidence_recovers_a_lagging_peer() {
    use ring::digest;
    let mut nodes = cluster();
    elect(&mut nodes);
    let dir = fixture_dir();
    let new_bytes = nodes[1].wal_bytes().unwrap();
    let envelope: WalEnvelope = bincode::deserialize(&new_bytes).unwrap();
    let (mut payload, _): (WalPayload, Option<TermCertificate>) =
        bincode::deserialize(&envelope.payload_bytes).unwrap();
    payload.version = 4;
    let payload_bytes = bincode::serialize(&payload).unwrap();
    let legacy = bincode::serialize(&WalEnvelope {
        sha256_checksum: digest::digest(&digest::SHA256, &payload_bytes)
            .as_ref()
            .try_into()
            .unwrap(),
        payload_bytes,
    })
    .unwrap();
    let path = dir.join("legacy-v4.wal");
    // Retain a real v4 encoding, with normal WAL permissions.
    nodes[1].save_wal(&path).unwrap();
    std::fs::write(&path, &legacy).unwrap();
    nodes[1].load_wal(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), legacy);
    assert_eq!(nodes[1].current_term, 1);
    assert_eq!(nodes[1].voted_for.as_deref(), Some("a"));
    let second = nodes[2].sign_rpc(ConsensusRpc::RequestVote {
        term: 1,
        candidate_id: "c".into(),
        last_log_index: 0,
        last_log_term: 0,
    });
    assert!(matches!(
        nodes[1].handle_signed_rpc(second).unwrap(),
        Some(ConsensusRpc::RequestVoteResponse {
            vote_granted: false,
            ..
        })
    ));
    // Construct valid admission evidence with no log votes. Keep c behind.
    let term = 4;
    let votes = nodes[..3]
        .iter()
        .map(|n| {
            (
                n.node_id.clone(),
                n.identity.sign_term_support(n.signing_context(), term),
            )
        })
        .collect();
    let inner = ConsensusRpc::RequestVote {
        term,
        candidate_id: "a".into(),
        last_log_index: 0,
        last_log_term: 0,
    };
    let certified = nodes[0].sign_rpc(ConsensusRpc::TermCertified {
        term,
        sender_id: "a".into(),
        certificate: TermCertificate::PreVote { term, votes },
        rpc_bytes: bincode::serialize(&inner).unwrap(),
    });
    nodes[1].handle_signed_rpc(certified).unwrap();
    let path = dir.join("certified-current.wal");
    nodes[1].save_wal(&path).unwrap();
    nodes[1].load_wal(&path).unwrap();
    assert_eq!(nodes[1].election.certificate.as_ref().unwrap().term(), term);
    let pre = nodes[2].start_pre_vote().unwrap();
    let reply = nodes[1].handle_signed_rpc(pre).unwrap().unwrap();
    let signed = nodes[1].sign_rpc(reply);
    nodes[2].handle_signed_rpc(signed).unwrap();
    assert_eq!(nodes[2].current_term, 4);
    assert!(nodes[2].finish_pre_vote().unwrap().is_none());
    assert_eq!(nodes[1].voted_for.as_deref(), Some("a"));
    // A checksum-valid corruption of the embedded evidence must still fail.
    let raw = nodes[1].wal_bytes().unwrap();
    let mut env: WalEnvelope = bincode::deserialize(&raw).unwrap();
    let (payload, Some(TermCertificate::PreVote { term, mut votes }), progress) =
        bincode::deserialize::<(
            WalPayload,
            Option<TermCertificate>,
            progress::ProgressDurable,
        )>(&env.payload_bytes)
        .unwrap()
    else {
        panic!("missing cert")
    };
    votes.get_mut("a").unwrap()[0] ^= 1;
    env.payload_bytes = bincode::serialize(&(
        payload,
        Some(TermCertificate::PreVote { term, votes }),
        progress,
    ))
    .unwrap();
    env.sha256_checksum = digest::digest(&digest::SHA256, &env.payload_bytes)
        .as_ref()
        .try_into()
        .unwrap();
    let bad_path = dir.join("bad-evidence.wal");
    nodes[1].save_wal(&bad_path).unwrap();
    std::fs::write(&bad_path, bincode::serialize(&env).unwrap()).unwrap();
    let error = nodes[1].load_wal(&bad_path).unwrap_err();
    assert!(
        error.to_string().contains("Invalid term support signature"),
        "{error}"
    );
}

#[test]
fn arena_manual_reelection_uses_the_same_term_admission_rules() {
    let mut app = crate::tui::TuiArenaApp::new();
    assert_eq!(app.nodes["node-alpha"].role, Role::Leader);
    app.elect_leader("node-beta");
    assert_eq!(app.nodes["node-beta"].role, Role::Leader);
    assert_eq!(app.nodes["node-beta"].current_term, 2);
    assert!(app.nodes.values().all(|n| n.current_term == 2));
}
