//! Owner-only Unix control socket. No TCP listener or remote administration.
//! Socket paths are never unlinked implicitly; use a fresh path on each start.
use crate::{
    domain::{MeshCommand, Role, MAX_POLICY_TTL_SECS},
    usecase::ConsensusState,
};
use std::{net::Ipv4Addr, path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Mutex,
};

pub fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Control socket needs a private directory"))?;
    let meta = std::fs::symlink_metadata(parent)?;
    anyhow::ensure!(
        meta.is_dir() && meta.permissions().mode() & 0o777 == 0o700,
        "Control socket parent must be a real directory with mode 0700"
    );
    let listener = UnixListener::bind(path)?;
    let socket_meta = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        socket_meta.uid() == meta.uid(),
        "Control directory must belong to daemon owner"
    );
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub async fn serve(
    listener: UnixListener,
    state: Arc<Mutex<ConsensusState>>,
) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let path = listener
        .local_addr()?
        .as_pathname()
        .ok_or_else(|| anyhow::anyhow!("Missing socket path"))?
        .to_owned();
    let owner = std::fs::symlink_metadata(path)?.uid();
    let slots = Arc::new(tokio::sync::Semaphore::new(8));
    loop {
        let (mut stream, _) = listener.accept().await?;
        if stream.peer_cred()?.uid() != owner {
            continue;
        }
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let state = state.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let result = tokio::time::timeout(Duration::from_secs(2), async {
                // Fully read a bounded request before taking the consensus lock.
                let command = stream.read_u8().await?;
                let (index, ip, ttl) = match command {
                    b'B' | b'R' => (
                        0,
                        Some(Ipv4Addr::from(stream.read_u32().await?)),
                        stream.read_u32().await?,
                    ),
                    b'S' => {
                        let index = stream.read_u64().await?;
                        let has_ip = stream.read_u8().await?;
                        anyhow::ensure!(has_ip <= 1, "Invalid status IP flag");
                        let ip = if has_ip == 1 {
                            Some(Ipv4Addr::from(stream.read_u32().await?))
                        } else {
                            None
                        };
                        (index, ip, 0)
                    }
                    _ => (0, None, 0),
                };
                let mut request_id = [0; 32];
                if command == b'Q' { stream.read_exact(&mut request_id).await?; }
                let approved = if command == b'A' {
                    use bincode::Options;
                    let len=stream.read_u32().await? as usize;
                    anyhow::ensure!(len <= 1024, "Approval frame too large");
                    let mut bytes=vec![0;len]; stream.read_exact(&mut bytes).await?;
                    Some(bincode::DefaultOptions::new().with_fixint_encoding().with_limit(1024).reject_trailing_bytes().deserialize::<crate::domain::ApprovedPolicy>(&bytes)?)
                } else { None };
                let reply = super::state_worker::run(state.clone(), move |st| {
                    match command {
                        b'0' | b'1' if st.ebpf_bridge.is_none() => {
                            "ERROR no XDP backend configured\n".into()
                        }
                        b'0' | b'1' => match st.set_emergency_local_bypass(command == b'1') {
                            Ok(()) => "OK\n".into(),
                            Err(err) => format!("ERROR {err:?}\n"),
                        },
                        b'B' => ban_reply(st, ip.expect("BAN IP read above"), ttl),
                        b'R' => match st.accept_ban_request(ip.expect("request IP"), ttl) {
                            Ok(id) => format!("REQUESTED id={}\nAcceptance is durable; query --status-local --request-id {}. TTL includes queue/election delay. Do not blindly retry on a lost reply.\n", hex(&id), hex(&id)),
                            Err(error) => format!("ERROR {error}; acceptance may be uncertain after a WAL error\n"),
                        },
                        b'A' => match st.accept_policy_request(MeshCommand::AuthorizedPolicy(Box::new(approved.expect("approval read")))) {
                            Ok(id) => format!("REQUESTED id={}\n", hex(&id)),
                            Err(error) => format!("ERROR {error}\n"),
                        },
                        b'Q' => st.ban_request_status(&request_id),
                        b'S' => status_reply(st, index, ip),
                        _ => "ERROR unknown local command\n".into(),
                    }
                }).await?;
                stream.write_all(reply.as_bytes()).await?;
                stream.shutdown().await?;
                Ok::<_, anyhow::Error>(())
            })
            .await;
            if !matches!(result, Ok(Ok(()))) {
                tracing::warn!("Local control request failed or timed out");
            }
        });
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ban_reply(st: &mut ConsensusState, ip: Ipv4Addr, ttl: u32) -> String {
    if ttl == 0 || ttl > MAX_POLICY_TTL_SECS {
        return "ERROR TTL must be 1..=86400 seconds\n".into();
    }
    if st.role != Role::Leader {
        return format!(
            "ERROR not leader; leader={}\n",
            st.leader_id.as_deref().unwrap_or("unknown")
        );
    }
    match st.propose(MeshCommand::set_trust_with_ttl(ip, false, ttl)) {
        Ok(entry) => {
            let deadline = match &entry.command {
                MeshCommand::SetPeerTrustLeaseBatch { rules, .. } => rules[0].not_after_unix_ns,
                _ => unreachable!("positive TTL is stamped before log insertion"),
            };
            format!("ACCEPTED index={} term={} hash={} not_after_unix_ns={}\nAcceptance is durable; quorum commit and local apply must be checked with --status-local --entry-index {}. Do not blindly retry on a lost reply.\n",
                entry.index, entry.term, hex(&st.entry_hash_at(entry.index).expect("accepted entry hash")), deadline, entry.index)
        }
        Err(error) => format!("ERROR {error}; acceptance may be uncertain after a WAL error\n"),
    }
}

fn status_reply(st: &mut ConsensusState, index: u64, ip: Option<Ipv4Addr>) -> String {
    // Read current quality, not just the error from the last approval attempt.
    // This is observational: it never sets the clock or changes WAL state.
    let clock = st.lease_time();
    let index = if index == 0 {
        st.last_log_index()
    } else {
        index
    };
    let phase = if index > 0 && index < st.log_base() {
        "compacted"
    } else if st.entry_hash_at(index).is_none() {
        "absent"
    } else if st.last_applied >= index {
        "applied"
    } else if st.commit_index >= index {
        "committed"
    } else {
        "pending"
    };
    let mut reply = format!("OK\nnode={}\nrole={:?}\nleader={}\nterm={}\nquorum={}\nlast_log_index={}\ncommit_index={}\nlast_applied={}\nbackend={}\nbypass={}\npersistence_error={:?}\napply_error={:?}\nlease_clock_error={:?}\nentry_index={}\nentry_hash={}\nentry_phase={}\n",
        st.node_id, st.role, st.leader_id.as_deref().unwrap_or("unknown"), st.current_term,
        st.quorum_size(), st.last_log_index(), st.commit_index, st.last_applied,
        if st.ebpf_bridge.is_some() { "configured" } else { "none" }, st.emergency_local_bypass,
        st.persistence_error, st.last_apply_error, st.lease_clock_error, index,
        st.entry_hash_at(index).map(|h| hex(&h)).unwrap_or_else(|| "none".into()), phase);
    reply.push_str(&format!("wal_bytes={}\nwal_limit_bytes={}\nwal_remaining_bytes={}\ncheckpoint_height={}\nretained_entries={}\n", st.persisted_wal_bytes, st.max_wal_bytes, st.max_wal_bytes.saturating_sub(st.persisted_wal_bytes), st.log_base(), st.log.len()));
    match clock {
        Ok(sample) => reply.push_str(&format!(
            "clock_quality=bounded\nclock_unix_ns={}\nclock_boot_ns={}\nclock_uncertainty_ns={}\n",
            sample.unix_ns, sample.boot_ns, sample.uncertainty_ns
        )),
        Err(_) => reply.push_str("clock_quality=unavailable\n"),
    }
    if let Some((pass, drop)) = st.ebpf_bridge.as_ref().and_then(|b| b.read_xdp_counters()) {
        reply.push_str(&format!("kernel_pass={pass}\nkernel_drop={drop}\n"));
    }
    reply.push_str(&format!(
        "authorization_revision={}\nretired_through_unix_ns={}\n",
        st.authorization_revision(),
        st.retired_through_unix_ns()
    ));
    if let Some(profile) = st.approval_policy() {
        reply.push_str("policy_authority=independent_operator\nscope_coverage=plain_ethernet_unfragmented_ipv4\n");
        for (i, scope) in profile.scopes.iter().enumerate() {
            reply.push_str(&format!(
                "service_scope_{i}={}/{}/{}\n",
                Ipv4Addr::from(scope.destination),
                if scope.protocol == 6 { "tcp" } else { "udp" },
                scope.port
            ));
        }
    }
    if let Some(ip) = ip {
        reply.push_str(&format!(
            "policy_ip={}\npolicy_mirror={:?}\npolicy_boot_deadline_ns={}\n",
            ip,
            st.check_xdp_packet(ip),
            st.peer_rule_deadlines_ns
                .get(&u32::from(ip))
                .copied()
                .unwrap_or(0)
        ));
    }
    reply
}

async fn exchange(path: &Path, command: &[u8]) -> anyhow::Result<String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = UnixStream::connect(path).await?;
        stream.write_all(command).await?;
        let mut reply = String::new();
        stream.take(16_384).read_to_string(&mut reply).await?;
        anyhow::ensure!(
            reply.starts_with("OK\n")
                || reply.starts_with("ACCEPTED ")
                || reply.starts_with("REQUESTED "),
            "Local request failed: {}",
            reply.trim()
        );
        Ok(reply)
    })
    .await?
}

pub async fn submit_ban(path: &Path, ip: Ipv4Addr, ttl: u32) -> anyhow::Result<String> {
    let mut command = vec![b'B'];
    command.extend(u32::from(ip).to_be_bytes());
    command.extend(ttl.to_be_bytes());
    exchange(path, &command).await
}

pub async fn status(
    path: &Path,
    index: Option<u64>,
    ip: Option<Ipv4Addr>,
) -> anyhow::Result<String> {
    let mut command = vec![b'S'];
    command.extend(index.unwrap_or(0).to_be_bytes());
    command.push(u8::from(ip.is_some()));
    if let Some(ip) = ip {
        command.extend(u32::from(ip).to_be_bytes());
    }
    exchange(path, &command).await
}

pub async fn request(path: &Path, enabled: bool) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = UnixStream::connect(path).await?;
        stream.write_all(if enabled { b"1" } else { b"0" }).await?;
        let mut reply = String::new();
        stream.take(4096).read_to_string(&mut reply).await?;
        anyhow::ensure!(
            reply == "OK\n",
            "Bypass was not acknowledged: {}",
            reply.trim()
        );
        Ok(())
    })
    .await?
}

/// Witness a finite request locally, including on a follower.
pub async fn submit_ban_request(path: &Path, ip: Ipv4Addr, ttl: u32) -> anyhow::Result<String> {
    let mut command = vec![b'R'];
    command.extend(u32::from(ip).to_be_bytes());
    command.extend(ttl.to_be_bytes());
    exchange(path, &command).await
}
pub async fn request_status(path: &Path, id: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Request ID must be 64 hex characters"
    );
    let mut command = vec![b'Q'];
    for i in (0..64).step_by(2) {
        command.push(u8::from_str_radix(&id[i..i + 2], 16)?);
    }
    exchange(path, &command).await
}

pub async fn submit_approval(
    path: &Path,
    approval: &crate::domain::ApprovedPolicy,
) -> anyhow::Result<String> {
    let bytes = bincode::serialize(approval)?;
    anyhow::ensure!(bytes.len() <= 1024, "Approval too large");
    let mut request = vec![b'A'];
    request.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    request.extend_from_slice(&bytes);
    exchange(path, &request).await
}
