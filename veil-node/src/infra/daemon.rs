//! Automatic driving for a fixed membership. Election timers provide ordinary
//! crash-fault progress; they are not a Byzantine view-change protocol.
use crate::{
    domain::{MeshCommand, Role, SignedConsensusRpc},
    infra::{node_tls::NodeTls, send_authenticated_rpc},
    usecase::ConsensusState,
};
use ring::rand::{SecureRandom, SystemRandom};
use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{sync::Mutex, task::JoinSet, time::Instant};

fn election_delay() -> anyhow::Result<Duration> {
    let mut bytes = [0; 8];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Election timer random source failed"))?;
    Ok(Duration::from_millis(
        1_800 + u64::from_le_bytes(bytes) % 1_801,
    ))
}

pub async fn run(
    endpoint: quinn::Endpoint,
    state: Arc<Mutex<ConsensusState>>,
    peers: BTreeMap<String, SocketAddr>,
    tls: Arc<NodeTls>,
) -> anyhow::Result<()> {
    let mut observed_activity = state.lock().await.election_activity();
    let mut deadline = Instant::now() + election_delay()?;
    let mut initialized_leader_term = None;
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let round_peers = peers.clone();
        let (work, activity, next_deadline, initialized) = super::state_worker::run(state.clone(), move |st| {
            let peers = round_peers;
            let mut work = Vec::new();
            anyhow::ensure!(
                st.persistence_error.is_none(),
                "Consensus persistence failed"
            );
            if let Some(rpc) = st.drive_stable_checkpoint().map_err(anyhow::Error::msg)? {
                work.extend(
                    peers
                        .iter()
                        .map(|(id, addr)| (id.clone(), *addr, rpc.clone())),
                );
            }
            if let Some(rpc) = st.drive_two_phase().map_err(anyhow::Error::msg)? {
                work.extend(
                    peers
                        .iter()
                        .map(|(id, addr)| (id.clone(), *addr, rpc.clone())),
                );
            }
            if let Some(rpc) = st.drive_ban_progress().map_err(anyhow::Error::msg)? {
                work.extend(
                    peers
                        .iter()
                        .map(|(id, addr)| (id.clone(), *addr, rpc.clone())),
                );
            }
            if st.election_activity() != observed_activity {
                observed_activity = st.election_activity();
                deadline = Instant::now() + election_delay()?;
            }
            if st.role != Role::Leader && Instant::now() >= deadline {
                let request = st.start_pre_vote().map_err(anyhow::Error::msg)?;
                tracing::info!(node = %st.node_id, term = st.current_term, "Starting pre-vote");
                deadline = Instant::now() + election_delay()?;
                work.extend(
                    peers
                        .iter()
                        .map(|(id, addr)| (id.clone(), *addr, request.clone())),
                );
            }
            if let Some(request) = st.finish_pre_vote().map_err(anyhow::Error::msg)? {
                tracing::info!(node = %st.node_id, term = st.current_term, "Starting election");
                work.extend(
                    peers
                        .iter()
                        .map(|(id, addr)| (id.clone(), *addr, request.clone())),
                );
            }
            if let Some((id, probe)) = st.make_liveness_probe().map_err(anyhow::Error::msg)? {
                if let Some(addr) = peers.get(&id) {
                    work.push((id, *addr, probe));
                }
            }
            if st.role == Role::Leader {
                if !st.uses_two_phase() && initialized_leader_term != Some(st.current_term) {
                    // A current-term entry allows a recovered prefix to commit.
                    st.propose(MeshCommand::NoOp).map_err(anyhow::Error::msg)?;
                    initialized_leader_term = Some(st.current_term);
                    tracing::info!(node = %st.node_id, term = st.current_term, "Leader elected");
                }
                for (id, addr) in &peers {
                    if let Some(rpc) = st.make_append_entries(id) {
                        work.push((
                            id.clone(),
                            *addr,
                            st.sign_rpc_durable(rpc).map_err(anyhow::Error::msg)?,
                        ));
                    }
                }
            }
            Ok::<_, anyhow::Error>((work, observed_activity, deadline, initialized_leader_term))
        }).await??;
        observed_activity = activity;
        deadline = next_deadline;
        initialized_leader_term = initialized;
        let endpoint = endpoint.clone();
        let tls = tls.clone();
        exchange_round(state.clone(), &peers, work, move |id, addr, rpc| {
            let endpoint = endpoint.clone();
            let tls = tls.clone();
            async move { send_authenticated_rpc(&endpoint, addr, rpc, &tls, &id).await }
        })
        .await?;
    }
}

type Work = Vec<(String, SocketAddr, SignedConsensusRpc)>;

// A round remains bounded even when a quorum advances before a slow peer replies.
pub(crate) async fn exchange_round<F, Fut>(
    state: Arc<Mutex<ConsensusState>>,
    peers: &BTreeMap<String, SocketAddr>,
    work: Work,
    send: F,
) -> anyhow::Result<()>
where
    F: Fn(String, SocketAddr, SignedConsensusRpc) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<SignedConsensusRpc>>> + Send + 'static,
{
    let mut pending = JoinSet::new();
    let mut outgoing = work;
    let mut followups = 0;
    loop {
        for (id, addr, rpc) in outgoing.drain(..) {
            let response = send(id.clone(), addr, rpc);
            pending.spawn(async move {
                (
                    id,
                    tokio::time::timeout(Duration::from_millis(800), response).await,
                )
            });
        }
        let Some(result) = pending.join_next().await else {
            break;
        };
        let (peer, result) = result?;
        match &result {
            Err(error) => tracing::debug!(%peer, %error, "RPC round timeout"),
            Ok(Err(error)) => tracing::debug!(%peer, %error, "RPC transport failure"),
            _ => {}
        }
        if let Ok(Ok(Some(reply))) = result {
            if reply.signer_id != peer {
                tracing::warn!(%peer, "Unexpected response signer");
                continue;
            }
            let peers = peers.clone();
            let may_follow_up = followups < 2;
            let (handled, next) = super::state_worker::run(state.clone(), move |st| {
                let before = st.phase_replication_position();
                let handled = st.handle_signed_rpc(reply);
                let after = st.phase_replication_position();
                let mut next = Vec::new();
                if handled.is_ok()
                    && may_follow_up
                    && before.is_some()
                    && after.is_some()
                    && before != after
                {
                    // A new durable prepare/commit proof is ready. Do not wait
                    // for the slowest peer or another tick before relaying it.
                    // Preserve each peer's certified catch-up ordering.
                    for (id, addr) in peers {
                        if let Some(rpc) = st.make_append_entries(&id) {
                            next.push((
                                id,
                                addr,
                                st.sign_rpc_durable(rpc).map_err(anyhow::Error::msg)?,
                            ));
                        }
                    }
                }
                Ok::<_, anyhow::Error>((handled, next))
            })
            .await??;
            if let Err(error) = handled {
                tracing::warn!(%peer, %error, "Consensus response rejected");
            }
            if !next.is_empty() {
                followups += 1;
                outgoing = next;
            }
        }
    }
    Ok(())
}
