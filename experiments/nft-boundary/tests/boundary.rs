//! Deterministic fault injection against a separate in-memory reference kernel.
//! None of these tests contacts Linux, nftables, a host or a cloud service.
use nft_boundary_offline::*;
use std::{collections::BTreeMap, net::Ipv4Addr};

const SECOND: u64 = 1_000_000_000;
const CONTEXT: ObservedContext = ObservedContext {
    uid: 1001,
    namespace: 12,
    owner_socket: 42,
};
fn ip(n: u8) -> Ipv4Addr {
    Ipv4Addr::new(198, 18, 0, n)
}
fn scope() -> Scope {
    Scope::new(
        Ipv4Addr::new(10, 99, 0, 66),
        18443,
        CONTEXT,
        &[Ipv4Addr::new(198, 51, 100, 7)],
    )
    .unwrap()
}
fn broker() -> ModelBroker {
    ModelBroker::start(scope(), 1, true).unwrap()
}
fn preview(
    b: &ModelBroker,
    operation: Operation,
    ip: Ipv4Addr,
    ttl: u32,
    now: u64,
) -> [u8; FRAME_BYTES] {
    b.preview(CONTEXT, operation, ip, ttl, now).unwrap()
}

/// Absolute-BOOTTIME reference behavior, deliberately NOT an nft adapter.
/// Other rules and the base firewall are separate so a test can detect damage.
struct ReferenceKernel {
    owner: Option<ObservedContext>,
    bans: BTreeMap<Ipv4Addr, u64>,
    base_firewall: Vec<&'static str>,
    delayed_release: bool,
}
impl ReferenceKernel {
    fn new() -> Self {
        Self {
            owner: Some(CONTEXT),
            bans: BTreeMap::new(),
            base_firewall: vec!["base:ssh", "base:default-policy"],
            delayed_release: false,
        }
    }
    fn commit(&mut self, tx: &Transaction) -> Result<(), Error> {
        if self.owner != Some(tx.context()) {
            return Err(Error::Owner);
        }
        assert_eq!(tx.table(), TABLE);
        assert_eq!(
            (tx.destination(), tx.port()),
            (scope().destination(), 18443)
        );
        match tx.operation() {
            Operation::Ban => {
                if !self.bans.contains_key(&tx.ip()) && self.bans.len() == CAPACITY {
                    return Err(Error::Capacity);
                }
                self.bans.insert(tx.ip(), tx.deadline());
            }
            Operation::Unban => {
                self.bans.remove(&tx.ip());
            }
        }
        Ok(())
    }
    fn drops(&self, source: Ipv4Addr, dest: Ipv4Addr, port: u16, boot_ns: u64) -> bool {
        self.drops_protocol(source, dest, 6, port, boot_ns)
    }
    fn drops_protocol(
        &self,
        source: Ipv4Addr,
        dest: Ipv4Addr,
        protocol: u8,
        port: u16,
        boot_ns: u64,
    ) -> bool {
        self.owner.is_some()
            && protocol == 6
            && dest == scope().destination()
            && port == 18443
            && self
                .bans
                .get(&source)
                .is_some_and(|deadline| boot_ns < *deadline)
    }
    fn release(&mut self, owner: ObservedContext) {
        if self.owner == Some(owner) && !self.delayed_release {
            self.owner = None;
            self.bans.clear();
        }
    }
}

fn confirmed(
    b: &mut ModelBroker,
    k: &mut ReferenceKernel,
    operation: Operation,
    target: Ipv4Addr,
    ttl: u32,
    now: u64,
) -> Transaction {
    let wire = preview(b, operation, target, ttl, now);
    let tx = b.prepare(CONTEXT, &wire, now).unwrap();
    k.commit(&tx).unwrap();
    b.finish(Completion::Confirmed(tx.clone())).unwrap();
    tx
}

#[test]
fn malformed_lengths_and_headers_never_mutate() {
    let mut b = broker();
    let good = preview(&b, Operation::Ban, ip(1), 30, 0);
    for size in 0..=1024 {
        if size == FRAME_BYTES {
            continue;
        }
        assert_eq!(b.prepare(CONTEXT, &vec![0; size], 0), Err(Error::Frame));
    }
    for at in 0..4 {
        let mut wire = good;
        wire[at] ^= 1;
        assert_eq!(b.prepare(CONTEXT, &wire, 0), Err(Error::Frame));
    }
    for version in 0..=255 {
        if version == 1 {
            continue;
        }
        let mut wire = good;
        wire[4] = version;
        assert_eq!(b.prepare(CONTEXT, &wire, 0), Err(Error::Version));
    }
    for op in 0..=255 {
        if op == 1 || op == 2 {
            continue;
        }
        let mut wire = good;
        wire[5] = op;
        assert_eq!(b.prepare(CONTEXT, &wire, 0), Err(Error::Operation));
    }
    for at in [6, 7] {
        let mut wire = good;
        wire[at] = 1;
        assert_eq!(b.prepare(CONTEXT, &wire, 0), Err(Error::Reserved));
    }
    assert_eq!(
        (b.revision(), b.occupied(), b.phase()),
        (0, 0, Phase::Ready)
    );
}

#[test]
fn unauthorized_peer_rejected_before_parsing() {
    let mut b = broker();
    assert_eq!(
        b.prepare(
            ObservedContext {
                uid: 1002,
                ..CONTEXT
            },
            &[0; 2048],
            0
        ),
        Err(Error::Peer)
    );
    assert_eq!(b.revision(), 0);
}

#[test]
fn different_namespace_or_socket_cannot_submit_or_close() {
    let mut b = broker();
    let wire = preview(&b, Operation::Ban, ip(1), 30, 0);
    for (seen, error) in [
        (
            ObservedContext {
                namespace: 13,
                ..CONTEXT
            },
            Error::Namespace,
        ),
        (
            ObservedContext {
                owner_socket: 43,
                ..CONTEXT
            },
            Error::Owner,
        ),
    ] {
        assert_eq!(b.prepare(seen, &wire, 0), Err(error));
        assert_eq!(b.close_owner(seen), Err(error));
    }
    assert_eq!((b.revision(), b.occupied()), (0, 0));
}

#[test]
fn requests_cannot_change_scope_table_or_transport() {
    let mut b = broker();
    let wire = preview(&b, Operation::Ban, ip(1), 30, 0);
    let tx = b.prepare(CONTEXT, &wire, 0).unwrap();
    assert_eq!(
        (tx.table(), tx.destination(), tx.port(), tx.context()),
        (TABLE, scope().destination(), 18443, CONTEXT)
    );
    // Appending command text cannot become a second message or nft statement.
    let mut injected = wire.to_vec();
    injected.extend_from_slice(b"; flush ruleset\n");
    assert_eq!(Request::decode(&injected), Err(Error::Frame));
}

#[test]
fn reject_protected_destination_invalid_addresses_and_ttls() {
    let b = broker();
    for addr in [
        "0.0.0.0",
        "0.1.2.3",
        "127.0.0.1",
        "224.0.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "198.51.100.7",
        "10.99.0.66",
    ] {
        assert_eq!(
            b.preview(CONTEXT, Operation::Ban, addr.parse().unwrap(), 30, 0),
            Err(Error::InvalidIp)
        );
    }
    for ttl in [0, MAX_TTL_SECS + 1, u32::MAX] {
        assert_eq!(
            b.preview(CONTEXT, Operation::Ban, ip(1), ttl, 0),
            Err(Error::InvalidTtl)
        );
    }
    assert_eq!(
        b.preview(CONTEXT, Operation::Unban, ip(1), 30, 0),
        Err(Error::InvalidTtl)
    );
    assert_eq!(
        b.preview(CONTEXT, Operation::Ban, ip(1), 1, u64::MAX),
        Err(Error::Overflow)
    );
}

#[test]
fn root_peer_and_missing_identity_are_not_default_configs() {
    assert!(Scope::new(
        scope().destination(),
        18443,
        ObservedContext { uid: 0, ..CONTEXT },
        &[]
    )
    .is_err());
    assert!(Scope::new(scope().destination(), 0, CONTEXT, &[]).is_err());
    assert!(Scope::new(
        scope().destination(),
        18443,
        ObservedContext {
            namespace: 0,
            ..CONTEXT
        },
        &[]
    )
    .is_err());
    assert!(Scope::new(
        scope().destination(),
        18443,
        ObservedContext {
            owner_socket: 0,
            ..CONTEXT
        },
        &[]
    )
    .is_err());
    assert!(Scope::new(
        scope().destination(),
        18443,
        CONTEXT,
        &[ip(1); CAPACITY + 1]
    )
    .is_err());
    assert!(matches!(
        ModelBroker::start(scope(), 0, true),
        Err(Error::Instance)
    ));
}

#[test]
fn existing_foreign_or_residual_table_is_never_adopted() {
    assert!(matches!(
        ModelBroker::start(scope(), 1, false),
        Err(Error::TablePresent)
    ));
    // No table flush/takeover operation exists on the public request interface.
    assert_eq!(
        ReferenceKernel::new().base_firewall,
        vec!["base:ssh", "base:default-policy"]
    );
}

#[test]
fn preview_does_not_reserve_or_start_ttl() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    let wire = preview(&b, Operation::Ban, ip(1), 30, 0);
    assert_eq!((b.revision(), b.occupied()), (0, 0));
    let tx = b.prepare(CONTEXT, &wire, 10 * SECOND).unwrap();
    assert_eq!(tx.deadline(), 40 * SECOND);
    k.commit(&tx).unwrap();
    b.finish(Completion::Confirmed(tx)).unwrap();
    assert!(k.drops(ip(1), scope().destination(), 18443, 39 * SECOND));
    assert!(!k.drops(ip(1), scope().destination(), 18443, 40 * SECOND));
}

#[test]
fn target_only_drop_and_explicit_unban_preserve_other_traffic() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    let baseline = k.base_firewall.clone();
    confirmed(&mut b, &mut k, Operation::Ban, ip(1), 30, 0);
    assert!(k.drops(ip(1), scope().destination(), 18443, SECOND));
    assert!(!k.drops(ip(2), scope().destination(), 18443, SECOND));
    assert!(!k.drops(ip(1), scope().destination(), 22, SECOND));
    assert!(!k.drops_protocol(ip(1), scope().destination(), 17, 18443, SECOND));
    assert!(!k.drops(ip(1), Ipv4Addr::new(10, 99, 0, 67), 18443, SECOND));
    confirmed(&mut b, &mut k, Operation::Unban, ip(1), 0, SECOND);
    assert!(!k.drops(ip(1), scope().destination(), 18443, SECOND));
    assert_eq!(k.base_firewall, baseline);
}

#[test]
fn stale_revision_and_old_instance_never_become_new_mutations() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    let old = preview(&b, Operation::Ban, ip(1), 30, 0);
    confirmed(&mut b, &mut k, Operation::Ban, ip(2), 30, 0);
    assert_eq!(b.prepare(CONTEXT, &old, SECOND), Err(Error::Revision));
    let stale_instance = Request {
        operation: Operation::Unban,
        instance: 999,
        revision: 1,
        ip: ip(2),
        ttl_secs: 0,
    }
    .encode();
    assert_eq!(
        b.prepare(CONTEXT, &stale_instance, SECOND),
        Err(Error::Instance)
    );
    assert!(k.drops(ip(2), scope().destination(), 18443, SECOND));
}

#[test]
fn only_one_inflight_no_wait_queue_and_rate_limit_retained() {
    let mut b = broker();
    let wire = preview(&b, Operation::Ban, ip(1), 30, 0);
    let tx = b.prepare(CONTEXT, &wire, 0).unwrap();
    assert_eq!(b.prepare(CONTEXT, &wire, 0), Err(Error::Busy));
    b.finish(Completion::Confirmed(tx)).unwrap();
    let fresh = preview(&b, Operation::Ban, ip(2), 30, 0);
    assert_eq!(
        b.prepare(CONTEXT, &fresh, CHANGE_INTERVAL_NS - 1),
        Err(Error::RateLimit)
    );
    assert!(b.prepare(CONTEXT, &fresh, CHANGE_INTERVAL_NS).is_ok());
}

#[test]
fn definitive_rejection_restores_previous_state_but_consumes_revision() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    confirmed(&mut b, &mut k, Operation::Ban, ip(1), 30, 0);
    let previous = *b.entries();
    let wire = preview(&b, Operation::Ban, ip(1), 3, SECOND);
    b.prepare(CONTEXT, &wire, SECOND).unwrap();
    assert_eq!(
        b.finish(Completion::DefinitiveRejection),
        Err(Error::Rejected)
    );
    assert_eq!(b.entries(), &previous);
    assert_eq!(b.revision(), 2);
    assert_eq!(b.prepare(CONTEXT, &wire, 2 * SECOND), Err(Error::Revision));
    assert_eq!(k.bans.get(&ip(1)), Some(&(30 * SECOND)));
}

#[test]
fn commit_then_ack_loss_is_unknown_never_automatically_retried() {
    for committed in [false, true] {
        let mut b = broker();
        let mut k = ReferenceKernel::new();
        let wire = preview(&b, Operation::Ban, ip(1), 30, 0);
        let tx = b.prepare(CONTEXT, &wire, 0).unwrap();
        if committed {
            k.commit(&tx).unwrap();
        }
        assert_eq!(b.finish(Completion::Unknown), Err(Error::Unknown));
        assert_eq!(b.phase(), Phase::Quarantined);
        assert_eq!(b.occupied(), 1);
        assert!(!b.entries()[0].unwrap().confirmed);
        assert_eq!(b.prepare(CONTEXT, &wire, SECOND), Err(Error::NotReady));
        assert_eq!(b.sweep(60 * SECOND), Err(Error::NotReady));
        assert_eq!(k.bans.contains_key(&ip(1)), committed);
    }
}

#[test]
fn unknown_shorter_replacement_keeps_old_longer_horizon() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    confirmed(&mut b, &mut k, Operation::Ban, ip(1), 100, 0);
    let wire = preview(&b, Operation::Ban, ip(1), 3, SECOND);
    b.prepare(CONTEXT, &wire, SECOND).unwrap();
    assert_eq!(b.finish(Completion::Unknown), Err(Error::Unknown));
    assert_eq!(b.entries()[0].unwrap().deadline_boot_ns, 100 * SECOND);
    assert!(k.drops(ip(1), scope().destination(), 18443, 10 * SECOND));
}

#[test]
fn unknown_unban_never_claims_release_or_accepts_followup() {
    for committed in [false, true] {
        let mut b = broker();
        let mut k = ReferenceKernel::new();
        confirmed(&mut b, &mut k, Operation::Ban, ip(1), 100, 0);
        let wire = preview(&b, Operation::Unban, ip(1), 0, SECOND);
        let tx = b.prepare(CONTEXT, &wire, SECOND).unwrap();
        if committed {
            k.commit(&tx).unwrap();
        }
        assert_eq!(b.finish(Completion::Unknown), Err(Error::Unknown));
        assert_eq!(b.occupied(), 1);
        assert!(!b.entries()[0].unwrap().confirmed);
        assert_eq!(k.bans.contains_key(&ip(1)), !committed);
    }
}

#[test]
fn wrong_receipt_quarantines_without_forgetting_pending() {
    let mut b = broker();
    let mut other = broker();
    let wire = preview(&b, Operation::Ban, ip(1), 30, 0);
    b.prepare(CONTEXT, &wire, 0).unwrap();
    let wrong = other
        .prepare(
            CONTEXT,
            &preview(&broker(), Operation::Ban, ip(2), 30, 0),
            0,
        )
        .unwrap();
    assert_eq!(b.finish(Completion::Confirmed(wrong)), Err(Error::Receipt));
    assert_eq!((b.phase(), b.occupied()), (Phase::Quarantined, 1));
}

#[test]
fn old_generation_receipt_is_rejected_even_if_socket_id_and_revision_repeat() {
    let mut previous = broker();
    let old_wire = preview(&previous, Operation::Ban, ip(1), 30, 0);
    let old_transaction = previous.prepare(CONTEXT, &old_wire, 0).unwrap();
    // Numeric socket IDs may be reused. The generation must remain binding.
    let mut restarted = ModelBroker::start(scope(), 2, true).unwrap();
    let fresh_wire = preview(&restarted, Operation::Ban, ip(1), 30, 0);
    restarted.prepare(CONTEXT, &fresh_wire, 0).unwrap();
    assert_eq!(
        restarted.finish(Completion::Confirmed(old_transaction)),
        Err(Error::Receipt)
    );
    assert_eq!(restarted.phase(), Phase::Quarantined);
}

#[test]
fn capacity_256_rejects_257_without_eviction_or_revision_change() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    for n in 0..=255 {
        confirmed(
            &mut b,
            &mut k,
            Operation::Ban,
            ip(n),
            180,
            u64::from(n) * CHANGE_INTERVAL_NS,
        );
    }
    let before = *b.entries();
    let kernel_before = k.bans.clone();
    let target = Ipv4Addr::new(198, 19, 0, 1);
    assert_eq!(
        b.preview(CONTEXT, Operation::Ban, target, 180, 60 * SECOND),
        Err(Error::Capacity)
    );
    let wire = Request {
        operation: Operation::Ban,
        instance: 1,
        revision: 256,
        ip: target,
        ttl_secs: 180,
    }
    .encode();
    assert_eq!(b.prepare(CONTEXT, &wire, 60 * SECOND), Err(Error::Capacity));
    assert_eq!(b.revision(), 256);
    assert_eq!(b.entries(), &before);
    assert_eq!(k.bans, kernel_before);
}

#[test]
fn expired_entries_pass_before_gc_and_cleanup_work_is_bounded() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    for n in 0..=255 {
        confirmed(
            &mut b,
            &mut k,
            Operation::Ban,
            ip(n),
            180,
            u64::from(n) * CHANGE_INTERVAL_NS,
        );
    }
    assert!(!k.drops(ip(0), scope().destination(), 18443, 300 * SECOND));
    assert_eq!(b.occupied(), 256);
    for i in 0..16 {
        assert_eq!(b.sweep(300 * SECOND), Ok(SWEEP_BUDGET));
        assert_eq!(b.occupied(), 256 - (i + 1) * SWEEP_BUDGET);
    }
    assert_eq!(b.revision(), 256);
    // The reference kernel may still physically hold expired objects. A real
    // adapter must not infer kernel capacity from this userspace mirror.
    assert_eq!(k.bans.len(), 256);
}

#[test]
fn time_cannot_reverse_or_overflow() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    confirmed(&mut b, &mut k, Operation::Ban, ip(1), 30, 10 * SECOND);
    assert_eq!(
        b.preview(CONTEXT, Operation::Ban, ip(2), 30, 9 * SECOND),
        Err(Error::Clock)
    );
    assert_eq!(b.sweep(9 * SECOND), Err(Error::Clock));
    assert_eq!(
        relative_deadline_at_kernel_init(u64::MAX, 1),
        Err(Error::Overflow)
    );
}

#[test]
fn kill_at_every_transaction_boundary_requires_absence_before_restart() {
    // Kill before prepare, after prepare, after commit before ACK, and after ACK.
    for point in 0..4 {
        let mut b = broker();
        let mut k = ReferenceKernel::new();
        let baseline = k.base_firewall.clone();
        let stale = preview(&b, Operation::Ban, ip(1), 30, 0);
        if point > 0 {
            let tx = b.prepare(CONTEXT, &stale, 0).unwrap();
            if point > 1 {
                k.commit(&tx).unwrap();
            }
            if point > 2 {
                b.finish(Completion::Confirmed(tx)).unwrap();
            }
        }
        b.close_owner(CONTEXT).unwrap();
        k.delayed_release = true;
        k.release(CONTEXT);
        assert_eq!(
            b.observe_absent_after_close(CONTEXT, k.owner.is_none()),
            Err(Error::TablePresent)
        );
        assert!(matches!(
            ModelBroker::start(scope(), 2, k.owner.is_none()),
            Err(Error::TablePresent)
        ));
        k.delayed_release = false;
        k.release(CONTEXT);
        b.observe_absent_after_close(CONTEXT, k.owner.is_none())
            .unwrap();
        assert_eq!((b.phase(), b.occupied()), (Phase::Stopped, 0));
        let mut fresh = ModelBroker::start(scope(), 2, k.owner.is_none()).unwrap();
        assert_eq!(fresh.prepare(CONTEXT, &stale, SECOND), Err(Error::Instance));
        assert_eq!(fresh.occupied(), 0);
        assert_eq!(k.base_firewall, baseline);
    }
}

#[test]
fn closing_rejects_new_work_and_late_ack() {
    let mut b = broker();
    let wire = preview(&b, Operation::Ban, ip(1), 30, 0);
    let tx = b.prepare(CONTEXT, &wire, 0).unwrap();
    b.close_owner(CONTEXT).unwrap();
    assert_eq!(b.finish(Completion::Confirmed(tx)), Err(Error::NotReady));
    assert_eq!(b.prepare(CONTEXT, &wire, SECOND), Err(Error::NotReady));
    assert_eq!(b.phase(), Phase::Closing);
}

#[test]
fn wrong_socket_cannot_destroy_reference_kernel_table() {
    let mut b = broker();
    let mut k = ReferenceKernel::new();
    confirmed(&mut b, &mut k, Operation::Ban, ip(1), 30, 0);
    k.release(ObservedContext {
        owner_socket: 43,
        ..CONTEXT
    });
    assert_eq!(k.owner, Some(CONTEXT));
    assert!(k.drops(ip(1), scope().destination(), 18443, SECOND));
}

#[test]
fn relative_timeout_starts_at_kernel_init_and_extends_intended_deadline() {
    let intended = 30 * SECOND;
    let observed = relative_deadline_at_kernel_init(SECOND, 30 * SECOND).unwrap();
    assert_eq!(observed, 31 * SECOND);
    assert!(observed > intended);
    // Even subtracting elapsed time at send cannot account for an unbounded
    // queue delay between send and kernel element initialization.
    let send_at = 10 * SECOND;
    let observed = relative_deadline_at_kernel_init(send_at + SECOND, intended - send_at).unwrap();
    assert!(observed > intended);
}

#[test]
fn suspend_clock_counterexample_is_not_repaired_by_userspace_sweep() {
    // Explicit model assumptions, not a claim of a suspend test on Linux:
    // BOOTTIME advances during sleep while the timeout clock does not.
    let intended_boot_deadline = 5 * SECOND;
    let timeout_tick_deadline = 5 * SECOND;
    let boot_after_suspend = 11 * SECOND;
    let ticks_after_suspend = SECOND;
    assert!(boot_after_suspend >= intended_boot_deadline);
    assert!(ticks_after_suspend < timeout_tick_deadline);
    // Frozen userspace cannot run cleanup before the first resumed packet.
}

#[test]
fn deployment_gate_requires_every_piece_of_real_evidence() {
    let all = DeploymentEvidence {
        isolated_namespace: true,
        os_peer_credentials_verified: true,
        capabilities_confined: true,
        owner_without_persist_verified: true,
        absolute_boottime_deadline_verified: true,
        recovery_and_packet_tests_verified: true,
    };
    assert!(all.missing().is_empty());
    let cases = [
        DeploymentEvidence {
            isolated_namespace: false,
            ..all
        },
        DeploymentEvidence {
            os_peer_credentials_verified: false,
            ..all
        },
        DeploymentEvidence {
            capabilities_confined: false,
            ..all
        },
        DeploymentEvidence {
            owner_without_persist_verified: false,
            ..all
        },
        DeploymentEvidence {
            absolute_boottime_deadline_verified: false,
            ..all
        },
        DeploymentEvidence {
            recovery_and_packet_tests_verified: false,
            ..all
        },
    ];
    for missing in cases {
        assert_eq!(missing.missing().len(), 1);
    }
    // Even if other pieces were verified, the current nft relative timeout
    // evidence cannot authorize the unchanged absolute deadline contract.
    let current = DeploymentEvidence {
        absolute_boottime_deadline_verified: false,
        ..all
    };
    assert_eq!(current.missing(), vec!["absolute-CLOCK_BOOTTIME-deadline"]);
}
