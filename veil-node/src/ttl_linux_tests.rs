//! Explicit opt-in integration test. Uses only the existing isolated veth fixture.
//! No interface/namespace creation or deletion and no attachment to physical NICs.
use crate::{
    domain::MeshCommand,
    ebpf_bridge::AyaKernelBridge,
    infra::{local_control, policy_clock::LinuxBootClock, wal::FileWal},
    usecase::{ConsensusState, EbpfBridge, PolicyClock},
};
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::Arc,
    time::Duration,
};

const IFACE: &str = "veth-victim";
const NS: &str = "ns-veil-rogue";
const SOURCE: Ipv4Addr = Ipv4Addr::new(10, 99, 0, 66);

async fn quic_deliver(nodes: &[crate::infra::MeshNode], to: usize) -> anyhow::Result<()> {
    let signed = {
        let mut leader = nodes[0].state.lock().await;
        let rpc = leader.make_append_entries(&nodes[to].node_id).unwrap();
        leader.sign_rpc_durable(rpc).map_err(anyhow::Error::msg)?
    };
    let response =
        crate::infra::send_rpc_over_quic(&nodes[0].endpoint, nodes[to].local_addr()?, signed)
            .await?
            .ok_or_else(|| anyhow::anyhow!("No signed RPC response"))?;
    nodes[0]
        .state
        .lock()
        .await
        .handle_signed_rpc(response)
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

async fn wait_until_after(not_after_unix_ns: u64) -> anyhow::Result<()> {
    let now = LinuxBootClock::new()?.lease_time()?.unix_ns;
    let remaining = not_after_unix_ns.saturating_sub(now);
    anyhow::ensure!(remaining < 10_000_000_000, "unexpectedly long test lease");
    tokio::time::sleep(Duration::from_nanos(remaining) + Duration::from_millis(100)).await;
    anyhow::ensure!(
        LinuxBootClock::new()?.lease_time()?.unix_ns >= not_after_unix_ns,
        "clock did not pass signed deadline"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires root and unused isolated veth fixture; explicit signed catch-up test"]
async fn linux_delayed_catch_up_signed_deadline() -> anyhow::Result<()> {
    use crate::{
        domain::MeshCommand,
        infra::{send_rpc_over_quic, ClusterMtlsPki, MeshNode},
    };
    use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt};
    anyhow::ensure!(unsafe { libc::geteuid() } == 0, "requires root");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let before = Command::new("ip")
        .args(["-details", "link", "show", IFACE])
        .output()?;
    anyhow::ensure!(
        before.status.success() && !String::from_utf8_lossy(&before.stdout).contains("prog/xdp"),
        "fixture absent or already attached"
    );
    anyhow::ensure!(ping()?, "baseline ping failed");
    let clock = Arc::new(LinuxBootClock::new()?);
    let sample = clock.lease_time()?;
    println!(
        "clock_uncertainty_ns={} assumed_max_slow_rate_ppm={}",
        sample.uncertainty_ns, sample.max_slow_rate_ppm
    );
    let root = std::env::temp_dir().join(format!(
        "veil-signed-catchup-{}-{}",
        std::process::id(),
        clock.now_ns()?
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    println!("signed_catchup_evidence={}", root.display());
    let object = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("assets/veil_xdp_kern.o");
    let bridge = Arc::new(AyaKernelBridge::load_and_attach(
        object.to_str().unwrap(),
        IFACE,
    )?);
    let names = ["leader", "peer-b", "peer-c", "late"];
    let mut states: Vec<_> = names
        .iter()
        .map(|&id| {
            ConsensusState::new(
                id,
                names
                    .iter()
                    .filter(|&&p| p != id)
                    .map(|p| p.to_string())
                    .collect(),
            )
        })
        .collect();
    let pki = ClusterMtlsPki::load_or_generate_dir(&root.join("pki"))?;
    let endpoints: Vec<_> = (0..states.len())
        .map(|_| pki.make_endpoint("127.0.0.1:0".parse().unwrap()))
        .collect::<anyhow::Result<_>>()?;
    let members = states
        .iter()
        .zip(&endpoints)
        .map(|(s, e)| crate::domain::Member {
            node_id: s.node_id.clone(),
            address: e.local_addr().unwrap(),
            public_key: s.identity().public_key_bytes.as_slice().try_into().unwrap(),
        })
        .collect();
    let config =
        crate::domain::FixedConfiguration::new("linux-deadline", 1, members, vec![], 1_000)
            .map_err(anyhow::Error::msg)?;
    let mut nodes = Vec::new();
    for (mut state, endpoint) in states.drain(..).zip(endpoints) {
        state
            .configure_fixed_membership(config.clone(), state.identity().clone())
            .map_err(anyhow::Error::msg)?;
        state.configure_policy_clock(clock.clone())?;
        if state.node_id == "late" {
            state.ebpf_bridge = Some(bridge.clone());
        }
        state
            .enable_persistence(Arc::new(FileWal::open(
                &root.join(format!("{}.wal", state.node_id)),
            )?))
            .map_err(anyhow::Error::msg)?;
        nodes.push(MeshNode {
            node_id: state.node_id.clone(),
            endpoint,
            state: Arc::new(tokio::sync::Mutex::new(state)),
            peer_addrs: BTreeMap::new(),
        });
    }
    // Guard cancellation also applies when an assertion returns early.
    struct Servers(Vec<tokio::task::JoinHandle<()>>);
    impl Drop for Servers {
        fn drop(&mut self) {
            for task in &self.0 {
                task.abort();
            }
        }
    }
    let servers = Servers(nodes.iter().map(MeshNode::spawn_server).collect());
    let election = nodes[0]
        .state
        .lock()
        .await
        .start_signed_election()
        .map_err(anyhow::Error::msg)?;
    for node in nodes.iter().skip(1) {
        let reply = send_rpc_over_quic(&nodes[0].endpoint, node.local_addr()?, election.clone())
            .await?
            .unwrap();
        nodes[0]
            .state
            .lock()
            .await
            .handle_signed_rpc(reply)
            .map_err(anyhow::Error::msg)?;
    }
    for (phase, ttl) in [("expired", 3), ("fresh", 5)] {
        let entry = nodes[0]
            .state
            .lock()
            .await
            .propose(MeshCommand::set_trust_with_ttl(SOURCE, false, ttl))
            .map_err(anyhow::Error::msg)?;
        let MeshCommand::SetPeerTrustLeaseBatch {
            issued_at_unix_ns,
            rules,
        } = &entry.command
        else {
            anyhow::bail!("unsigned relative TTL entered log");
        };
        let not_after = rules[0].not_after_unix_ns;
        for i in [1, 2, 1, 2] {
            quic_deliver(&nodes, i).await?;
        }
        anyhow::ensure!(
            nodes[0].state.lock().await.commit_index == entry.index,
            "quorum not committed"
        );
        if phase == "expired" {
            anyhow::ensure!(
                nodes[3].state.lock().await.log.is_empty(),
                "late node already received entry"
            );
            wait_until_after(not_after).await?;
        }
        // Deliver a real mTLS QUIC AppendEntries with its original signatures/QC.
        quic_deliver(&nodes, 3).await?;
        let late = nodes[3].state.lock().await;
        anyhow::ensure!(
            late.commit_index == entry.index && late.last_applied == entry.index,
            "catch-up stalled"
        );
        anyhow::ensure!(
            late.entry_hash_at(entry.index)
                == nodes[0].state.lock().await.entry_hash_at(entry.index),
            "log hashes diverged"
        );
        drop(late);
        if phase == "expired" {
            anyhow::ensure!(ping()?, "expired catch-up reissued BAN");
            let (_, drops) = bridge.read_xdp_counters().unwrap();
            anyhow::ensure!(drops == 0, "expired decision caused kernel drops");
            quic_deliver(&nodes, 3).await?;
            anyhow::ensure!(ping()?, "heartbeat rearmed expired BAN");
        } else {
            anyhow::ensure!(!ping()?, "fresh deadline failed to enforce BAN");
            wait_until_after(not_after).await?;
            anyhow::ensure!(ping()?, "fresh BAN outlived signed deadline");
            let (passes, drops) = bridge.read_xdp_counters().unwrap();
            anyhow::ensure!(passes > 0 && drops > 0, "missing real XDP counter evidence");
            println!("kernel_pass={passes} kernel_drop={drops}");
        }
        let record = format!("phase={phase} index={} issued_at_unix_ns={issued_at_unix_ns} not_after_unix_ns={not_after} result=PASS\n", entry.index);
        std::fs::write(root.join(format!("{phase}.result")), &record)?;
        print!("{record}");
    }
    for node in &nodes {
        node.endpoint.close(0u32.into(), b"test complete");
    }
    drop(servers);
    for node in &nodes {
        node.endpoint.wait_idle().await;
    }
    drop(nodes);
    drop(bridge);
    // Connection tasks are runtime-owned; let them observe endpoint closure.
    tokio::task::yield_now().await;
    let after = Command::new("ip")
        .args(["-details", "link", "show", IFACE])
        .output()?;
    anyhow::ensure!(
        after.status.success() && !String::from_utf8_lossy(&after.stdout).contains("prog/xdp"),
        "XDP attachment remained"
    );
    anyhow::ensure!(ping()?, "final recovery ping failed");
    println!("signed_catchup_cleanup: no XDP attachment; ping PASS");
    Ok(())
}

#[tokio::test]
#[ignore = "child fixture for linux_live_ttl_bypass_recovery"]
async fn linux_lease_child() -> anyhow::Result<()> {
    let root = PathBuf::from(std::env::var_os("VEIL_LEASE_ROOT").unwrap());
    let phase = std::env::var("VEIL_LEASE_PHASE")?;
    let object = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("assets/veil_xdp_kern.o");
    let bridge = Arc::new(AyaKernelBridge::load_and_attach(
        object.to_str().unwrap(),
        IFACE,
    )?);
    let mut s = ConsensusState::new("lease-test", vec![]).with_ebpf_bridge(bridge.clone());
    let identity = crate::adapter::NodeCryptoIdentity::load_or_generate_file(
        "lease-test",
        &root.join("signing.pk8"),
    )?;
    let member = crate::domain::Member {
        node_id: "lease-test".into(),
        address: std::fs::read_to_string(root.join("endpoint"))?.parse()?,
        public_key: identity.public_key_bytes.as_slice().try_into().unwrap(),
    };
    let config = crate::domain::FixedConfiguration::new(
        "linux-lease-fixture",
        1,
        vec![member],
        vec![],
        1_000,
    )
    .map_err(anyhow::Error::msg)?;
    s.configure_fixed_membership(config, identity)
        .map_err(anyhow::Error::msg)?;
    // Legacy unapproved fixture: normal daemon must refuse this profile.
    s.enable_two_phase().map_err(anyhow::Error::msg)?;
    s.configure_policy_clock(Arc::new(LinuxBootClock::new()?))?;
    let wal = root.join("node.wal");
    let store = Arc::new(FileWal::open(&wal)?);
    if wal.exists() {
        s.load_wal(&wal)?;
    }
    s.enable_persistence(store).map_err(anyhow::Error::msg)?;
    if phase == "ttl" || phase == "bypass" {
        s.start_signed_election().map_err(anyhow::Error::msg)?;
        let cmd = if phase == "ttl" {
            MeshCommand::set_trust_with_ttl(SOURCE, false, 5)
        } else {
            MeshCommand::set_trust_with_ttl(SOURCE, false, 300)
        };
        s.propose(cmd).map_err(anyhow::Error::msg)?;
    }
    anyhow::ensure!(
        s.last_applied == s.commit_index && s.last_apply_error.is_none(),
        "fixture enforcement not applied"
    );
    if phase == "ttl" {
        std::fs::write(
            root.join("deadline"),
            s.peer_rule_deadlines_ns[&u32::from(SOURCE)].to_string(),
        )?;
    }
    let socket = root.join(format!("{phase}.sock"));
    let listener = local_control::bind(&socket)?;
    let state = Arc::new(tokio::sync::Mutex::new(s));
    tokio::spawn(local_control::serve(listener, state.clone()));
    std::fs::write(root.join(format!("{phase}.ready")), "ready")?;
    // Deliberately no expiry/GC loop. The packet-path deadline must work alone.
    loop {
        if root.join(format!("{phase}.report")).exists() {
            let (pass, drop) = bridge.read_xdp_counters().unwrap();
            std::fs::write(
                root.join(format!("{phase}.counters")),
                format!("{pass} {drop}"),
            )?;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn launch(root: &Path, phase: &str) -> anyhow::Result<OwnedChild> {
    let child = Command::new(std::env::current_exe()?)
        .args([
            "--ignored",
            "--exact",
            "ttl_linux_tests::linux_lease_child",
            "--nocapture",
        ])
        .env("VEIL_LEASE_ROOT", root)
        .env("VEIL_LEASE_PHASE", phase)
        .stdout(std::fs::File::create(root.join(format!("{phase}.stdout")))?)
        .stderr(std::fs::File::create(root.join(format!("{phase}.stderr")))?)
        .spawn()?;
    let mut child = OwnedChild(child);
    for _ in 0..200 {
        if root.join(format!("{phase}.ready")).exists() {
            return Ok(child);
        }
        anyhow::ensure!(
            child.0.try_wait()?.is_none(),
            "child {phase} exited: inspect {}",
            root.display()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    anyhow::bail!("child {phase} readiness timeout")
}
fn ping() -> anyhow::Result<bool> {
    let output = Command::new("ip")
        .args([
            "netns",
            "exec",
            NS,
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
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "ping setup error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.status.success())
}
async fn counters(root: &Path, phase: &str) -> anyhow::Result<(u64, u64)> {
    std::fs::write(root.join(format!("{phase}.report")), "report")?;
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(root.join(format!("{phase}.counters"))) {
            let values: Vec<_> = text
                .split_whitespace()
                .filter_map(|s| s.parse::<u64>().ok())
                .collect();
            if values.len() == 2 {
                return Ok((values[0], values[1]));
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    anyhow::bail!("counter report timeout")
}

#[tokio::test]
#[ignore = "requires root, unused veth-victim and ns-veil-rogue; run explicitly"]
async fn linux_live_ttl_bypass_recovery() -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    anyhow::ensure!(unsafe { libc::geteuid() } == 0, "requires root");
    let before = Command::new("ip")
        .args(["-details", "link", "show", IFACE])
        .output()?;
    anyhow::ensure!(
        before.status.success() && !String::from_utf8_lossy(&before.stdout).contains("prog/xdp"),
        "fixture absent or XDP already attached"
    );
    anyhow::ensure!(ping()?, "baseline ping must succeed");
    let root = std::env::temp_dir().join(format!(
        "veil-live-lease-{}-{}",
        std::process::id(),
        LinuxBootClock::new()?.now_ns()?
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let reserved = std::net::UdpSocket::bind("127.0.0.1:0")?;
    std::fs::write(root.join("endpoint"), reserved.local_addr()?.to_string())?;
    drop(reserved);
    println!("evidence_directory={}", root.display());
    {
        let child = launch(&root, "ttl").await?;
        // Kernel rejects a second owner; testing this must not replace the live program.
        let object = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("assets/veil_xdp_kern.o");
        assert!(AyaKernelBridge::load_and_attach(object.to_str().unwrap(), IFACE).is_err());
        anyhow::ensure!(!ping()?, "active TTL BAN must drop");
        anyhow::ensure!(
            unsafe { libc::kill(child.0.id() as i32, libc::SIGSTOP) } == 0,
            "SIGSTOP failed"
        );
        // Signal delivery is asynchronous; observe the stopped state before probing.
        let mut stopped = false;
        for _ in 0..100 {
            let status = std::fs::read_to_string(format!("/proc/{}/status", child.0.id()))?;
            stopped = status
                .lines()
                .any(|l| l.starts_with("State:") && l.contains('T'));
            if stopped {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::ensure!(stopped, "child did not enter stopped state");
        let deadline: u64 = std::fs::read_to_string(root.join("deadline"))?.parse()?;
        let remaining = deadline.saturating_sub(LinuxBootClock::new()?.now_ns()?);
        tokio::time::sleep(Duration::from_nanos(remaining) + Duration::from_millis(100)).await;
        anyhow::ensure!(
            ping()?,
            "kernel must pass expired BAN while userspace is SIGSTOPped"
        );
        anyhow::ensure!(
            unsafe { libc::kill(child.0.id() as i32, libc::SIGCONT) } == 0,
            "SIGCONT failed"
        );
        let (pass, drop) = counters(&root, "ttl").await?;
        anyhow::ensure!(pass > 0 && drop > 0, "real XDP PASS/DROP counters required");
        println!("ttl_sigstop: PASS; kernel_pass={pass} kernel_drop={drop}");
    }
    anyhow::ensure!(ping()?, "process death must release FD-owned XDP link");
    {
        let _child = launch(&root, "expired-recovery").await?;
        anyhow::ensure!(ping()?, "expired WAL lease resurrected");
        let (_, drop) = counters(&root, "expired-recovery").await?;
        anyhow::ensure!(drop == 0, "recovery must not transiently drop traffic");
        println!("expired_wal_recovery: PASS");
    }
    {
        let _child = launch(&root, "bypass").await?;
        anyhow::ensure!(!ping()?, "finite BAN must initially drop");
        local_control::request(&root.join("bypass.sock"), true).await?;
        anyhow::ensure!(ping()?, "acknowledged emergency bypass must pass");
        println!("live_socket_bypass: PASS");
    }
    {
        let _child = launch(&root, "bypass-recovery").await?;
        anyhow::ensure!(ping()?, "durable bypass must survive restart");
        local_control::request(&root.join("bypass-recovery.sock"), false).await?;
        anyhow::ensure!(
            !ping()?,
            "explicit bypass off must enforce retained finite BAN"
        );
        local_control::request(&root.join("bypass-recovery.sock"), true).await?;
        anyhow::ensure!(ping()?, "bypass must be reversible");
        println!("bypass_recovery_and_explicit_off: PASS");
    }
    {
        // Also exercise the actual daemon composition root and clap client, not
        // just the state-machine fixture. This test requires cargo build first.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let binary = std::env::var_os("VEIL_NODE_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo.join("target/debug/veil-node"));
        anyhow::ensure!(
            binary.is_file(),
            "build veil-node before live integration test"
        );
        let socket = root.join("daemon.sock");
        let identity = crate::adapter::NodeCryptoIdentity::load_existing_file(
            "lease-test",
            &root.join("signing.pk8"),
        )?;
        let public_key: String = identity
            .public_key_bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let endpoint = std::fs::read_to_string(root.join("endpoint"))?;
        let member = format!("lease-test@{endpoint}={public_key}");
        let outsider = crate::adapter::NodeCryptoIdentity::generate_random("outsider");
        std::fs::create_dir_all(root.join("daemon-cluster/peers"))?;
        std::fs::write(
            root.join("daemon-cluster/peers/outsider.pub"),
            &outsider.public_key_bytes,
        )?;
        let child = Command::new(&binary)
            .args([
                "--cluster-id",
                "linux-lease-fixture",
                "--config-epoch",
                "1",
                "--node-id",
                "lease-test",
                "--iface",
                IFACE,
            ])
            .arg("--member")
            .arg(member)
            .arg("--listen")
            .arg(&endpoint)
            .arg("--key-file")
            .arg(root.join("signing.pk8"))
            .arg("--cluster-dir")
            .arg(root.join("daemon-cluster"))
            .arg("--wal-file")
            .arg(root.join("node.wal"))
            .arg("--control-socket")
            .arg(&socket)
            .arg("--ebpf-obj")
            .arg(repo.join("assets/veil_xdp_kern.o"))
            .stdout(std::fs::File::create(root.join("daemon.stdout"))?)
            .stderr(std::fs::File::create(root.join("daemon.stderr"))?)
            .spawn()?;
        // The old unapproved WAL must not silently enter the approved daemon
        // profile. Full strict daemon BAN/unban/bypass coverage is exercised by
        // scripts/daemon-flow-check.py with independent operator credentials.
        let wal_before = std::fs::read(root.join("node.wal"))?;
        let mut child = OwnedChild(child);
        let exit = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(exit) = child.0.try_wait()? {
                    break Ok::<_, std::io::Error>(exit);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await??;
        anyhow::ensure!(!exit.success(), "unapproved legacy daemon started");
        anyhow::ensure!(!socket.exists(), "rejected daemon exposed control socket");
        anyhow::ensure!(
            std::fs::read(root.join("node.wal"))? == wal_before,
            "rejected legacy startup changed WAL"
        );
        let error = std::fs::read_to_string(root.join("daemon.stderr"))?;
        anyhow::ensure!(
            error.contains("approval-public-key"),
            "unexpected refusal: {error}"
        );
        println!("actual_daemon_unapproved_profile_refused_and_wal_preserved: PASS");
    }
    let after = Command::new("ip")
        .args(["-details", "link", "show", IFACE])
        .output()?;
    anyhow::ensure!(
        after.status.success() && !String::from_utf8_lossy(&after.stdout).contains("prog/xdp"),
        "test attachment remained"
    );
    anyhow::ensure!(ping()?, "final recovery ping failed");
    println!("cleanup: no XDP attachment; existing interface/namespace retained; ping PASS");
    Ok(())
}
