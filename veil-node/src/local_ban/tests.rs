use super::*;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

#[derive(Default)]
struct Map {
    entries: Mutex<BTreeMap<u32, PeerPolicy>>,
    fail_insert: AtomicBool,
    fail_delete: AtomicBool,
}
impl Backend for Map {
    fn insert(&self, ip: u32, p: PeerPolicy) -> anyhow::Result<()> {
        if self.fail_insert.load(Ordering::SeqCst) {
            anyhow::bail!("injected");
        }
        self.entries.lock().unwrap().insert(ip, p);
        Ok(())
    }
    fn remove(&self, ip: u32) -> anyhow::Result<()> {
        if self.fail_delete.load(Ordering::SeqCst) {
            anyhow::bail!("injected");
        }
        self.entries.lock().unwrap().remove(&ip);
        Ok(())
    }
    fn bypass(&self, _: bool) -> anyhow::Result<()> {
        Ok(())
    }
    fn counters(&self) -> anyhow::Result<(u64, u64)> {
        Ok((0, 0))
    }
}
fn fixture() -> (Engine, Arc<Map>) {
    let map = Arc::new(Map::default());
    (
        Engine::new(map.clone(), &[Ipv4Addr::new(10, 99, 0, 1)]).unwrap(),
        map,
    )
}
fn ip(n: u32) -> u32 {
    u32::from(Ipv4Addr::new(198, 18, 0, 1)) + n
}

#[test]
fn full_table_refuses_new_ip_without_evicting_and_same_ip_is_explicit_update() {
    let (mut e, m) = fixture();
    for n in 0..256 {
        e.ban(ip(n), 1000, n as u64, STEP_NS * (n as u64 + 1))
            .unwrap();
    }
    assert_eq!(
        e.ban(ip(256), 1000, 256, 100 * 1_000_000_000),
        Err("CAPACITY")
    );
    assert_eq!(
        (e.occupied(), m.entries.lock().unwrap().len(), e.revision()),
        (256, 256, 256)
    );
    e.ban(ip(0), 1000, 256, 101 * 1_000_000_000).unwrap();
    assert_eq!(e.occupied(), 256);
}

#[test]
fn kernel_deadline_passes_before_sweep_and_all_expired_cleanup_is_split() {
    let (mut e, m) = fixture();
    for n in 0..256 {
        e.ban(ip(n), 100, n as u64, STEP_NS * (n as u64 + 1))
            .unwrap();
    }
    let now = 200 * 1_000_000_000;
    assert_eq!(e.active(now), 0);
    assert_eq!(e.occupied(), 256);
    assert!(m
        .entries
        .lock()
        .unwrap()
        .values()
        .all(
            |p| veil_common::evaluate_peer_policy_at(Some(p), now) == veil_common::XdpVerdict::Pass
        ));
    for batch in 0..16 {
        let s = e.sweep(now).unwrap();
        assert_eq!((s.examined, s.attempted, s.removed), (16, 16, 16));
        assert_eq!(e.occupied(), 256 - 16 * (batch + 1));
    }
    assert_eq!(
        (e.event_count(), e.max_examined, e.max_attempted),
        (64, 16, 16)
    );
}

#[test]
fn failures_keep_slot_and_original_deadline_until_verified_delete() {
    let (mut e, m) = fixture();
    m.fail_insert.store(true, Ordering::SeqCst);
    assert_eq!(e.ban(ip(0), 2, 0, 1_000_000_000), Err("MAP_INSERT_FAILED"));
    assert_eq!((e.occupied(), e.pending(), e.revision()), (1, 1, 1));
    let original = e.deadline(ip(0));
    m.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(e.unban(ip(0), 1, 2_000_000_000), Err("MAP_DELETE_FAILED"));
    e.sweep(4_000_000_000).unwrap();
    assert_eq!(e.occupied(), 1);
    assert_eq!(e.deadline(ip(0)), original);
    m.fail_delete.store(false, Ordering::SeqCst);
    for _ in 0..16 {
        e.sweep(4_000_000_000).unwrap();
    }
    assert_eq!(e.occupied(), 0);
}

#[test]
fn release_then_replayed_ban_is_rejected_and_sweeping_does_not_extend() {
    let (mut e, _) = fixture();
    e.ban(ip(0), 2, 0, 1_000_000_000).unwrap();
    let deadline = e.deadline(ip(0));
    e.sweep(1_200_000_000).unwrap();
    assert_eq!(e.deadline(ip(0)), deadline);
    e.unban(ip(0), 1, 1_400_000_000).unwrap();
    assert_eq!(e.ban(ip(0), 2, 0, 1_600_000_000), Err("REVISION_CONFLICT"));
    assert_eq!(e.occupied(), 0);
}

#[test]
fn invalid_time_ttl_protected_targets_and_rate_limit_do_not_allocate() {
    let (mut e, _) = fixture();
    for target in [
        0,
        u32::from(Ipv4Addr::LOCALHOST),
        u32::from(Ipv4Addr::new(127, 2, 3, 4)),
        u32::MAX,
        u32::from(Ipv4Addr::new(224, 0, 0, 1)),
        u32::from(Ipv4Addr::new(10, 99, 0, 1)),
    ] {
        assert_eq!(
            e.ban(target, 1, 0, 1_000_000_000),
            Err("PROTECTED_OR_INVALID_IP")
        );
    }
    for ttl in [0, 86_401] {
        assert_eq!(e.ban(ip(0), ttl, 0, 1_000_000_000), Err("INVALID_TTL"));
    }
    e.ban(ip(0), 1, 0, 1_000_000_000).unwrap();
    assert_eq!(e.ban(ip(1), 1, 1, 1_000_000_001), Err("RATE_LIMIT"));
    assert_eq!(e.ban(ip(1), 1, 1, 1), Err("CLOCK_REVERSED"));
    assert_eq!(e.occupied(), 1);
}

#[test]
fn new_instance_has_no_recovery_input_and_counters_never_wrap() {
    let (mut e, _) = fixture();
    e.ban(ip(0), 100, 0, STEP_NS).unwrap();
    let (empty, _) = fixture();
    assert_eq!((empty.occupied(), empty.revision()), (0, 0));
    e.revision = u64::MAX;
    assert_eq!(
        e.ban(ip(1), 1, u64::MAX, STEP_NS * 2),
        Err("REVISION_EXHAUSTED")
    );
    assert_eq!(
        e.ban(ip(1), 1, u64::MAX, u64::MAX),
        Err("DEADLINE_OVERFLOW")
    );
}

#[test]
fn failed_shortening_does_not_forget_longer_kernel_lease() {
    let (mut e, m) = fixture();
    e.ban(ip(0), 100, 0, 1_000_000_000).unwrap();
    let original = e.deadline(ip(0)).unwrap();
    m.fail_insert.store(true, Ordering::SeqCst);
    assert_eq!(e.ban(ip(0), 1, 1, 2_000_000_000), Err("MAP_INSERT_FAILED"));
    assert_eq!(e.deadline(ip(0)), Some(original));
    assert_eq!(e.verdict(ip(0), 4_000_000_000), "Unknown");
    for _ in 0..16 {
        e.sweep(4_000_000_000).unwrap();
    }
    assert_eq!(e.occupied(), 1);
    assert_eq!(
        m.entries
            .lock()
            .unwrap()
            .get(&ip(0))
            .unwrap()
            .expires_at_boot_ns,
        original
    );
}

#[test]
fn snapshots_and_previews_never_mutate_or_extend_leases() {
    let (mut e, m) = fixture();
    e.ban(ip(0), 2, 0, 1_000_000_000).unwrap();
    let before = (
        e.entries(),
        e.revision(),
        e.event_count(),
        e.last_now,
        e.last_mutation,
    );
    for now in [1_000_000_001, 4_000_000_000] {
        assert_eq!(e.preview_ban(ip(1), 30, now), Ok(()));
        assert_eq!(e.preview_unban(ip(0), now), Ok(()));
        let snapshot = e.entries();
        assert_eq!(snapshot.iter().flatten().count(), 1);
        assert_eq!(snapshot[0].unwrap().expires_at_boot_ns, 3_000_000_000);
    }
    assert_eq!(
        before,
        (
            e.entries(),
            e.revision(),
            e.event_count(),
            e.last_now,
            e.last_mutation
        )
    );
    assert_eq!(m.entries.lock().unwrap().len(), 1);
    let (empty, _) = fixture();
    assert!(empty.entries().iter().all(Option::is_none));
}

#[test]
fn previews_refuse_invalid_targets_ttl_time_and_capacity_without_reserving() {
    let (mut e, _) = fixture();
    assert_eq!(
        e.preview_ban(u32::from(Ipv4Addr::new(10, 99, 0, 1)), 30, STEP_NS),
        Err("PROTECTED_OR_INVALID_IP")
    );
    assert_eq!(e.preview_ban(ip(0), 0, STEP_NS), Err("INVALID_TTL"));
    assert_eq!(
        e.preview_ban(ip(0), MAX_TTL_SECS + 1, STEP_NS),
        Err("INVALID_TTL")
    );
    assert_eq!(e.preview_ban(ip(0), 1, u64::MAX), Err("DEADLINE_OVERFLOW"));
    for n in 0..256 {
        e.ban(ip(n), 1000, n as u64, STEP_NS * (n as u64 + 1))
            .unwrap();
    }
    let before = (e.entries(), e.revision(), e.event_count(), e.last_now);
    assert_eq!(e.preview_ban(ip(256), 30, STEP_NS * 300), Err("CAPACITY"));
    assert_eq!(e.preview_ban(ip(0), 30, STEP_NS * 300), Ok(()));
    assert_eq!(e.preview_unban(ip(0), 0), Err("CLOCK_REVERSED"));
    assert_eq!(
        e.preview_unban(0, STEP_NS * 300),
        Err("PROTECTED_OR_INVALID_IP")
    );
    assert_eq!(
        before,
        (e.entries(), e.revision(), e.event_count(), e.last_now)
    );
}

#[test]
fn snapshot_keeps_pending_and_expired_slots_until_verified_cleanup() {
    let (mut e, m) = fixture();
    m.fail_insert.store(true, Ordering::SeqCst);
    assert_eq!(e.ban(ip(0), 1, 0, STEP_NS), Err("MAP_INSERT_FAILED"));
    let entry = e.entries()[0].unwrap();
    assert!(!entry.confirmed);
    assert_eq!(entry.ip, Ipv4Addr::from(ip(0)));
    assert_eq!(entry.expires_at_boot_ns, STEP_NS + 1_000_000_000);
    assert_eq!(e.entries()[0], Some(entry));
    assert_eq!(e.active(2_000_000_000), 0);
    assert_eq!(e.occupied(), 1);
}
