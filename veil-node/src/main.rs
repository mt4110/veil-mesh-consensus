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
use infra::{handle_connection, make_quic_endpoint, send_ping_round};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
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

    /// Path to persistent Ed25519 private key file (generated via CSPRNG with 0600 permissions if absent)
    #[arg(long)]
    key_file: Option<PathBuf>,

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

    info!(
        node_id = %args.node_id,
        listen = %args.listen,
        peers = ?args.peer,
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

    if let Some(ref key_path) = args.key_file {
        let identity = NodeCryptoIdentity::load_or_generate_file(&args.node_id, key_path)?;
        consensus_state.set_crypto_identity(identity);
        info!(path = %key_path.display(), "Loaded/generated CSPRNG Ed25519 key file (0600)");
    }

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

    let endpoint = make_quic_endpoint(args.listen)?;
    let state = Arc::new(Mutex::new(consensus_state));

    for peer_addr in args.peer {
        let client_endpoint = endpoint.clone();
        let node_id = args.node_id.clone();
        tokio::spawn(async move {
            let mut epoch: u32 = 1;
            loop {
                if let Err(err) =
                    send_ping_round(&client_endpoint, peer_addr, &node_id, epoch).await
                {
                    warn!(peer = %peer_addr, epoch, error = %err, "Consensus ping round failed");
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
