use super::daemon;
use crate::{
    adapter::NodeCryptoIdentity,
    domain::{FixedConfiguration, Member},
    ebpf_bridge::MockEbpfBridge,
    infra::{local_control, wal::FileWal, ClusterMtlsPki, MeshNode},
    usecase::{ConsensusState, LeaseTime, PolicyClock},
};
use anyhow::Context;
use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncWriteExt, sync::Mutex};

fn directory() -> PathBuf {
    static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "veil-driver-{}-{}-{serial}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    dir
}

// Keep a bounded timing trace even when logging is disabled, so a deadline
// failure identifies disk waits without changing the test's scheduling mode.
struct TimedWal {
    disk: FixtureWal,
    start: Instant,
    writes: std::sync::Mutex<std::collections::VecDeque<WalWrite>>,
    stages: std::sync::Mutex<std::collections::VecDeque<(&'static str, u128)>>,
}
// Test-only A/B selection; production startup never sees this variable.
enum FixtureWal {
    Replace(FileWal),
    Append(super::snapshot_append_prototype::AppendWal),
}
impl FixtureWal {
    fn open(path: &std::path::Path) -> std::io::Result<Self> {
        match std::env::var("VEIL_TEST_WAL").as_deref() {
            Ok("append") => Ok(Self::Append(
                super::snapshot_append_prototype::AppendWal::create_new(
                    path,
                    crate::domain::DEFAULT_MAX_WAL_BYTES,
                )?,
            )),
            Err(std::env::VarError::NotPresent) | Ok("replace") => {
                Ok(Self::Replace(FileWal::open(path)?))
            }
            _ => Err(std::io::Error::other("Unknown test WAL backend")),
        }
    }
    fn persist_observed(
        &self,
        bytes: &[u8],
        observe: impl FnMut(&'static str),
    ) -> std::io::Result<()> {
        match self {
            Self::Replace(wal) => wal.persist_observed(bytes, observe),
            Self::Append(wal) => wal.persist_observed(bytes, observe),
        }
    }
}
impl Drop for TimedWal {
    fn drop(&mut self) {
        if std::env::var_os("VEIL_TEST_WAL_METRICS").is_none() {
            return;
        }
        let Ok(writes) = self.writes.get_mut() else {
            return;
        };
        let Ok(stages) = self.stages.get_mut() else {
            return;
        };
        let mut times: Vec<_> = writes
            .iter()
            .filter_map(|w| w.end_ms.map(|end| end - w.start_ms))
            .collect();
        times.sort_unstable();
        let backend = match self.disk {
            FixtureWal::Replace(_) => "replace",
            FixtureWal::Append(_) => "append",
        };
        eprintln!("WAL_PROTOTYPE_METRIC backend={} samples={} pending={} median_ms={} max_ms={} sampled_file_syncs={} sampled_directory_syncs={} stage_samples={}",
            backend, times.len(), writes.iter().filter(|w|w.end_ms.is_none()).count(),
            times.get(times.len()/2).copied().unwrap_or(0), times.last().copied().unwrap_or(0),
            stages.iter().filter(|(s,_)|*s=="file-sync").count(), stages.iter().filter(|(s,_)|*s=="directory-sync").count(), stages.len());
    }
}
#[derive(Debug)]
struct WalWrite {
    start_ms: u128,
    end_ms: Option<u128>,
    snapshot: String,
}
impl crate::usecase::ports::ConsensusPersistence for TimedWal {
    fn persist(&self, bytes: &[u8]) -> std::io::Result<()> {
        let snapshot =
            ConsensusState::phase_snapshot_diagnostics(bytes).map_err(std::io::Error::other)?;
        let started = self.start.elapsed().as_millis();
        {
            let mut writes = self.writes.lock().unwrap();
            if writes.len() == 128 {
                writes.pop_front();
            }
            writes.push_back(WalWrite {
                start_ms: started,
                end_ms: None,
                snapshot,
            });
        }
        let result = self.disk.persist_observed(bytes, |stage| {
            let mut stages = self.stages.lock().unwrap();
            if stages.len() == 1024 {
                stages.pop_front();
            }
            stages.push_back((stage, self.start.elapsed().as_millis()));
        });
        self.writes.lock().unwrap().back_mut().unwrap().end_ms =
            Some(self.start.elapsed().as_millis());
        result
    }
}

struct TestClock {
    start: Instant,
    wall: u64,
}
impl TestClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            wall: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
        }
    }
}
impl PolicyClock for TestClock {
    fn boot_id(&self) -> &str {
        "test-clock"
    }
    fn now_ns(&self) -> std::io::Result<u64> {
        Ok(self.start.elapsed().as_nanos() as u64 + 1)
    }
    fn lease_time(&self) -> std::io::Result<LeaseTime> {
        let boot_ns = self.now_ns()?;
        Ok(LeaseTime {
            boot_ns,
            unix_ns: self.wall + boot_ns,
            uncertainty_ns: 0,
            max_slow_rate_ppm: 0,
        })
    }
}
struct Tasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

#[tokio::test]
async fn automatic_driver_elects_and_commits_with_one_unreachable_member() -> anyhow::Result<()> {
    driver_fixture(false, false, false).await
}

#[tokio::test]
async fn automatic_driver_survives_high_terms_and_dead_leader_heartbeat_replay(
) -> anyhow::Result<()> {
    driver_fixture(true, false, false).await
}

#[tokio::test]
async fn automatic_driver_replaces_fresh_censoring_leader_and_preserves_request_deadline(
) -> anyhow::Result<()> {
    driver_fixture(true, true, false).await
}

#[tokio::test]
async fn automatic_driver_recovers_unprepared_split_with_faulty_leader_offline(
) -> anyhow::Result<()> {
    driver_fixture(false, false, true).await
}

// Do not wait for a state lock after a deadline: a busy worker is evidence too.
fn dump_fixture_states(nodes: &[MeshNode], stage: &str) {
    for node in nodes {
        match node.state.try_lock() {
            Ok(state) => eprintln!(
                "stage={stage} node={} {}",
                node.node_id,
                state.phase_diagnostics()
            ),
            Err(_) => eprintln!("stage={stage} node={} state=busy", node.node_id),
        }
    }
}

struct LiveFixture {
    clock: Arc<dyn PolicyClock>,
    bridge: Arc<dyn crate::usecase::EbpfBridge>,
}
async fn driver_fixture(attack: bool, censorship: bool, split: bool) -> anyhow::Result<()> {
    driver_fixture_with_backend(attack, censorship, split, None).await
}
async fn driver_fixture_with_backend(
    attack: bool,
    censorship: bool,
    split: bool,
    live: Option<LiveFixture>,
) -> anyhow::Result<()> {
    if std::env::var_os("VEIL_TEST_TRACE").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "veil_node=warn,veil_node::infra::daemon=debug,veil_node::usecase::consensus_engine=debug".into()))
            .try_init();
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root = directory();
    let pki = ClusterMtlsPki::generate()?;
    let identities: Vec<_> = ["a", "b", "c", "d"]
        .iter()
        .map(|id| NodeCryptoIdentity::generate_random(*id))
        .collect();
    let endpoints: Vec<_> = (0..4)
        .map(|_| pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap())
        .collect();
    let members: Vec<_> = identities
        .iter()
        .zip(&endpoints)
        .map(|(key, endpoint)| Member {
            node_id: key.node_id.clone(),
            address: endpoint.local_addr().unwrap(),
            public_key: key.public_key_bytes.as_slice().try_into().unwrap(),
        })
        .collect();
    let config = FixedConfiguration::new("driver-test", 1, members, vec![], 1000)
        .map_err(anyhow::Error::msg)?;
    let authority = crate::infra::node_tls_tests::Authority::new();
    let clock: Arc<dyn PolicyClock> = live
        .as_ref()
        .map(|l| l.clock.clone())
        .unwrap_or_else(|| Arc::new(TestClock::new()));
    let mut nodes = Vec::new();
    let mut credentials = Vec::new();
    let mut stops = Vec::new();
    let mut tasks = Tasks(vec![]);
    let mut states = Vec::new();
    let fixture_started = Instant::now();
    let mut wal_timings = BTreeMap::new();
    for identity in identities {
        let id = identity.node_id.clone();
        let mut st =
            ConsensusState::new(&id, vec![]).with_ebpf_bridge(Arc::new(MockEbpfBridge::default()));
        st.configure_fixed_membership(config.clone(), identity)
            .map_err(anyhow::Error::msg)?;
        if id == "b" {
            if let Some(live) = &live {
                st.ebpf_bridge = Some(live.bridge.clone());
            }
        }
        st.enable_two_phase().map_err(anyhow::Error::msg)?;
        st.configure_policy_clock(clock.clone())?;
        let store = Arc::new(TimedWal {
            disk: FixtureWal::open(&root.join(format!("{id}.wal")))?,
            start: fixture_started,
            writes: std::sync::Mutex::new(std::collections::VecDeque::new()),
            stages: std::sync::Mutex::new(std::collections::VecDeque::new()),
        });
        st.enable_persistence(store.clone())
            .map_err(anyhow::Error::msg)?;
        wal_timings.insert(id.clone(), store);
        states.push(st);
    }
    if split {
        crate::usecase::consensus_engine::two_phase_tests::split(&mut states);
        assert_ne!(states[1].entry_hash_at(1), states[2].entry_hash_at(1));
    }
    // Old faulty member is offline. The other three run ordinary randomized elections.
    for (i, (st, mut endpoint)) in states.into_iter().zip(endpoints).enumerate() {
        if i == 0 && !attack {
            endpoint.close(0u32.into(), b"offline fixture");
            continue;
        }
        let id = st.node_id.clone();
        let dir = authority.bundle(&root.join(format!("tls-{id}")), &config, &id);
        let tls = Arc::new(crate::infra::node_tls::NodeTls::load(&dir, &config, &id)?);
        tls.configure_test_endpoint(&mut endpoint);
        credentials.push(tls.clone());
        let state = Arc::new(Mutex::new(st));
        let node = MeshNode {
            node_id: id.clone(),
            endpoint: endpoint.clone(),
            state: state.clone(),
            peer_addrs: BTreeMap::new(),
        };
        let server_endpoint = node.endpoint.clone();
        let server_state = node.state.clone();
        let server_id = id.clone();
        let server_tls = tls.clone();
        tasks.0.push(tokio::spawn(async move {
            while let Some(incoming) = server_endpoint.accept().await {
                let st = server_state.clone();
                let id = server_id.clone();
                let tls = server_tls.clone();
                tokio::spawn(async move {
                    if let Ok(conn) = incoming.await {
                        let _ =
                            crate::infra::handle_authenticated_connection(conn, &id, st, tls).await;
                    }
                });
            }
        }));
        let server_stop = tasks.0.last().unwrap().abort_handle();
        let path = root.join(format!("{id}.sock"));
        let listener = local_control::bind(&path)?;
        let control_state = state.clone();
        tasks.0.push(tokio::spawn(async move {
            local_control::serve(listener, control_state).await.unwrap();
        }));
        let peers = config.peer_addrs(&id);
        tasks.0.push(tokio::spawn(async move {
            daemon::run(endpoint, state, peers, tls).await.unwrap();
        }));
        stops.push((server_stop, tasks.0.last().unwrap().abort_handle()));
        nodes.push(node);
    }
    let attacker = if attack && !censorship {
        let st = nodes[0].state.lock().await;
        let rpc = st.sign_rpc(crate::domain::ConsensusRpc::RequestVote {
            term: crate::domain::MAX_SAFE_TERM - 1,
            candidate_id: st.node_id.clone(),
            last_log_index: 0,
            last_log_term: 0,
        });
        let endpoint = nodes[0].endpoint.clone();
        let tls = credentials[0].clone();
        let peers = config.peer_addrs(&st.node_id);
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = attempts.clone();
        let task = tokio::spawn(async move {
            loop {
                for (id, addr) in &peers {
                    let _ = crate::infra::send_authenticated_rpc(
                        &endpoint,
                        *addr,
                        rpc.clone(),
                        &tls,
                        id,
                    )
                    .await;
                    count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        });
        let stop = task.abort_handle();
        tasks.0.push(task);
        Some((stop, attempts))
    } else {
        None
    };
    let leader = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            for (i, node) in nodes.iter().enumerate() {
                let st = node.state.lock().await;
                if st.role == crate::domain::Role::Leader && st.commit_index > 0 {
                    return i;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .inspect_err(|_| dump_fixture_states(&nodes, "fixture initial election and NoOp commit"))
    .context("fixture initial election and NoOp commit")?;
    let ip = if live.is_some() {
        Ipv4Addr::new(10, 99, 0, 66)
    } else {
        Ipv4Addr::new(192, 0, 2, 44)
    };
    let ttl = if live.is_some() { 8 } else { 30 };
    let receipt = local_control::submit_ban(
        &root.join(format!("{}.sock", nodes[leader].node_id)),
        ip,
        ttl,
    )
    .await
    .inspect_err(|_| dump_fixture_states(&nodes, "fixture initial BAN submission"))
    .context("fixture initial BAN submission")?;
    assert!(receipt.starts_with("ACCEPTED "));
    let index = nodes[leader].state.lock().await.last_log_index();
    let hash = nodes[leader]
        .state
        .lock()
        .await
        .entry_hash_at(index)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut done = true;
            for node in &nodes {
                let st = node.state.lock().await;
                done &= st.last_applied >= index && st.entry_hash_at(index) == Some(hash);
            }
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .inspect_err(|_| dump_fixture_states(&nodes, "fixture initial BAN application on all nodes"))
    .context("fixture initial BAN application on all nodes")?;
    let other = (leader + 1) % nodes.len();
    assert!(local_control::submit_ban(
        &root.join(format!("{}.sock", nodes[other].node_id)),
        ip,
        30
    )
    .await
    .is_err());
    let leader_status = local_control::status(
        &root.join(format!("{}.sock", nodes[leader].node_id)),
        Some(index),
        Some(ip),
    )
    .await
    .inspect_err(|_| dump_fixture_states(&nodes, "fixture initial BAN status"))
    .context("fixture initial BAN status")?;
    assert!(leader_status.contains("entry_phase=applied\n"));
    assert!(leader_status.contains("policy_mirror=Drop\n"));
    #[cfg(target_os = "linux")]
    if live.is_some() {
        anyhow::ensure!(
            !fixture_ping()?,
            "Committed BAN did not block real fixture packets"
        );
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if fixture_ping()? {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await??;
        for node in &nodes {
            assert_eq!(
                node.state.lock().await.check_xdp_packet(ip),
                veil_common::XdpVerdict::Pass
            );
        }
        println!("PASS: divergent unprepared prefix -> automatic election -> node TLS/QUIC -> Commit QC -> actual XDP DROP -> signed-deadline PASS");
    }
    if censorship {
        let phase_started = Instant::now();
        if std::env::var_os("VEIL_TEST_TRACE").is_some() {
            let observed: Vec<_> = nodes.iter().map(|n| n.state.clone()).collect();
            tasks.0.push(tokio::spawn(async move {
                loop {
                    for state in &observed {
                        let s = state.lock().await;
                        eprintln!(
                            "censor sample ms={} node={} {}",
                            phase_started.elapsed().as_millis(),
                            s.node_id,
                            s.phase_diagnostics()
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            }));
        }
        let old_term = nodes[leader].state.lock().await.current_term;
        nodes[leader].state.lock().await.censor_ban_requests = true;
        let initial_activity = nodes[other].state.lock().await.election_activity();
        let request_ip = Ipv4Addr::new(192, 0, 2, 72);
        let receipt = local_control::submit_ban_request(
            &root.join(format!("{}.sock", nodes[other].node_id)),
            request_ip,
            25,
        )
        .await
        .inspect_err(|_| dump_fixture_states(&nodes, "censorship request acceptance"))
        .context("censorship request acceptance")?;
        let id = receipt
            .split_whitespace()
            .nth(1)
            .unwrap()
            .strip_prefix("id=")
            .unwrap()
            .to_owned();
        let initial_status = local_control::request_status(
            &root.join(format!("{}.sock", nodes[other].node_id)),
            &id,
        )
        .await
        .inspect_err(|_| dump_fixture_states(&nodes, "censorship initial request status"))
        .context("censorship initial request status")?;
        let deadline = initial_status
            .lines()
            .find(|line| line.starts_with("not_after_unix_ns="))
            .unwrap()
            .to_owned();
        // The faulty leader continues authenticated fresh challenge responses and
        // unrelated commits; its only fault is omitting witnessed BAN proposals.
        let mut saw_fresh_old_leader_progress = false;
        for _ in 0..5 {
            let proposal = super::state_worker::run(nodes[leader].state.clone(), |st| {
                st.propose(crate::domain::MeshCommand::NoOp)
            })
            .await?
            .map_err(anyhow::Error::msg)?;
            if std::env::var_os("VEIL_TEST_TRACE").is_some() {
                eprintln!(
                    "censor NoOp index={} start_ms={}",
                    proposal.index,
                    phase_started.elapsed().as_millis()
                );
            }
            let wait_started = fixture_started.elapsed().as_millis();
            let committed = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    // Observe the adversary's fresh responses and unrelated
                    // progress while it still owns this term. Do not demand
                    // that the valid censorship timer remain idle afterward.
                    if let Ok(witness) = nodes[other].state.try_lock() {
                        saw_fresh_old_leader_progress |= witness.current_term == old_term
                            && witness.election_activity() > initial_activity
                            && witness.commit_index > index;
                    }
                    if nodes[leader].state.lock().await.commit_index >= proposal.index {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            })
            .await;
            if committed.is_err() {
                dump_fixture_states(&nodes, "unrelated NoOp commit");
                eprintln!(
                    "NoOp {} leader={} wait_ms={}..{}",
                    proposal.index,
                    nodes[leader].node_id,
                    wait_started,
                    fixture_started.elapsed().as_millis()
                );
                for (id, store) in &wal_timings {
                    let writes = store.writes.lock().unwrap();
                    let recent: Vec<_> = writes
                        .iter()
                        .filter(|write| {
                            write.end_ms.unwrap_or(u128::MAX) >= wait_started
                                || write.start_ms >= wait_started
                        })
                        .collect();
                    for write in recent {
                        eprintln!(
                            "WAL node={id} start_ms={} end_ms={:?} {}",
                            write.start_ms, write.end_ms, write.snapshot
                        );
                    }
                    let stages = store.stages.lock().unwrap();
                    let stages: Vec<_> = stages
                        .iter()
                        .filter(|(_, at)| *at >= wait_started.saturating_sub(1000))
                        .collect();
                    eprintln!("WAL node={id} stages_ms={stages:?}");
                }
            }
            committed
                .map_err(|e| anyhow::anyhow!("unrelated NoOp {} commit: {e}", proposal.index))?;
        }
        assert!(
            saw_fresh_old_leader_progress,
            "fresh acks and unrelated commits must be observed in the censored term"
        );
        {
            let witness = nodes[other].state.lock().await;
            let request_id: [u8; 32] =
                std::array::from_fn(|i| u8::from_str_radix(&id[i * 2..i * 2 + 2], 16).unwrap());
            let status = witness.ban_request_status(&request_id);
            if witness.current_term == old_term {
                assert!(status.contains("request_phase=queued"));
            } else {
                assert!(witness.current_term > old_term);
                assert!(
                    witness.certified_handoff_for(&request_id, old_term, &nodes[leader].node_id),
                    "early handoff must have a verified quorum proof for this request/leader/term"
                );
                assert!(
                    status.contains("request_phase=queued")
                        || status.contains("request_phase=applied")
                );
            }
        }
        let replacement = tokio::time::timeout(Duration::from_secs(18), async {
            loop {
                for (i, node) in nodes.iter().enumerate() {
                    if i == leader {
                        continue;
                    }
                    let st = node.state.lock().await;
                    if st.role == crate::domain::Role::Leader
                        && st.current_term > old_term
                        && st.check_xdp_packet(request_ip) == veil_common::XdpVerdict::Drop
                    {
                        return i;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        if replacement.is_err() {
            for node in &nodes {
                let st = node.state.lock().await;
                eprintln!("censorship handoff timeout: {:?} leader={:?} vote={:?} last_log={} persistence={:?}",
                    *st, st.leader_id, st.voted_for, st.last_log_index(), st.persistence_error);
            }
        }
        let replacement = replacement.map_err(|e| anyhow::anyhow!("censorship handoff: {e}"))?;

        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let mut done = true;
                for (i, node) in nodes.iter().enumerate() {
                    if i == leader {
                        continue;
                    }
                    done &= node.state.lock().await.check_xdp_packet(request_ip)
                        == veil_common::XdpVerdict::Drop;
                }
                if done {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .inspect_err(|_| {
            dump_fixture_states(
                &nodes,
                "censorship replacement BAN application on all honest nodes",
            )
        })
        .context("censorship replacement BAN application on all honest nodes")?;
        let final_status = local_control::request_status(
            &root.join(format!("{}.sock", nodes[other].node_id)),
            &id,
        )
        .await
        .inspect_err(|_| dump_fixture_states(&nodes, "censorship final request status"))
        .context("censorship final request status")?;
        assert!(final_status.contains("request_phase=applied"));
        assert!(
            final_status.lines().any(|line| line == deadline),
            "handoff cannot restamp the request deadline"
        );
        let new_term = nodes[replacement].state.lock().await.current_term;
        assert_eq!(nodes[leader].state.lock().await.current_term, old_term);
        assert_eq!(
            nodes[leader].state.lock().await.role,
            crate::domain::Role::Leader
        );
        // Advance only the test backend's policy clock naturally, never host time.
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let mut done = true;
                for node in &nodes {
                    done &= super::state_worker::run(node.state.clone(), move |st| {
                        st.retry_pending_apply();
                        st.expire_stale_policies();
                        st.check_xdp_packet(request_ip) == veil_common::XdpVerdict::Pass
                    })
                    .await
                    .expect("expiry worker");
                }
                if done {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .inspect_err(|_| dump_fixture_states(&nodes, "censorship signed deadline expiry"))
        .context("censorship signed deadline expiry")?;
        assert!(local_control::request_status(
            &root.join(format!("{}.sock", nodes[other].node_id)),
            &id
        )
        .await?
        .contains("request_phase=expired"));
        println!("authenticated censorship: fresh acks and unrelated commits observed; leader {} -> {}; term {} -> {}; immutable deadline {}; applied on three honest nodes and expired; faulty leader ignored handoff", nodes[leader].node_id, nodes[replacement].node_id, old_term, new_term, deadline);
    }
    if let Some((attack_stop, attempts)) = attacker {
        attack_stop.abort();
        assert!(attempts.load(std::sync::atomic::Ordering::Relaxed) > 0);
        for node in &nodes {
            assert!(node.state.lock().await.current_term < 20);
        }
        // Save real signed heartbeats before the leader's server/driver stop.
        let old_term = nodes[leader].state.lock().await.current_term;
        let mut replay = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            if i == leader {
                continue;
            }
            let st = nodes[leader].state.lock().await;
            let rpc = st.make_append_entries(&node.node_id).unwrap();
            replay.push((node.node_id.clone(), node.local_addr()?, st.sign_rpc(rpc)));
        }
        stops[leader].0.abort();
        stops[leader].1.abort();
        nodes[leader].endpoint.close(0u32.into(), b"leader stopped");
        // A client possessing the former leader's leaf can replay its signed bytes,
        // but no server remains to answer new follower-generated challenges.
        let mut endpoint = pki.make_endpoint("127.0.0.1:0".parse()?)?;
        let tls = credentials[leader].clone();
        tls.configure_test_endpoint(&mut endpoint);
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let replay_accepted = accepted.clone();
        let replay_endpoint = endpoint.clone();
        let replay_task = tokio::spawn(async move {
            loop {
                for (id, addr, rpc) in &replay {
                    if let Ok(Some(reply)) = crate::infra::send_authenticated_rpc(
                        &replay_endpoint,
                        *addr,
                        rpc.clone(),
                        &tls,
                        id,
                    )
                    .await
                    {
                        let rpc = match reply.rpc {
                            crate::domain::ConsensusRpc::TermCertified { rpc_bytes, .. } => {
                                bincode::deserialize(&rpc_bytes).unwrap()
                            }
                            rpc => rpc,
                        };
                        if matches!(rpc, crate::domain::ConsensusRpc::TwoPhase { message, .. }
                            if matches!(*message, crate::domain::PhaseMessage::Receipt { .. }))
                        {
                            replay_accepted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        });
        tasks.0.push(replay_task);
        let new_leader = tokio::time::timeout(Duration::from_secs(18), async {
            loop {
                for (i, node) in nodes.iter().enumerate() {
                    if i == leader {
                        continue;
                    }
                    let st = node.state.lock().await;
                    if st.role == crate::domain::Role::Leader
                        && st.current_term > old_term
                        && st.commit_index > index
                    {
                        return i;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        if new_leader.is_err() {
            for node in &nodes {
                eprintln!("reelection timeout: {:?}", node.state.lock().await);
            }
        }
        let new_leader = new_leader?;
        assert!(
            accepted.load(std::sync::atomic::Ordering::Relaxed) > 0,
            "replayed heartbeats must actually reach acceptance before the next election"
        );
        let st = nodes[new_leader].state.lock().await;
        println!("authenticated attack: high-term attempts={}, accepted stale heartbeats={}, reelection {} -> {}, committed={}",
            attempts.load(std::sync::atomic::Ordering::Relaxed), accepted.load(std::sync::atomic::Ordering::Relaxed), old_term, st.current_term, st.commit_index);
        endpoint.close(0u32.into(), b"replay complete");
    }
    if split {
        let first = nodes[0].state.lock().await.entry_hash_at(1);
        for node in &nodes {
            let st = node.state.lock().await;
            assert_eq!(st.entry_hash_at(1), first);
            assert!(st.current_term > 1);
        }
    }
    for node in &nodes {
        node.endpoint.close(0u32.into(), b"test complete");
    }
    if std::env::var_os("VEIL_TEST_WAL_STATS").is_some() {
        for (id, store) in &wal_timings {
            let stages: Vec<_> = store.stages.lock().unwrap().iter().copied().collect();
            let slow: Vec<_> = stages
                .windows(2)
                .filter(|pair| pair[0].0 != "done" && pair[1].1.saturating_sub(pair[0].1) >= 100)
                .map(|pair| (pair[0].0, pair[0].1, pair[1].1 - pair[0].1))
                .collect();
            eprintln!("WAL completed fixture censorship={censorship} node={id} slow_stage/start_ms/duration_ms={slow:?}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn local_control_rejects_bad_bans_and_slow_reader_does_not_block_bypass() -> anyhow::Result<()>
{
    let root = directory();
    let mut st =
        ConsensusState::new("solo", vec![]).with_ebpf_bridge(Arc::new(MockEbpfBridge::default()));
    st.start_signed_election().unwrap();
    let state = Arc::new(Mutex::new(st));
    let path = root.join("node.sock");
    let listener = local_control::bind(&path)?;
    let task_state = state.clone();
    let _tasks = Tasks(vec![tokio::spawn(async move {
        local_control::serve(listener, task_state).await.unwrap();
    })]);
    let mut stalled = tokio::net::UnixStream::connect(&path).await?;
    stalled.write_all(b"B").await?;
    tokio::time::timeout(Duration::from_secs(1), local_control::request(&path, true)).await??;
    assert!(state.lock().await.emergency_local_bypass);
    let ip = Ipv4Addr::new(192, 0, 2, 1);
    for (ip, ttl) in [(ip, 0), (ip, 86401), (Ipv4Addr::LOCALHOST, 30)] {
        assert!(local_control::submit_ban(&path, ip, ttl).await.is_err());
    }
    assert_eq!(state.lock().await.last_log_index(), 0);
    Ok(())
}

#[test]
fn concurrent_pki_startup_shares_one_pair_and_partial_state_is_preserved() {
    let root = directory();
    let dir = root.join("pki");
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                ClusterMtlsPki::load_or_generate_dir(&dir).unwrap();
                std::fs::read(dir.join("ca.cert.der")).unwrap()
            })
        })
        .collect();
    let certs: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(certs.iter().all(|c| c == &certs[0]));
    let partial = root.join("partial");
    std::fs::create_dir(&partial).unwrap();
    std::fs::write(partial.join("ca.cert.der"), b"incomplete fixture").unwrap();
    assert!(ClusterMtlsPki::load_or_generate_dir(&partial).is_err());
    assert_eq!(
        std::fs::read(partial.join("ca.cert.der")).unwrap(),
        b"incomplete fixture"
    );
    assert!(!partial.join("ca.key.der").exists());
}

#[cfg(target_os = "linux")]
fn fixture_ping() -> anyhow::Result<bool> {
    let output = std::process::Command::new("ip")
        .args([
            "netns",
            "exec",
            "ns-veil-rogue",
            "ping",
            "-n",
            "-c",
            "1",
            "-W",
            "1",
            "10.99.0.1",
        ])
        .output()?;
    anyhow::ensure!(
        matches!(output.status.code(), Some(0 | 1)),
        "isolated ping setup failed"
    );
    Ok(output.status.success())
}
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "root, unused existing veth-victim/ns-veil-rogue only; actual split recovery + XDP"]
async fn linux_two_phase_split_recovery_applies_and_expires_in_real_xdp() -> anyhow::Result<()> {
    use crate::{ebpf_bridge::AyaKernelBridge, infra::policy_clock::LinuxBootClock};
    use std::{path::Path, process::Command};
    anyhow::ensure!(unsafe { libc::geteuid() } == 0, "requires root");
    let before = Command::new("ip")
        .args(["-details", "link", "show", "veth-victim"])
        .output()?;
    anyhow::ensure!(
        before.status.success() && !String::from_utf8_lossy(&before.stdout).contains("prog/xdp"),
        "fixture missing or already attached"
    );
    anyhow::ensure!(fixture_ping()?, "baseline ping failed");
    let clock = Arc::new(LinuxBootClock::new()?);
    let sample = clock.lease_time()?;
    println!(
        "clock_uncertainty_ns={} assumed_max_slow_rate_ppm={}",
        sample.uncertainty_ns, sample.max_slow_rate_ppm
    );
    let object = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("assets/veil_xdp_kern.o");
    let bridge = Arc::new(AyaKernelBridge::load_and_attach(
        object.to_str().unwrap(),
        "veth-victim",
    )?);
    driver_fixture_with_backend(
        false,
        false,
        true,
        Some(LiveFixture {
            clock,
            bridge: bridge.clone(),
        }),
    )
    .await?;
    drop(bridge);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = Command::new("ip")
        .args(["-details", "link", "show", "veth-victim"])
        .output()?;
    anyhow::ensure!(
        after.status.success() && !String::from_utf8_lossy(&after.stdout).contains("prog/xdp"),
        "XDP attachment remained"
    );
    anyhow::ensure!(fixture_ping()?, "cleanup ping failed");
    println!("PASS: cleanup has no XDP attachment and real fixture ping succeeds");
    Ok(())
}
