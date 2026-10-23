// ==============================================================================
// veil-node: Clean Architecture Composition Root
// - domain:   Enterprise Domain Entities & Guardrail Security Policies
// - usecase:  ConsensusState Engine & Output Port Traits (`EbpfBridge`)
// - adapter:  Ring Ed25519 CSPRNG Identity (`0600` KeyFile) & Wire Protocol
// - infra:    Quinn mTLS 1.3 Transport, Aya Linux Kernel eBPF, Ratatui TUI Arena
// ==============================================================================

pub mod adapter;
pub mod consensus;
pub mod domain;
pub mod ebpf_bridge;
pub mod infra;
pub mod tui;
pub mod usecase;

#[cfg(test)]
mod proof_tests;

use adapter::NodeCryptoIdentity;
use anyhow::Result;
use clap::Parser;
use infra::{handle_connection, send_ping_round, send_rpc_over_quic, ClusterMtlsPki};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tracing::{info, warn};
use usecase::ConsensusState;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "veil-node",
    about = "veil-mesh-consensus QUIC & eBPF node daemon (Clean Architecture)"
)]
struct Args {
    /// Node identifier (e.g. `node-alpha`)
    #[arg(long, default_value = "node-alpha")]
    node_id: String,

    /// Local QUIC UDP bind address
    #[arg(long, default_value = "0.0.0.0:5000")]
    listen: SocketAddr,

    /// Peer QUIC addresses (can be specified multiple times: `--peer 10.0.0.2:5000 --peer 10.0.0.3:5000`)
    #[arg(long)]
    peer: Vec<SocketAddr>,

    /// Shared cluster directory for mTLS Root CA, peer Ed25519 public keys, and WAL state
    #[arg(long)]
    cluster_dir: Option<PathBuf>,

    /// Path to persistent Ed25519 private key file (generated via CSPRNG with 0600 permissions if absent)
    #[arg(long)]
    key_file: Option<PathBuf>,

    /// Path to persistent Crash-Recovery WAL file (`0600`)
    #[arg(long)]
    wal_file: Option<PathBuf>,

    /// Target network interface for XDP attachment (default: eth1)
    #[arg(long, default_value = "eth1")]
    iface: String,

    /// Path to compiled eBPF object file (optional on non-Linux / dev mode)
    #[arg(long)]
    ebpf_obj: Option<String>,

    /// Launch interactive TUI Arena
    #[arg(long)]
    tui: bool,

    /// Run automated TUI demo scenario and exit
    #[arg(long)]
    tui_demo: bool,
}

fn sync_cluster_peer_pubkeys(
    cluster_dir: &std::path::Path,
    node_id: &str,
    my_pub_key: &[u8],
) -> BTreeMap<String, Vec<u8>> {
    let peers_dir = cluster_dir.join("peers");
    let _ = std::fs::create_dir_all(&peers_dir);
    let my_pub_path = peers_dir.join(format!("{node_id}.pub"));
    let _ = std::fs::write(&my_pub_path, my_pub_key);

    let mut map = BTreeMap::new();
    map.insert(node_id.to_string(), my_pub_key.to_vec());
    if let Ok(entries) = std::fs::read_dir(&peers_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("pub") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(bytes) = std::fs::read(&path) {
                        if bytes.len() == 32 {
                            map.insert(stem.to_string(), bytes);
                        }
                    }
                }
            }
        }
    }
    map
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "veil_node=info".into()),
        )
        .init();

    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = Args::parse();

    if args.tui || args.tui_demo {
        #[allow(unused_mut)]
        let mut real_bridge: Option<Arc<dyn usecase::EbpfBridge>> = None;
        #[cfg(target_os = "linux")]
        if let Some(ref obj_path) = args.ebpf_obj {
            if let Ok(b) = ebpf_bridge::AyaKernelBridge::load_and_attach(obj_path, &args.iface) {
                real_bridge = Some(Arc::new(b));
            }
        }
        return tui::run_tui(args.tui_demo, real_bridge, args.iface);
    }

    let cluster_dir = args
        .cluster_dir
        .clone()
        .or_else(|| std::env::var_os("VEIL_CLUSTER_PKI_DIR").map(PathBuf::from))
        .unwrap_or_else(|| std::env::temp_dir().join("veil-mesh-cluster-pki-v2"));
    std::fs::create_dir_all(&cluster_dir)?;

    info!(
        node_id = %args.node_id,
        listen = %args.listen,
        peers = ?args.peer,
        cluster_dir = %cluster_dir.display(),
        iface = %args.iface,
        "Starting veil-node daemon"
    );

    let peer_ids: Vec<String> = args
        .peer
        .iter()
        .enumerate()
        .map(|(i, addr)| format!("peer-{i}-{addr}"))
        .collect();
    let mut consensus_state = ConsensusState::new(args.node_id.clone(), peer_ids);

    let key_path = args.key_file.clone().unwrap_or_else(|| {
        cluster_dir
            .join("keys")
            .join(format!("{}.pk8", args.node_id))
    });
    let identity = NodeCryptoIdentity::load_or_generate_file(&args.node_id, &key_path)?;
    let my_pub_bytes = identity.public_key_bytes.clone();
    consensus_state.set_crypto_identity(identity);
    info!(path = %key_path.display(), "Loaded/generated CSPRNG Ed25519 key file (0600)");

    let pub_keys = sync_cluster_peer_pubkeys(&cluster_dir, &args.node_id, &my_pub_bytes);
    consensus_state.register_peer_public_keys(pub_keys);

    #[cfg(target_os = "linux")]
    if let Some(ref obj_path) = args.ebpf_obj {
        match ebpf_bridge::AyaKernelBridge::load_and_attach(obj_path, &args.iface) {
            Ok(bridge) => {
                consensus_state = consensus_state.with_ebpf_bridge(Arc::new(bridge));
                info!("Connected ConsensusState to Linux kernel Aya eBPF maps (`PEER_POLICY_MAP` & `LSM_POLICY_MAP`)");
            }
            Err(err) => {
                warn!(error = %err, "Failed to attach eBPF program (continuing in QUIC-only mode)");
            }
        }
    }

    let wal_path = args.wal_file.clone().unwrap_or_else(|| {
        cluster_dir
            .join("wal")
            .join(format!("{}.wal", args.node_id))
    });
    if wal_path.exists() {
        consensus_state.load_wal(&wal_path)?;
        info!(path = %wal_path.display(), term = consensus_state.current_term, commit_index = consensus_state.commit_index, "Restored ConsensusState from Crash-Recovery WAL");
    } else {
        consensus_state.save_wal(&wal_path)?;
    }

    let pki = ClusterMtlsPki::load_or_generate_dir(&cluster_dir)?;
    let endpoint = pki.make_endpoint(args.listen)?;
    let state = Arc::new(Mutex::new(consensus_state));

    for (idx, peer_addr) in args.peer.into_iter().enumerate() {
        let client_endpoint = endpoint.clone();
        let node_id = args.node_id.clone();
        let peer_label = format!("peer-{idx}-{peer_addr}");
        let state_bg = state.clone();
        let cluster_dir_bg = cluster_dir.clone();
        let my_pub_bg = my_pub_bytes.clone();
        let wal_path_bg = wal_path.clone();
        tokio::spawn(async move {
            let mut epoch: u32 = 1;
            loop {
                {
                    let latest_keys =
                        sync_cluster_peer_pubkeys(&cluster_dir_bg, &node_id, &my_pub_bg);
                    let mut st = state_bg.lock().await;
                    st.register_peer_public_keys(latest_keys);
                }
                if let Err(err) =
                    send_ping_round(&client_endpoint, peer_addr, &node_id, epoch).await
                {
                    warn!(peer = %peer_addr, epoch, error = %err, "Consensus ping round failed");
                }
                let maybe_ae = {
                    let st = state_bg.lock().await;
                    st.make_append_entries(&peer_label)
                        .map(|rpc| st.sign_rpc(rpc))
                };
                if let Some(signed_ae) = maybe_ae {
                    if let Ok(Some(signed_resp)) =
                        send_rpc_over_quic(&client_endpoint, peer_addr, signed_ae).await
                    {
                        let mut st = state_bg.lock().await;
                        let _ = st.handle_signed_rpc(signed_resp);
                        let _ = st.save_wal(&wal_path_bg);
                    }
                }
                epoch = epoch.wrapping_add(1);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    while let Some(incoming) = endpoint.accept().await {
        let node_id = args.node_id.clone();
        let state = state.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    if let Err(err) = handle_connection(conn, &node_id, state).await {
                        warn!(error = %err, "Connection handler error");
                    }
                }
                Err(err) => warn!(error = %err, "Incoming QUIC mTLS handshake failed"),
            }
        });
    }

    Ok(())
}
