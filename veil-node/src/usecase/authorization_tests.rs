use super::checkpoint_tests::{fill, gossip};
use super::two_phase_tests::{elect, round, send, view};
use super::*;
use crate::domain::{ApprovalPolicy, ApprovedPolicy, Member, PhaseBlock, PhaseMessage};

#[test]
fn unavailable_clock_rejects_approval_without_changing_durable_state_and_recovers() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Clock(AtomicBool);
    impl PolicyClock for Clock {
        fn boot_id(&self) -> &str {
            "approval-clock-test"
        }
        fn now_ns(&self) -> std::io::Result<u64> {
            Ok(0)
        }
        fn lease_time(&self) -> std::io::Result<LeaseTime> {
            if !self.0.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("Kernel clock error exceeds configured budget: maxerror_us=1024000 budget_ns=1000000000"));
            }
            Ok(LeaseTime {
                boot_ns: 0,
                unix_ns: SIMULATION_WALL_EPOCH_NS + 2_000_000_000,
                uncertainty_ns: 128_000_000,
                max_slow_rate_ppm: 1_000,
            })
        }
    }
    let (mut nodes, operator) = cluster();
    let clock = Arc::new(Clock(AtomicBool::new(false)));
    nodes[0].configure_policy_clock(clock.clone()).unwrap();
    let command = approved(&nodes[0], &operator, 1, false);
    let before = nodes[0].wal_bytes().unwrap();
    assert_eq!(
        nodes[0].accept_policy_request(command.clone()),
        Err("Clock bound required for operator approval")
    );
    assert!(nodes[0]
        .lease_clock_error
        .as_ref()
        .unwrap()
        .contains("maxerror_us=1024000"));
    assert_eq!(nodes[0].wal_bytes().unwrap(), before);
    assert_eq!(nodes[0].authorization_revision(), 0);
    clock.0.store(true, Ordering::SeqCst);
    let id = nodes[0].accept_policy_request(command).unwrap();
    assert!(nodes[0].lease_clock_error.is_none());
    assert!(nodes[0]
        .ban_request_status(&id)
        .contains("request_phase=queued"));
    // Receipt of a fresh approval still does not mean commit or application.
    assert_eq!(nodes[0].commit_index, 0);
    assert_eq!(nodes[0].last_applied, 0);
}

fn cluster() -> (Vec<ConsensusState>, NodeCryptoIdentity) {
    let op = NodeCryptoIdentity::generate_random("operator");
    let mut n = super::two_phase_tests::nodes();
    let members: Vec<_> = n
        .iter()
        .enumerate()
        .map(|(i, s)| Member {
            node_id: s.node_id.clone(),
            address: format!("127.0.0.{}:5700", i + 1).parse().unwrap(),
            public_key: s.identity.public_key_bytes.clone().try_into().unwrap(),
        })
        .collect();
    let config = FixedConfiguration::new("approved-test", 1, members, vec![], 1000)
        .unwrap()
        .with_approval(ApprovalPolicy {
            public_key: op.public_key_bytes.clone().try_into().unwrap(),
            scopes: vec!["192.0.2.10/tcp/443".parse().unwrap()],
        })
        .unwrap();
    for old in &mut n {
        let mut s = ConsensusState::new(&old.node_id, vec![]);
        s.configure_fixed_membership(config.clone(), old.identity.clone())
            .unwrap();
        s.enable_two_phase().unwrap();
        *old = s;
    }
    (n, op)
}
fn approved(
    s: &ConsensusState,
    op: &NodeCryptoIdentity,
    revision: u64,
    trusted: bool,
) -> MeshCommand {
    let issued = SIMULATION_WALL_EPOCH_NS + s.monotonic_now_ns;
    let mut a = ApprovedPolicy {
        revision,
        issued_at_unix_ns: issued,
        valid_until_unix_ns: issued + 60_000_000_000,
        rule: DeadlinePeerTrustRule {
            rule: PeerTrustRule::new(
                Ipv4Addr::new(192, 0, 2, 77),
                trusted,
                if trusted { 0 } else { 60 },
            ),
            not_after_unix_ns: if trusted { 0 } else { issued + 60_000_000_000 },
        },
        signature: vec![],
    };
    a.signature = op.sign_progress_payload(&a.payload(&s.signing_context));
    MeshCommand::AuthorizedPolicy(Box::new(a))
}
fn restore(s: &ConsensusState, path: &std::path::Path) -> ConsensusState {
    let mut n = ConsensusState::new(&s.node_id, vec![]);
    n.configure_fixed_membership(s.fixed_configuration.clone().unwrap(), s.identity.clone())
        .unwrap();
    n.enable_two_phase().unwrap();
    n.load_wal(path).unwrap();
    n
}
#[test]
fn compromised_node_cannot_authorize_policy_or_expand_context() {
    let (mut n, op) = cluster();
    elect(&mut n, 0, &[1, 2, 3]);
    let good = approved(&n[0], &op, 1, false);
    let mut altered = good.clone();
    if let MeshCommand::AuthorizedPolicy(a) = &mut altered {
        a.rule.rule.peer_ip ^= 1;
    }
    let forged = approved(&n[0], &n[0].identity, 1, false);
    let raw = MeshCommand::set_trust_with_ttl(Ipv4Addr::new(192, 0, 2, 77), false, 60);
    for command in [
        raw,
        forged,
        altered,
        MeshCommand::set_lockdown("*", true),
        MeshCommand::set_trust(Ipv4Addr::new(192, 0, 2, 77), true),
    ] {
        let block = PhaseBlock {
            entry: LogEntry {
                index: 1,
                term: 1,
                command,
            },
            parent: [0; 32],
        };
        let rpc = ConsensusRpc::TwoPhase {
            term: 1,
            sender_id: "a".into(),
            message: Box::new(PhaseMessage::Proposal {
                view: view(&n, 0),
                block,
            }),
        };
        let signed = n[0].sign_rpc(rpc);
        let before = n[1].two_phase.as_ref().unwrap().durable.votes.clone();
        assert!(n[1].handle_signed_rpc(signed).is_err());
        assert_eq!(n[1].two_phase.as_ref().unwrap().durable.votes, before);
        assert_eq!(n[1].commit_index, 0);
        assert!(n[1].log.is_empty());
    }
    let context = n[1].signing_context.clone();
    n[1].signing_context.config_digest[0] ^= 1;
    assert!(n[1].verify_operator_command(&good).is_err());
    n[1].signing_context = context;
    n[0].propose(good).unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2, 3]);
    }
    assert!(n.iter().all(|s| s.authorization_revision() == 1));
}
#[test]
fn unban_revision_survives_checkpoint_transfer_restart_and_old_ban_replay() {
    let (mut n, op) = cluster();
    elect(&mut n, 0, &[1, 2, 3]);
    let ban = approved(&n[0], &op, 1, false);
    n[0].propose(ban.clone()).unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2]);
    }
    assert_eq!(
        n[1].check_xdp_packet(Ipv4Addr::new(192, 0, 2, 77)),
        XdpVerdict::Drop
    );
    let unban = approved(&n[1], &op, 2, true);
    n[0].propose(unban).unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2]);
    }
    assert_eq!(
        n[1].check_xdp_packet(Ipv4Addr::new(192, 0, 2, 77)),
        XdpVerdict::Pass
    );
    fill(&mut n, 0, &[1, 2], 30);
    gossip(&mut n, &[0, 1, 2]);
    let snapshot = n[0]
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .stable
        .clone()
        .unwrap();
    assert_eq!(snapshot.state.authorization_revision, 2);
    n[3].install_snapshot(snapshot).unwrap();
    let path = super::regression_tests::fixture_dir().join("approved.wal");
    n[3].save_wal(&path).unwrap();
    n[3] = restore(&n[3], &path);
    assert_eq!(n[3].authorization_revision(), 2);
    assert!(n[3].accept_policy_request(ban.clone()).is_err());
    assert!(n[0].propose(ban).is_err());
    for s in &n {
        assert_eq!(
            s.check_xdp_packet(Ipv4Addr::new(192, 0, 2, 77)),
            XdpVerdict::Pass
        );
    }
}
#[test]
fn expired_approval_cannot_start_but_prepared_value_can_finish_without_renewal() {
    let (mut n, op) = cluster();
    elect(&mut n, 0, &[1, 2, 3]);
    let ban = approved(&n[0], &op, 1, false);
    n[0].propose(ban.clone()).unwrap();
    // Form prepare QC but withhold it from followers until the admission/lease window closes.
    for i in [1, 2] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let reply = send(&mut n, 0, i, rpc).unwrap();
        send(&mut n, i, 0, reply);
    }
    for s in &mut n {
        s.advance_monotonic_clock(120);
    }
    assert!(n[3].accept_policy_request(ban).is_err());
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2, 3]);
    }
    for s in &n[..3] {
        assert_eq!(s.commit_index, 1);
        assert_eq!(
            s.check_xdp_packet(Ipv4Addr::new(192, 0, 2, 77)),
            XdpVerdict::Pass
        );
    }
    let unban = approved(&n[1], &op, 2, true);
    n[0].propose(unban).unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2, 3]);
    }
    assert_eq!(n[1].authorization_revision(), 2);
}
#[test]
fn configuration_binds_scope_and_independent_operator_key() {
    let (n, op) = cluster();
    let a = approved(&n[0], &op, 1, false);
    let mut s = ConsensusState::new("a", vec![]);
    let base = FixedConfiguration::new(
        "approved-test",
        1,
        n[0].fixed_configuration
            .as_ref()
            .unwrap()
            .members()
            .to_vec(),
        vec![],
        1000,
    )
    .unwrap();
    let same = base
        .clone()
        .with_approval(ApprovalPolicy {
            public_key: op.public_key_bytes.clone().try_into().unwrap(),
            scopes: vec!["192.0.2.10/tcp/443".parse().unwrap()],
        })
        .unwrap();
    assert_eq!(
        same.context(),
        n[0].fixed_configuration.as_ref().unwrap().context()
    );
    let c = base
        .clone()
        .with_approval(ApprovalPolicy {
            public_key: op.public_key_bytes.clone().try_into().unwrap(),
            scopes: vec!["192.0.2.10/tcp/444".parse().unwrap()],
        })
        .unwrap();
    s.configure_fixed_membership(c, n[0].identity.clone())
        .unwrap();
    s.enable_two_phase().unwrap();
    assert!(s.verify_operator_command(&a).is_err());
    let bad = base.with_approval(ApprovalPolicy {
        public_key: n[0].identity.public_key_bytes.clone().try_into().unwrap(),
        scopes: vec!["192.0.2.10/tcp/443".parse().unwrap()],
    });
    assert!(bad.is_err());
    for scope in [
        "0.0.0.0/tcp/443",
        "192.0.2.10/tcp/0",
        "192.0.2.10/icmp/0",
        "::1/tcp/443",
        "192.0.2.0/24/tcp/443",
    ] {
        assert!(scope.parse::<crate::domain::ServiceScope>().is_err());
    }
}

#[test]
fn expired_bare_approval_abdicates_without_rewriting_votes_and_new_view_resumes() {
    let (mut n, op) = cluster();
    elect(&mut n, 0, &[1, 2, 3]);
    let ban = approved(&n[0], &op, 1, false);
    n[0].propose(ban).unwrap();
    // Only a and b prepare: no quorum certificate can be completed.
    let rpc = n[0].make_append_entries(&n[1].node_id).unwrap();
    let reply = send(&mut n, 0, 1, rpc).unwrap();
    send(&mut n, 1, 0, reply);
    let votes = n[0].two_phase.as_ref().unwrap().durable.votes.clone();
    for s in &mut n {
        s.advance_monotonic_clock(120);
    }
    n[0].drive_two_phase().unwrap();
    assert_eq!(n[0].role, Role::Follower);
    assert_eq!(n[0].two_phase.as_ref().unwrap().durable.votes, votes);
    elect(&mut n, 2, &[1, 3]);
    n[2].drive_two_phase().unwrap();
    for _ in 0..4 {
        round(&mut n, 2, &[0, 1, 3]);
    }
    assert!(n.iter().all(|s| s.authorization_revision() == 0));
    let new = approved(&n[2], &op, 1, false);
    n[2].propose(new).unwrap();
    for _ in 0..4 {
        round(&mut n, 2, &[0, 1, 3]);
    }
    assert!(n.iter().all(|s| s.authorization_revision() == 1));
}

#[test]
fn hidden_prepare_in_new_view_overrides_expired_bare_local_approval() {
    let (mut n, op) = cluster();
    elect(&mut n, 0, &[1, 2, 3]);
    let ban = approved(&n[0], &op, 1, false);
    n[0].propose(ban).unwrap();
    for i in [1, 2] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let reply = send(&mut n, 0, i, rpc).unwrap();
        send(&mut n, i, 0, reply);
    }
    assert!(n[0].two_phase.as_ref().unwrap().durable.prepared.is_some());
    assert!(n[1].two_phase.as_ref().unwrap().durable.prepared.is_none());
    for s in &mut n {
        s.advance_monotonic_clock(120);
    }
    // Restart the former leader, retaining its durable prepare proof.
    let path = super::regression_tests::fixture_dir().join("hidden-approval.wal");
    n[0].save_wal(&path).unwrap();
    n[0] = restore(&n[0], &path);
    n[0].advance_monotonic_clock(120);
    // b has only a bare local vote, but a's new-view report proves preparation.
    elect(&mut n, 1, &[0, 2, 3]);
    n[1].drive_two_phase().unwrap();
    assert_eq!(n[1].role, Role::Leader);
    for _ in 0..5 {
        round(&mut n, 1, &[0, 2, 3]);
    }
    assert!(n.iter().all(|s| s.authorization_revision() == 1));
    for s in &n {
        assert_eq!(
            s.check_xdp_packet(Ipv4Addr::new(192, 0, 2, 77)),
            XdpVerdict::Pass
        );
    }
}

#[test]
fn approved_ban_and_unban_release_no_vote_on_wal_failure() {
    struct Fail;
    impl ConsensusPersistence for Fail {
        fn persist(&self, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("approval WAL failure"))
        }
    }
    for unban in [false, true] {
        let (mut n, op) = cluster();
        elect(&mut n, 0, &[1, 2, 3]);
        if unban {
            let ban = approved(&n[0], &op, 1, false);
            n[0].propose(ban).unwrap();
            for _ in 0..4 {
                round(&mut n, 0, &[1, 2, 3]);
            }
        }
        let revision = n[1].authorization_revision();
        let command = approved(&n[0], &op, revision + 1, unban);
        n[0].propose(command).unwrap();
        let path = super::regression_tests::fixture_dir().join("approval-write-failure.wal");
        n[1].enable_persistence(Arc::new(
            crate::infra::wal::FileWal::create_new(&path).unwrap(),
        ))
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        n[1].persistence = Some(Arc::new(Fail));
        let rpc = n[0].make_append_entries(&n[1].node_id).unwrap();
        let signed = n[0].sign_rpc_durable(rpc).unwrap();
        assert!(n[1].handle_signed_rpc(signed).is_err());
        assert!(n[1].persistence_error.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let restored = restore(&n[1], &path);
        assert_eq!(restored.authorization_revision(), revision);
        assert_eq!(
            restored.check_xdp_packet(Ipv4Addr::new(192, 0, 2, 77)),
            if unban {
                XdpVerdict::Drop
            } else {
                XdpVerdict::Pass
            }
        );
    }
}

#[test]
fn individual_unban_preserves_other_ip_bans_across_wal_restart() {
    let (mut n, op) = cluster();
    elect(&mut n, 0, &[1, 2, 3]);
    let first = Ipv4Addr::new(192, 0, 2, 77);
    let second = Ipv4Addr::new(192, 0, 2, 78);
    for (revision, ip, trusted) in [(1, first, false), (2, second, false), (3, first, true)] {
        let mut command = approved(&n[0], &op, revision, trusted);
        if let MeshCommand::AuthorizedPolicy(a) = &mut command {
            a.rule.rule.peer_ip = u32::from(ip);
            a.signature = op.sign_progress_payload(&a.payload(&n[0].signing_context));
        }
        n[0].propose(command).unwrap();
        for _ in 0..4 {
            round(&mut n, 0, &[1, 2, 3]);
        }
    }
    for s in &n {
        assert_eq!(s.check_xdp_packet(first), XdpVerdict::Pass);
        assert_eq!(s.check_xdp_packet(second), XdpVerdict::Drop);
    }
    let path = super::regression_tests::fixture_dir().join("individual-unban.wal");
    n[1].save_wal(&path).unwrap();
    let recovered = restore(&n[1], &path);
    assert_eq!(recovered.authorization_revision(), 3);
    assert_eq!(recovered.check_xdp_packet(first), XdpVerdict::Pass);
    assert_eq!(recovered.check_xdp_packet(second), XdpVerdict::Drop);
}
