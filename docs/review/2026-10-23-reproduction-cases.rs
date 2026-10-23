// Review reproductions for c8d3f7f. Append only to an isolated copy of proof_tests.rs.
// These tests assert the presence of defects; passing does not mean the product is safe.

#[test]
fn audit_apply_error_is_reported_as_applied() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct FailingBridge(AtomicUsize);
    impl crate::usecase::EbpfBridge for FailingBridge {
        fn sync_peer_policy(&self, _: u32, _: veil_common::PeerPolicy) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("injected map update failure")
        }
        fn sync_lsm_policy(&self, _: veil_common::NodeLsmPolicy) -> Result<()> {
            anyhow::bail!("injected map update failure")
        }
        fn stealth_gc_purge_count(&self) -> u64 {
            0
        }
    }
    let bridge = Arc::new(FailingBridge(AtomicUsize::new(0)));
    let mut node = ConsensusState::new("single-node", vec![]).with_ebpf_bridge(bridge.clone());
    node.start_election();
    let ip = Ipv4Addr::new(203, 0, 113, 55);
    node.propose(MeshCommand::set_trust(ip, false)).unwrap();
    assert_eq!(bridge.0.load(Ordering::SeqCst), 1);
    assert_eq!(node.commit_index, 1);
    assert_eq!(node.last_applied, 1);
    assert_eq!(node.check_xdp_packet(ip), XdpVerdict::Drop);
}

// No interface is attached. The kernel executes synthetic packet bytes through
// BPF_PROG_TEST_RUN; this does not send network traffic or pin kernel objects.
// Requires Linux x86_64 and BPF privileges. Syscall number 321 is x86_64-specific.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn audit_linux_xdp_source_port_and_map_capacity() -> Result<()> {
    use std::os::fd::{AsFd, AsRawFd};
    #[repr(C)]
    #[derive(Default)]
    struct TestRun {
        prog_fd: u32,
        retval: u32,
        data_size_in: u32,
        data_size_out: u32,
        data_in: u64,
        data_out: u64,
        repeat: u32,
        duration: u32,
        ctx_size_in: u32,
        ctx_size_out: u32,
        ctx_in: u64,
        ctx_out: u64,
        flags: u32,
        cpu: u32,
        batch_size: u32,
        // Make trailing ABI padding explicit so CHECK_ATTR sees zero bytes.
        padding: u32,
    }
    unsafe extern "C" {
        fn syscall(number: std::os::raw::c_long, ...) -> std::os::raw::c_long;
    }
    fn verdict(fd: i32, source_port: u16) -> Result<u32> {
        let mut packet = [0u8; 64];
        packet[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        packet[14] = 0x45;
        packet[16..18].copy_from_slice(&40u16.to_be_bytes());
        packet[22] = 64;
        packet[23] = 6;
        packet[26..30].copy_from_slice(&[203, 0, 113, 55]);
        packet[30..34].copy_from_slice(&[198, 51, 100, 10]);
        packet[34..36].copy_from_slice(&source_port.to_be_bytes());
        packet[36..38].copy_from_slice(&443u16.to_be_bytes());
        packet[46] = 0x50;
        packet[47] = 0x02;
        let mut output = [0u8; 128];
        let mut attr = TestRun {
            prog_fd: fd as u32,
            data_size_in: packet.len() as u32,
            data_size_out: output.len() as u32,
            data_in: packet.as_ptr() as u64,
            data_out: output.as_mut_ptr() as u64,
            repeat: 1,
            ..Default::default()
        };
        let result = unsafe { syscall(321, 10u32, &mut attr, std::mem::size_of::<TestRun>()) };
        if result < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(attr.retval)
    }
    let object = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("assets/veil_xdp_kern.o");
    let mut bpf = aya::Ebpf::load_file(object)?;
    let program: &mut aya::programs::Xdp =
        bpf.program_mut("veil_xdp_filter").unwrap().try_into()?;
    program.load()?;
    let fd = program.fd()?.as_fd().as_raw_fd();
    let blocked_ip = u32::from(Ipv4Addr::new(203, 0, 113, 55));
    let mut map: aya::maps::HashMap<_, u32, veil_common::PeerPolicy> =
        aya::maps::HashMap::try_from(bpf.map_mut("PEER_POLICY_MAP").unwrap())?;
    map.insert(blocked_ip, veil_common::PeerPolicy::new(false, 1), 0)?;
    let regular = verdict(fd, 40000)?;
    let ssh_source = verdict(fd, 22)?;
    let alternate_ssh_source = verdict(fd, 10023)?;
    assert_eq!(regular, 1, "ordinary source port must hit the deny map");
    assert_eq!(ssh_source, 2, "source port 22 bypasses the deny map");
    assert_eq!(
        alternate_ssh_source, 2,
        "source port 10023 bypasses the deny map"
    );
    for i in 0..255u32 {
        map.insert(0xc0000200u32 + i, veil_common::PeerPolicy::new(false, 1), 0)?;
    }
    let capacity_error = map
        .insert(0xc0000300u32, veil_common::PeerPolicy::new(false, 1), 0)
        .is_err();
    assert!(
        capacity_error,
        "257th distinct map entry must fail in the shipped object"
    );
    println!("REVIEW_XDP ordinary={regular} source_22={ssh_source} source_10023={alternate_ssh_source} capacity_257_rejected={capacity_error}");
    Ok(())
}

// Independent audit reproductions: assertions intentionally confirm unsafe behavior.
fn audit_append(
    leader: &ConsensusState,
    follower: &mut ConsensusState,
    term: u64,
    prev_log_index: u64,
    prev_log_term: u64,
    entries: Vec<LogEntry>,
    leader_commit: u64,
    commit_qc: Option<QuorumCertificate>,
) -> ConsensusRpc {
    follower
        .handle_signed_rpc(leader.sign_rpc(ConsensusRpc::AppendEntries {
            term,
            leader_id: leader.node_id.clone(),
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit,
            commit_qc,
        }))
        .unwrap()
        .unwrap()
}
fn audit_sig(resp: ConsensusRpc) -> Vec<u8> {
    match resp {
        ConsensusRpc::AppendEntriesResponse {
            success: true,
            qc_vote_signature,
            ..
        } => qc_vote_signature,
        other => panic!("expected accepted AppendEntries, got {other:?}"),
    }
}
fn audit_qc(a: &ConsensusState, e: &LogEntry, others: Vec<(String, Vec<u8>)>) -> QuorumCertificate {
    let mut signatures: BTreeMap<String, Vec<u8>> = others.into_iter().collect();
    signatures.insert(
        a.node_id.clone(),
        a.identity.sign_qc_vote(e.term, e.index, &e.digest()),
    );
    QuorumCertificate {
        term: e.term,
        index: e.index,
        entry_hash: e.digest(),
        signatures,
    }
}
#[test]
fn audit_tail_qc_commits_conflicting_prefixes() {
    let (a, mut b, mut c, _d) = make_four_node_cluster();
    let ip = Ipv4Addr::new(203, 0, 113, 55);
    let x = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(ip, false),
    };
    let y = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(ip, true),
    };
    let tail = LogEntry {
        term: 1,
        index: 2,
        command: MeshCommand::NoOp,
    };
    let bs = audit_sig(audit_append(
        &a,
        &mut b,
        1,
        0,
        0,
        vec![x, tail.clone()],
        0,
        None,
    ));
    let cs = audit_sig(audit_append(
        &a,
        &mut c,
        1,
        0,
        0,
        vec![y, tail.clone()],
        0,
        None,
    ));
    let qc = audit_qc(
        &a,
        &tail,
        vec![(b.node_id.clone(), bs), (c.node_id.clone(), cs)],
    );
    audit_sig(audit_append(
        &a,
        &mut b,
        1,
        2,
        1,
        vec![],
        2,
        Some(qc.clone()),
    ));
    audit_sig(audit_append(&a, &mut c, 1, 2, 1, vec![], 2, Some(qc)));
    assert_eq!((b.commit_index, c.commit_index), (2, 2));
    assert_ne!(b.log[0], c.log[0]);
    assert_eq!(b.check_xdp_packet(ip), XdpVerdict::Drop);
    assert_eq!(c.check_xdp_packet(ip), XdpVerdict::Pass);
}
#[test]
fn audit_hidden_qc_allows_conflicting_commits_across_terms() {
    let (a, mut b, mut c, mut d) = make_four_node_cluster();
    let ip = Ipv4Addr::new(203, 0, 113, 55);
    let x = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(ip, false),
    };
    let y = LogEntry {
        term: 2,
        index: 1,
        command: MeshCommand::set_trust(ip, true),
    };
    let bs = audit_sig(audit_append(&a, &mut b, 1, 0, 0, vec![x.clone()], 0, None));
    let cs = audit_sig(audit_append(&a, &mut c, 1, 0, 0, vec![x.clone()], 0, None));
    let qx = audit_qc(
        &a,
        &x,
        vec![(b.node_id.clone(), bs), (c.node_id.clone(), cs)],
    );
    audit_sig(audit_append(&a, &mut b, 1, 1, 1, vec![], 1, Some(qx)));
    let cs = audit_sig(audit_append(&a, &mut c, 2, 0, 0, vec![y.clone()], 0, None));
    let ds = audit_sig(audit_append(&a, &mut d, 2, 0, 0, vec![y.clone()], 0, None));
    let qy = audit_qc(
        &a,
        &y,
        vec![(c.node_id.clone(), cs), (d.node_id.clone(), ds)],
    );
    audit_sig(audit_append(
        &a,
        &mut c,
        2,
        1,
        2,
        vec![],
        1,
        Some(qy.clone()),
    ));
    audit_sig(audit_append(&a, &mut d, 2, 1, 2, vec![], 1, Some(qy)));
    assert_eq!((b.commit_index, c.commit_index, d.commit_index), (1, 1, 1));
    assert_eq!(b.check_xdp_packet(ip), XdpVerdict::Drop);
    assert_eq!(c.check_xdp_packet(ip), XdpVerdict::Pass);
}
#[test]
fn audit_term_sandwich_obtains_two_votes_for_same_term_and_index() {
    let (a, mut b, _c, _d) = make_four_node_cluster();
    let x = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::NoOp,
    };
    let z = LogEntry {
        term: 2,
        index: 1,
        command: MeshCommand::NoOp,
    };
    let y = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(Ipv4Addr::new(203, 0, 113, 55), false),
    };
    let sx = audit_sig(audit_append(&a, &mut b, 1, 0, 0, vec![x.clone()], 0, None));
    audit_sig(audit_append(&a, &mut b, 2, 0, 0, vec![z], 0, None));
    let sy = audit_sig(audit_append(&a, &mut b, 2, 0, 0, vec![y.clone()], 0, None));
    for (entry, sig) in [(x, sx), (y, sy)] {
        assert!(NodeCryptoIdentity::verify_signature(
            &b.identity.public_key_bytes,
            &crate::domain::qc_vote_payload(1, 1, &entry.digest()),
            &sig
        ));
    }
}
#[test]
fn audit_last_accepted_term_saturates_election() {
    let (a, mut b, _c, _d) = make_four_node_cluster();
    b.handle_signed_rpc(a.sign_rpc(ConsensusRpc::RequestVote {
        term: crate::domain::MAX_SAFE_TERM - 1,
        candidate_id: a.node_id.clone(),
        last_log_index: 0,
        last_log_term: 0,
    }))
    .unwrap();
    let previous = b.current_term;
    b.start_election();
    assert_eq!(b.current_term, previous);
    assert_eq!(b.voted_for, Some(b.node_id.clone())); // Re-voted in the same term.
}
#[test]
fn audit_catch_up_batch_exceeds_receivers_entry_limit() {
    let (mut a, mut b, _c, _d) = make_four_node_cluster();
    a.role = Role::Leader;
    a.current_term = 1;
    for _ in 0..=crate::domain::MAX_LOG_ENTRIES_PER_RPC {
        a.propose(MeshCommand::NoOp).unwrap();
    }
    let msg = a.sign_rpc(a.make_append_entries(&b.node_id).unwrap());
    let resp = b.handle_signed_rpc(msg).unwrap().unwrap();
    assert!(matches!(
        resp,
        ConsensusRpc::AppendEntriesResponse { success: false, .. }
    ));
    assert!(b.log.is_empty());
}

#[tokio::test]
async fn transport_review_propose_returns_success_without_commit() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (mut alpha, mut beta, mut gamma, _delta) = make_four_node_cluster();
    let vote = alpha.start_signed_election();
    for peer in [&mut beta, &mut gamma] {
        let response = peer.handle_signed_rpc(vote.clone()).unwrap().unwrap();
        alpha.handle_signed_rpc(peer.sign_rpc(response)).unwrap();
    }
    assert_eq!(alpha.role, Role::Leader);
    let unavailable_peer = std::net::UdpSocket::bind("127.0.0.1:0")?;
    let unavailable_addr = unavailable_peer.local_addr()?;
    let endpoint = make_quic_endpoint("127.0.0.1:0".parse()?)?;
    let node = MeshNode {
        node_id: "node-alpha".into(),
        endpoint,
        state: Arc::new(Mutex::new(alpha)),
        peer_addrs: BTreeMap::from([
            ("node-beta".into(), unavailable_addr),
            ("node-gamma".into(), unavailable_addr),
            ("node-delta".into(), unavailable_addr),
        ]),
    };
    let returned = node
        .propose_and_commit(MeshCommand::set_trust(Ipv4Addr::new(10, 0, 0, 99), false))
        .await?;
    let state = node.state.lock().await;
    println!(
        "REVIEW_UNCOMMITTED_SUCCESS returned_index={returned} commit_index={} last_applied={}",
        state.commit_index, state.last_applied
    );
    assert_eq!(returned, 1);
    assert_eq!(state.commit_index, 0);
    assert_eq!(state.last_applied, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn transport_review_existing_key_permissions_are_not_validated() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let unique_dir = std::env::temp_dir().join(format!(
        "veil-mesh-transport-review-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&unique_dir)?;
    let key_path = unique_dir.join("review-generated-identity.pk8");
    let original = NodeCryptoIdentity::load_or_generate_file("permission-check", &key_path)?;
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644))?;
    let loaded = NodeCryptoIdentity::load_or_generate_file("permission-check", &key_path)?;
    let actual_mode = std::fs::metadata(&key_path)?.permissions().mode() & 0o777;
    println!(
        "REVIEW_INSECURE_KEY_ACCEPTED mode={actual_mode:o} same_public_key={}",
        original.public_key_bytes == loaded.public_key_bytes
    );
    assert_eq!(actual_mode, 0o644);
    assert_eq!(original.public_key_bytes, loaded.public_key_bytes);
    let _ = std::fs::remove_dir_all(&unique_dir);
    Ok(())
}
