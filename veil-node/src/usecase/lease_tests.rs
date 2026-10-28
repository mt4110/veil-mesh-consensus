use super::*;
use crate::{ebpf_bridge::MockEbpfBridge, infra::wal::FileWal};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};

struct Clock {
    boot: String,
    now: AtomicU64,
}
impl Clock {
    fn at(boot: &str, seconds: u64) -> Arc<Self> {
        Arc::new(Self {
            boot: boot.into(),
            now: AtomicU64::new(seconds * 1_000_000_000),
        })
    }
    fn set(&self, seconds: u64) {
        self.now.store(seconds * 1_000_000_000, Ordering::SeqCst);
    }
}
impl PolicyClock for Clock {
    fn boot_id(&self) -> &str {
        &self.boot
    }
    fn now_ns(&self) -> std::io::Result<u64> {
        Ok(self.now.load(Ordering::SeqCst))
    }
    fn lease_time(&self) -> std::io::Result<LeaseTime> {
        let now = self.now_ns()?;
        Ok(LeaseTime {
            boot_ns: now,
            unix_ns: SIMULATION_WALL_EPOCH_NS + now,
            uncertainty_ns: 0,
            max_slow_rate_ppm: 0,
        })
    }
}

#[derive(Default)]
struct Bridge {
    mock: MockEbpfBridge,
    fail: AtomicBool,
    fail_policy: AtomicBool,
    writes: Mutex<Vec<(bool, PeerPolicy)>>,
}
impl EbpfBridge for Bridge {
    fn sync_peer_policy(&self, ip: u32, policy: PeerPolicy) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.fail.load(Ordering::SeqCst) && !self.fail_policy.load(Ordering::SeqCst),
            "injected map failure"
        );
        self.writes
            .lock()
            .unwrap()
            .push((*self.mock.xdp_bypass.lock().unwrap(), policy));
        self.mock.sync_peer_policy(ip, policy)
    }
    fn set_xdp_bypass(&self, enabled: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.fail.load(Ordering::SeqCst),
            "injected control failure"
        );
        self.mock.set_xdp_bypass(enabled)
    }
    fn sync_lsm_policy(&self, p: NodeLsmPolicy) -> anyhow::Result<()> {
        self.mock.sync_lsm_policy(p)
    }
    fn stealth_gc_purge_count(&self) -> u64 {
        0
    }
}

const IP: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 44);
fn state(clock: Arc<Clock>, bridge: Arc<Bridge>) -> ConsensusState {
    let mut s = ConsensusState::new("solo", vec![]).with_ebpf_bridge(bridge);
    s.configure_policy_clock(clock).unwrap();
    s
}
fn path() -> std::path::PathBuf {
    super::regression_tests::fixture_dir().join("lease.wal")
}
fn start(clock: Arc<Clock>, bridge: Arc<Bridge>, path: &std::path::Path) -> ConsensusState {
    let mut s = state(clock, bridge);
    s.enable_persistence(Arc::new(FileWal::open(path).unwrap()))
        .unwrap();
    s.start_signed_election().unwrap();
    s
}

#[test]
fn restart_keeps_original_deadline_and_expiry_needs_no_gc_tick() {
    let p = path();
    let clock = Clock::at("boot-a", 100);
    let mut s = start(clock.clone(), Arc::new(Bridge::default()), &p);
    s.propose(MeshCommand::set_trust_with_ttl(IP, false, 10))
        .unwrap();
    assert_eq!(s.peer_rule_deadlines_ns[&u32::from(IP)], 110_000_000_000);
    drop(s);
    clock.set(109);
    let bridge = Arc::new(Bridge::default());
    let mut recovered = state(clock.clone(), bridge.clone());
    recovered.load_wal(&p).unwrap();
    assert_eq!(recovered.check_xdp_packet(IP), XdpVerdict::Drop);
    assert_eq!(
        bridge.mock.kernel_peer_map.lock().unwrap()[&u32::from(IP)].expires_at_boot_ns,
        110_000_000_000
    );
    clock.set(110);
    assert_eq!(recovered.check_xdp_packet(IP), XdpVerdict::Pass);
    recovered.retry_pending_apply();
    assert!(recovered.ebpf_policy_map.is_empty());
}

#[test]
fn expired_lease_never_reappears_before_or_after_gc_or_os_reboot() {
    for gc in [false, true] {
        let p = path();
        let clock = Clock::at("boot-a", 100);
        let mut s = start(clock.clone(), Arc::new(Bridge::default()), &p);
        s.propose(MeshCommand::set_trust_with_ttl(IP, false, 10))
            .unwrap();
        clock.set(111);
        if gc {
            s.retry_pending_apply();
            s.checkpoint().unwrap();
        }
        drop(s);
        for recovery_clock in [clock.clone(), Clock::at("boot-b", 0)] {
            let bridge = Arc::new(Bridge::default());
            let mut restored = state(recovery_clock, bridge.clone());
            restored.load_wal(&p).unwrap();
            restored.retry_pending_apply();
            assert_eq!(restored.check_xdp_packet(IP), XdpVerdict::Pass);
            assert!(bridge
                .writes
                .lock()
                .unwrap()
                .iter()
                .all(|(_, policy)| policy.trusted != 0));
        }
    }
}

#[test]
fn failed_apply_and_batch_retry_cannot_rearm_lease() {
    let p = path();
    let clock = Clock::at("boot-a", 100);
    let bridge = Arc::new(Bridge::default());
    let mut s = start(clock.clone(), bridge.clone(), &p);
    bridge.fail.store(true, Ordering::SeqCst);
    let _ = s.propose(MeshCommand::set_trust_with_ttl(IP, false, 10));
    assert_eq!(s.last_applied, 0);
    assert_eq!(s.lease_deadlines[&(1, 0)], 110_000_000_000);
    clock.set(111);
    bridge.fail.store(false, Ordering::SeqCst);
    s.retry_pending_apply();
    assert_eq!(s.last_applied, 1);
    assert!(bridge
        .writes
        .lock()
        .unwrap()
        .iter()
        .all(|(_, p)| p.trusted != 0));
}

struct LeaseFailStore {
    disk: FileWal,
}
impl ConsensusPersistence for LeaseFailStore {
    fn persist(&self, bytes: &[u8]) -> std::io::Result<()> {
        let e: WalEnvelope = bincode::deserialize(bytes).unwrap();
        let p: WalPayload = bincode::deserialize(&e.payload_bytes).unwrap();
        if !p.lease_deadlines.is_empty() {
            return Err(std::io::Error::other("injected lease fsync failure"));
        }
        self.disk.persist(bytes)
    }
}

#[test]
fn crash_between_commit_and_lease_preparation_never_creates_a_new_lease() {
    let p = path();
    let clock = Clock::at("boot-a", 100);
    let bridge = Arc::new(Bridge::default());
    let mut s = state(clock.clone(), bridge.clone());
    s.enable_persistence(Arc::new(LeaseFailStore {
        disk: FileWal::open(&p).unwrap(),
    }))
    .unwrap();
    s.start_signed_election().unwrap();
    let _ = s.propose(MeshCommand::set_trust_with_ttl(IP, false, 10));
    assert!(s.persistence_error.is_some());
    assert!(bridge.writes.lock().unwrap().is_empty());
    drop(s);
    let mut restored = state(clock, bridge.clone());
    restored.load_wal(&p).unwrap();
    assert_eq!(restored.commit_index, 1);
    assert_eq!(restored.check_xdp_packet(IP), XdpVerdict::Pass);
    assert!(bridge
        .writes
        .lock()
        .unwrap()
        .iter()
        .all(|(_, p)| p.trusted != 0));
}

#[test]
fn recovery_applies_only_final_rule_and_restores_bypass_before_any_ban() {
    let p = path();
    let clock = Clock::at("boot-a", 100);
    let mut s = start(clock.clone(), Arc::new(Bridge::default()), &p);
    s.propose(MeshCommand::set_trust_with_ttl(IP, false, 300))
        .unwrap();
    s.propose(MeshCommand::set_trust(IP, true)).unwrap();
    let other = Ipv4Addr::new(192, 0, 2, 45);
    s.propose(MeshCommand::set_trust_with_ttl(other, false, 300))
        .unwrap();
    s.set_emergency_local_bypass(true).unwrap();
    drop(s);
    let bridge = Arc::new(Bridge::default());
    let mut restored = state(clock, bridge.clone());
    restored.load_wal(&p).unwrap();
    let writes = bridge.writes.lock().unwrap();
    assert_eq!(writes.len(), 2);
    assert!(writes.iter().all(|(bypass, _)| *bypass));
    assert_eq!(restored.check_xdp_packet(other), XdpVerdict::Pass);
    assert_eq!(
        bridge.mock.kernel_peer_map.lock().unwrap()[&u32::from(IP)].trusted,
        1
    );
}

#[test]
fn bypass_failure_is_not_acknowledged_and_pending_control_is_retried() {
    let p = path();
    let bridge = Arc::new(Bridge::default());
    let mut s = start(Clock::at("boot-a", 100), bridge.clone(), &p);
    bridge.fail.store(true, Ordering::SeqCst);
    assert!(s.set_emergency_local_bypass(true).is_err());
    assert!(s.last_apply_error.is_some());
    assert!(!*bridge.mock.xdp_bypass.lock().unwrap());
    bridge.fail.store(false, Ordering::SeqCst);
    s.retry_pending_apply();
    assert!(*bridge.mock.xdp_bypass.lock().unwrap());
}

#[tokio::test]
async fn owner_socket_acknowledges_durable_kernel_bypass() {
    use crate::infra::local_control;
    use std::os::unix::fs::PermissionsExt;
    let p = path();
    std::fs::set_permissions(p.parent().unwrap(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let bridge = Arc::new(Bridge::default());
    let s = start(Clock::at("boot-a", 100), bridge.clone(), &p);
    let socket = p.with_extension("sock");
    let listener = local_control::bind(&socket).unwrap();
    let state = Arc::new(tokio::sync::Mutex::new(s));
    let server = tokio::spawn(local_control::serve(listener, state.clone()));
    local_control::request(&socket, true).await.unwrap();
    assert!(*bridge.mock.xdp_bypass.lock().unwrap());
    bridge.fail.store(true, Ordering::SeqCst);
    assert!(local_control::request(&socket, false).await.is_err());
    assert!(*bridge.mock.xdp_bypass.lock().unwrap());
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    server.abort();
}

#[test]
fn disk_failure_cannot_disable_bypass_and_enabling_still_relaxes_kernel() {
    let p = path();
    let bridge = Arc::new(Bridge::default());
    let mut s = start(Clock::at("boot-a", 100), bridge.clone(), &p);
    s.persistence_error = Some("injected latched storage failure".into());
    assert!(s.set_emergency_local_bypass(true).is_err());
    assert!(*bridge.mock.xdp_bypass.lock().unwrap());
    assert!(s.set_emergency_local_bypass(false).is_err());
    assert!(s.emergency_local_bypass);
    assert!(*bridge.mock.xdp_bypass.lock().unwrap());
    s.retry_pending_apply();
    assert!(*bridge.mock.xdp_bypass.lock().unwrap());
}

#[test]
fn legacy_wal_is_rejected_without_overwriting_votes_or_applying_rules() {
    let p = path();
    let s = state(Clock::at("boot-a", 100), Arc::new(Bridge::default()));
    let mut envelope: WalEnvelope = bincode::deserialize(&s.wal_bytes().unwrap()).unwrap();
    envelope.payload_bytes[..4].copy_from_slice(&1u32.to_le_bytes());
    envelope.sha256_checksum = ring::digest::digest(&ring::digest::SHA256, &envelope.payload_bytes)
        .as_ref()
        .try_into()
        .unwrap();
    let bytes = bincode::serialize(&envelope).unwrap();
    FileWal::open(&p).unwrap().persist(&bytes).unwrap();
    let bridge = Arc::new(Bridge::default());
    let mut restored = state(Clock::at("boot-a", 100), bridge.clone());
    assert!(restored
        .load_wal(&p)
        .unwrap_err()
        .to_string()
        .contains("explicit migration"));
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    assert!(bridge.writes.lock().unwrap().is_empty());
}

#[test]
fn backwards_boot_clock_stops_new_apply_without_extending_deadline() {
    let p = path();
    let clock = Clock::at("boot-a", 100);
    let bridge = Arc::new(Bridge::default());
    let mut s = start(clock.clone(), bridge.clone(), &p);
    s.propose(MeshCommand::set_trust_with_ttl(IP, false, 10))
        .unwrap();
    let count = bridge.writes.lock().unwrap().len();
    clock.set(99);
    s.retry_pending_apply();
    assert!(s.last_apply_error.as_ref().unwrap().contains("backwards"));
    assert_eq!(bridge.writes.lock().unwrap().len(), count);
    assert_eq!(s.lease_deadlines[&(1, 0)], 110_000_000_000);
}

#[test]
fn recovery_filter_does_not_skip_new_commits_after_a_failed_restore() {
    let p = path();
    let clock = Clock::at("boot-a", 100);
    let mut s = start(clock.clone(), Arc::new(Bridge::default()), &p);
    s.propose(MeshCommand::set_trust_with_ttl(IP, false, 300))
        .unwrap();
    drop(s);
    let bridge = Arc::new(Bridge::default());
    // Bypass restoration succeeds, then a policy write fails.
    bridge.fail_policy.store(true, Ordering::SeqCst);
    let mut restored = state(clock, bridge.clone());
    restored.load_wal(&p).unwrap();
    assert_eq!(restored.last_applied, 0);
    restored
        .enable_persistence(Arc::new(FileWal::open(&p).unwrap()))
        .unwrap();
    restored.start_signed_election().unwrap();
    let other = Ipv4Addr::new(192, 0, 2, 99);
    let _ = restored.propose(MeshCommand::set_trust_with_ttl(other, false, 300));
    assert_eq!(restored.commit_index, 2);
    bridge.fail_policy.store(false, Ordering::SeqCst);
    restored.retry_pending_apply();
    assert_eq!(restored.last_applied, 2);
    assert_eq!(
        bridge.mock.kernel_peer_map.lock().unwrap()[&u32::from(other)].trusted,
        0
    );
}
