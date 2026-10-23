pub mod consensus;
pub mod ebpf_bridge;
pub mod tui;

use anyhow::Result;
use clap::Parser;
use consensus::{ConsensusState, MeshCommand, SignedConsensusRpc, MAX_WIRE_MESSAGE_BYTES};
use serde::{Deserialize, Serialize};
#[cfg(any(test, target_os = "linux"))]
use std::net::Ipv4Addr;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "veil-node",
    about = "veil-mesh-consensus QUIC & eBPF node daemon"
)]
struct Args {
    /// Node identifier (e.g. `node-alpha` or `node-beta`)
    #[arg(long, default_value = "node-alpha")]
    node_id: String,

    /// Local QUIC UDP bind address
    #[arg(long, default_value = "0.0.0.0:5000")]
    listen: SocketAddr,

    /// Optional peer QUIC address to connect and exchange consensus heartbeats
    #[arg(long)]
    peer: Option<SocketAddr>,

    /// Target network interface for XDP attachment (default: eth1)
    #[arg(long, default_value = "eth1")]
    iface: String,

    /// Path to compiled eBPF object file (optional on non-Linux / dev mode)
    #[arg(long)]
    ebpf_obj: Option<String>,

    /// Launch interactive TUI Arena (spawns mock rogue attack processes & visualizes eBPF extermination)
    #[arg(long)]
    tui: bool,

    /// Run automated TUI demo scenario (for GIF recording / smoke check) and exit
    #[arg(long)]
    tui_demo: bool,
}

/// QUIC ストリーム上でやり取りするメッセージフレーム（すべての Consensus RPC は Ed25519 署名付き `SignedConsensusRpc` を必須とする）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireMessage {
    Ping {
        from_node: String,
        epoch: u32,
    },
    Ack {
        from_node: String,
        epoch: u32,
        trusted: bool,
    },
    Consensus(SignedConsensusRpc),
}

/// クラスタ内の相互 TLS (mTLS) 認証用 PKI（Root CA 証明書ピンニング + クライアント・サーバー双方の証明書検証）
pub struct ClusterMtlsPki {
    ca_cert: rcgen::Certificate,
    ca_key: rcgen::KeyPair,
    ca_cert_der: rustls::pki_types::CertificateDer<'static>,
}

impl ClusterMtlsPki {
    /// 新しいクラスタ専用 Root CA を生成する
    pub fn generate() -> Result<Self> {
        let mut ca_params = rcgen::CertificateParams::new(vec!["veil.mesh.ca".to_string()])?;
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::DigitalSignature,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        let ca_key = rcgen::KeyPair::generate()?;
        let ca_cert = ca_params.self_signed(&ca_key)?;
        let ca_cert_der = ca_cert.der().clone();
        Ok(Self {
            ca_cert,
            ca_key,
            ca_cert_der,
        })
    }

    /// このクラスタ CA によって署名されたノード用の mTLS (ServerAuth + ClientAuth) QUIC エンドポイントを生成する
    pub fn make_endpoint(&self, bind_addr: SocketAddr) -> Result<quinn::Endpoint> {
        let mut leaf_params = rcgen::CertificateParams::new(vec!["veil.mesh".to_string()])?;
        leaf_params.is_ca = rcgen::IsCa::NoCa;
        leaf_params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
        leaf_params.extended_key_usages = vec![
            rcgen::ExtendedKeyUsagePurpose::ServerAuth,
            rcgen::ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let leaf_key = rcgen::KeyPair::generate()?;
        let leaf_cert = leaf_params.signed_by(&leaf_key, &self.ca_cert, &self.ca_key)?;

        let leaf_cert_der = leaf_cert.der().clone();
        let leaf_priv_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(leaf_key.serialize_der()),
        );
        let leaf_priv_der_client = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(leaf_key.serialize_der()),
        );

        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(self.ca_cert_der.clone())?;
        let root_store_arc = Arc::new(root_store.clone());

        // Server 側: クライアント証明書を必須とし、クラスタ Root CA で厳密検証する (mTLS)
        let client_verifier =
            rustls::server::WebPkiClientVerifier::builder(root_store_arc).build()?;
        let server_crypto = rustls::ServerConfig::builder()
            .with_client_cert_verifier(client_verifier)
            .with_single_cert(
                vec![leaf_cert_der.clone(), self.ca_cert_der.clone()],
                leaf_priv_der,
            )?;
        let server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
        ));

        // Client 側: サーバー証明書をクラスタ Root CA で厳密検証し、自身のクライアント証明書を提示する (mTLS)
        let client_crypto = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_client_auth_cert(
                vec![leaf_cert_der, self.ca_cert_der.clone()],
                leaf_priv_der_client,
            )?;
        let client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)?,
        ));

        let mut endpoint = quinn::Endpoint::server(server_config, bind_addr)?;
        endpoint.set_default_client_config(client_config);
        Ok(endpoint)
    }
}

/// プロセス内のデフォルト共有クラスタ mTLS PKI を取得する
pub fn default_cluster_pki() -> &'static ClusterMtlsPki {
    static PKI: OnceLock<ClusterMtlsPki> = OnceLock::new();
    PKI.get_or_init(|| ClusterMtlsPki::generate().expect("Failed to initialize ClusterMtlsPki"))
}

/// クイック・コンセンサス・ノード（mTLS QUIC エンドポイント + 決定論的コンセンサス・ステートマシン）
pub struct MeshNode {
    pub node_id: String,
    pub endpoint: quinn::Endpoint,
    pub state: Arc<Mutex<ConsensusState>>,
    pub peer_addrs: BTreeMap<String, SocketAddr>,
}

impl MeshNode {
    pub fn new(
        node_id: impl Into<String>,
        listen_addr: SocketAddr,
        peer_addrs: BTreeMap<String, SocketAddr>,
    ) -> Result<Self> {
        let node_id = node_id.into();
        let peer_ids: Vec<String> = peer_addrs.keys().cloned().collect();
        let endpoint = make_quic_endpoint(listen_addr)?;
        let state = Arc::new(Mutex::new(ConsensusState::new(node_id.clone(), peer_ids)));

        Ok(Self {
            node_id,
            endpoint,
            state,
            peer_addrs,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    /// 受信 QUIC ストリームの待受タスクを起動
    pub fn spawn_server(&self) -> tokio::task::JoinHandle<()> {
        let endpoint = self.endpoint.clone();
        let state = self.state.clone();
        let node_id = self.node_id.clone();

        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let state = state.clone();
                let node_id = node_id.clone();
                tokio::spawn(async move {
                    if let Ok(conn) = incoming.await {
                        let _ = handle_connection(conn, &node_id, state).await;
                    }
                });
            }
        })
    }

    /// 選挙を開始し、mTLS QUIC 双方向ストリーム経由で全ピアから Ed25519 署名付き投票を収集する
    pub async fn campaign(&self) -> Result<bool> {
        let signed_req = {
            let mut st = self.state.lock().await;
            let req = st.start_election();
            st.sign_rpc(req)
        };

        for (peer_id, &addr) in &self.peer_addrs {
            match send_rpc_over_quic(&self.endpoint, addr, signed_req.clone()).await {
                Ok(Some(signed_resp)) => {
                    let mut st = self.state.lock().await;
                    let _ = st.handle_signed_rpc(signed_resp);
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(node = %self.node_id, peer = %peer_id, error = %err, "RequestVote QUIC RPC failed");
                }
            }
        }

        let is_leader = self.state.lock().await.role == consensus::Role::Leader;
        Ok(is_leader)
    }

    /// リーダーとして全フォロワーへ Ed25519 署名付き `AppendEntries`（およびマルチシグ `QuorumCertificate`）をレプリケートする
    pub async fn replicate_round(&self) -> Result<()> {
        for (peer_id, &addr) in &self.peer_addrs {
            let maybe_signed_rpc = {
                let st = self.state.lock().await;
                st.make_append_entries(peer_id).map(|rpc| st.sign_rpc(rpc))
            };

            if let Some(signed_rpc) = maybe_signed_rpc {
                if let Ok(Some(signed_resp)) =
                    send_rpc_over_quic(&self.endpoint, addr, signed_rpc).await
                {
                    let mut st = self.state.lock().await;
                    let _ = st.handle_signed_rpc(signed_resp);
                }
            }
        }
        Ok(())
    }

    /// リーダーとしてコマンドを提案し、2ラウンドのレプリケーション（1: $2f+1$ QC 署名収集 & リーダーCommit、2: フォロワーへの QC 付き Commit 伝搬）を実行する
    pub async fn propose_and_commit(&self, cmd: MeshCommand) -> Result<u64> {
        let index = {
            let mut st = self.state.lock().await;
            st.propose(cmd).map_err(|e| anyhow::anyhow!(e))?.index
        };

        // Round 1: エントリを全フォロワーへ送信し、2f+1 台分の Ed25519 QC 署名でリーダーが QuorumCertificate を構築 & Commit
        self.replicate_round().await?;
        // Round 2: 生成された QuorumCertificate (commit_qc) をフォロワーへ伝搬し、フォロワー側でも暗号検証後に Apply (eBPF Map 同期)
        self.replicate_round().await?;

        Ok(index)
    }
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
        let mut real_bridge: Option<Arc<dyn ebpf_bridge::EbpfBridge>> = None;
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
        peer = ?args.peer,
        iface = %args.iface,
        "Starting veil-node daemon"
    );

    #[allow(unused_mut)]
    let mut consensus_state = ConsensusState::new(args.node_id.clone(), vec![]);

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

    // ピア指定がある場合は定期的に QUIC Ping/Consensus ラウンドを実行
    if let Some(peer_addr) = args.peer {
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

    // 受信ループ
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

async fn handle_connection(
    conn: quinn::Connection,
    node_id: &str,
    state: Arc<Mutex<ConsensusState>>,
) -> Result<()> {
    let remote = conn.remote_address();
    while let Ok((mut send, mut recv)) = conn.accept_bi().await {
        let buf = recv.read_to_end(MAX_WIRE_MESSAGE_BYTES).await?;
        let msg: WireMessage = bincode::deserialize(&buf)?;
        match msg {
            WireMessage::Ping { from_node, epoch } => {
                info!(remote = %remote, from = %from_node, epoch, "Received QUIC Consensus Ping");
                let reply = WireMessage::Ack {
                    from_node: node_id.to_string(),
                    epoch,
                    trusted: true,
                };
                send.write_all(&bincode::serialize(&reply)?).await?;
                send.finish()?;
            }
            WireMessage::Consensus(signed_rpc) => {
                // ネットワーク受信経路では必ず handle_signed_rpc を通して Ed25519 署名を検証する！
                let maybe_signed_resp = {
                    let mut st = state.lock().await;
                    match st.handle_signed_rpc(signed_rpc) {
                        Ok(Some(resp)) => Some(st.sign_rpc(resp)),
                        Ok(None) => None,
                        Err(err) => {
                            warn!(remote = %remote, error = %err, "Rejected Byzantine/Unsigned Consensus RPC on wire");
                            None
                        }
                    }
                };
                if let Some(signed_resp) = maybe_signed_resp {
                    let wire = WireMessage::Consensus(signed_resp);
                    send.write_all(&bincode::serialize(&wire)?).await?;
                }
                send.finish()?;
            }
            WireMessage::Ack { .. } => {}
        }
    }
    Ok(())
}

async fn send_rpc_over_quic(
    endpoint: &quinn::Endpoint,
    peer_addr: SocketAddr,
    signed_rpc: SignedConsensusRpc,
) -> Result<Option<SignedConsensusRpc>> {
    let conn = tokio::time::timeout(
        Duration::from_millis(300),
        endpoint.connect(peer_addr, "veil.mesh")?,
    )
    .await??;

    let (mut send, mut recv) = conn.open_bi().await?;
    let wire = WireMessage::Consensus(signed_rpc);
    send.write_all(&bincode::serialize(&wire)?).await?;
    send.finish()?;

    let buf = tokio::time::timeout(
        Duration::from_millis(300),
        recv.read_to_end(MAX_WIRE_MESSAGE_BYTES),
    )
    .await??;
    if buf.is_empty() {
        return Ok(None);
    }
    if let WireMessage::Consensus(signed_resp) = bincode::deserialize(&buf)? {
        Ok(Some(signed_resp))
    } else {
        Ok(None)
    }
}

async fn send_ping_round(
    endpoint: &quinn::Endpoint,
    peer_addr: SocketAddr,
    node_id: &str,
    epoch: u32,
) -> Result<()> {
    let conn = endpoint.connect(peer_addr, "veil.mesh")?.await?;
    let (mut send, mut recv) = conn.open_bi().await?;

    let msg = WireMessage::Ping {
        from_node: node_id.to_string(),
        epoch,
    };
    send.write_all(&bincode::serialize(&msg)?).await?;
    send.finish()?;

    let buf = recv.read_to_end(4096).await?;
    let ack: WireMessage = bincode::deserialize(&buf)?;
    info!(peer = %peer_addr, ?ack, "Completed QUIC consensus round");
    Ok(())
}

/// クラスタ共通の Root CA ピンニング mTLS 設定を用いて QUIC エンドポイントを構築する
pub fn make_quic_endpoint(bind_addr: SocketAddr) -> Result<quinn::Endpoint> {
    default_cluster_pki().make_endpoint(bind_addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use consensus::{ConsensusRpc, LogEntry, NodeCryptoIdentity, QuorumCertificate, Role};
    use ebpf_bridge::{EbpfBridge, MockEbpfBridge};
    use veil_common::{evaluate_lsm_policy, evaluate_peer_policy, XdpVerdict};

    fn make_four_node_cluster() -> (
        ConsensusState,
        ConsensusState,
        ConsensusState,
        ConsensusState,
    ) {
        let names = ["node-alpha", "node-beta", "node-gamma", "node-delta"];
        let mut states: Vec<ConsensusState> = names
            .iter()
            .map(|&me| {
                let peers = names
                    .iter()
                    .filter(|&&n| n != me)
                    .map(|&s| s.to_string())
                    .collect();
                ConsensusState::new(me, peers)
            })
            .collect();
        let delta = states.pop().unwrap();
        let gamma = states.pop().unwrap();
        let beta = states.pop().unwrap();
        let alpha = states.pop().unwrap();
        (alpha, beta, gamma, delta)
    }

    /// 証明1: Election Safety & BFT Quorum ($2f+1 = 3/4$) 署名付き投票の検証
    /// 同一 Term 内で高々1つのリーダーしか選出されないこと、二重投票が厳密に拒否されることを証明する
    #[test]
    fn proof_1_election_safety_and_log_up_to_date_check() {
        let (mut alpha, mut beta, mut gamma, mut delta) = make_four_node_cluster();
        assert_eq!(alpha.quorum_size(), 3);

        // 1. node-alpha が Term 1 で立候補し、Ed25519 署名付き RequestVote を送信
        let vote_req_alpha = alpha.start_signed_election();
        assert_eq!(alpha.current_term, 1);
        assert_eq!(alpha.role, Role::Candidate);

        // 2. node-beta と node-delta が node-alpha に投票
        let resp_beta = beta
            .handle_signed_rpc(vote_req_alpha.clone())
            .unwrap()
            .unwrap();
        let resp_delta = delta.handle_signed_rpc(vote_req_alpha).unwrap().unwrap();
        assert_eq!(
            resp_beta,
            ConsensusRpc::RequestVoteResponse {
                term: 1,
                voter_id: "node-beta".into(),
                vote_granted: true,
            }
        );

        // 3. 同じ Term 1 で node-gamma も同時に立候補（Split Vote 競合の試行）
        let vote_req_gamma = gamma.start_signed_election();
        // すでに node-alpha に投票済みの node-beta は node-gamma への投票を厳密に拒否する
        let resp_beta_to_gamma = beta.handle_signed_rpc(vote_req_gamma).unwrap().unwrap();
        assert_eq!(
            resp_beta_to_gamma,
            ConsensusRpc::RequestVoteResponse {
                term: 1,
                voter_id: "node-beta".into(),
                vote_granted: false,
            }
        );

        // 4. node-alpha が node-beta, node-delta の署名付き賛成票を受け取り、BFT Quorum (3/4) 達成で唯一の Leader に昇格
        let signed_beta = beta.sign_rpc(resp_beta);
        let signed_delta = delta.sign_rpc(resp_delta);
        alpha.handle_signed_rpc(signed_beta).unwrap();
        alpha.handle_signed_rpc(signed_delta).unwrap();
        assert_eq!(alpha.role, Role::Leader);
        assert_ne!(gamma.role, Role::Leader);
    }

    /// 証明2: 4ノード実 QUIC (mTLS 1.3 over UDP) メッシュでの署名付き合意形成・QC 検証・eBPF XDP 遮断ポリシーの即時同期
    #[tokio::test]
    async fn proof_2_three_node_quic_consensus_and_ebpf_xdp_enforcement() -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let ep_a = make_quic_endpoint("127.0.0.1:0".parse()?)?;
        let ep_b = make_quic_endpoint("127.0.0.1:0".parse()?)?;
        let ep_c = make_quic_endpoint("127.0.0.1:0".parse()?)?;
        let ep_d = make_quic_endpoint("127.0.0.1:0".parse()?)?;

        let all_addrs = BTreeMap::from([
            ("node-alpha".to_string(), ep_a.local_addr()?),
            ("node-beta".to_string(), ep_b.local_addr()?),
            ("node-gamma".to_string(), ep_c.local_addr()?),
            ("node-delta".to_string(), ep_d.local_addr()?),
        ]);

        let build_peers = |me: &str| -> BTreeMap<String, SocketAddr> {
            all_addrs
                .iter()
                .filter(|(k, _)| k.as_str() != me)
                .map(|(k, v)| (k.clone(), *v))
                .collect()
        };

        let make_node = |id: &str, ep: quinn::Endpoint| {
            let peers = build_peers(id);
            let peer_ids: Vec<String> = peers.keys().cloned().collect();
            MeshNode {
                node_id: id.into(),
                endpoint: ep,
                state: Arc::new(Mutex::new(ConsensusState::new(id, peer_ids))),
                peer_addrs: peers,
            }
        };

        let node_alpha = make_node("node-alpha", ep_a);
        let node_beta = make_node("node-beta", ep_b);
        let node_gamma = make_node("node-gamma", ep_c);
        let node_delta = make_node("node-delta", ep_d);

        let _srv_a = node_alpha.spawn_server();
        let _srv_b = node_beta.spawn_server();
        let _srv_c = node_gamma.spawn_server();
        let _srv_d = node_delta.spawn_server();

        // node-alpha が mTLS QUIC 経由で選挙を実施し、4ノード BFT クラスタ (Quorum >= 3/4) の Leader に選出される
        let elected = node_alpha.campaign().await?;
        assert!(elected, "node-alpha must win BFT quorum over mTLS QUIC");

        let attacker_ip = Ipv4Addr::new(10, 0, 0, 99);
        assert_eq!(
            node_alpha.state.lock().await.check_xdp_packet(attacker_ip),
            XdpVerdict::Pass
        );

        // Leader (node-alpha) が「10.0.0.99 を Untrusted (XDP_DROP) に設定する」コマンドを提案し、
        // フォロワーからの Ed25519 QC 投票署名を集めて QuorumCertificate 付きでコミット！
        let log_idx = node_alpha
            .propose_and_commit(MeshCommand::set_trust(attacker_ip, false))
            .await?;
        assert_eq!(log_idx, 1);

        // 全4ノードでコミットインデックスが 1 に到達し、マルチシグ QC が保存され、XDP_DROP へ切り替わったことを証明！
        for (name, node) in [
            ("node-alpha", &node_alpha),
            ("node-beta", &node_beta),
            ("node-gamma", &node_gamma),
            ("node-delta", &node_delta),
        ] {
            let st = node.state.lock().await;
            assert_eq!(st.commit_index, 1, "{name} commit_index must be 1");
            assert_eq!(st.last_applied, 1, "{name} last_applied must be 1");
            assert!(
                st.commit_qcs.contains_key(&1),
                "{name} must store verified QuorumCertificate for index 1"
            );
            assert!(
                st.commit_qcs[&1].signatures.len() >= 3,
                "{name} QC must contain >= 3 distinct Ed25519 signatures"
            );
            assert_eq!(
                st.check_xdp_packet(attacker_ip),
                XdpVerdict::Drop,
                "{name} eBPF XDP filter must immediately XDP_DROP packets from {attacker_ip}"
            );
        }

        // 続けて「10.0.0.99 の隔離解除 (trusted = true)」を合意形成すると、全ノードで XDP_PASS に復帰することを証明
        let log_idx_2 = node_alpha
            .propose_and_commit(MeshCommand::set_trust(attacker_ip, true))
            .await?;
        assert_eq!(log_idx_2, 2);

        for node in [&node_alpha, &node_beta, &node_gamma, &node_delta] {
            let st = node.state.lock().await;
            assert_eq!(st.commit_index, 2);
            assert_eq!(st.check_xdp_packet(attacker_ip), XdpVerdict::Pass);
        }

        Ok(())
    }

    /// 証明3: ネットワーク分断（Split-Brain）耐性と再合流時のログ修復（Log Convergence）の証明
    #[test]
    fn proof_3_network_partition_split_brain_prevention_and_recovery() {
        let (mut alpha, mut beta, mut gamma, mut delta) = make_four_node_cluster();

        // Step 1: Term 1 で alpha がリーダーに選出される (3/4 票)
        let rv = alpha.start_signed_election();
        for peer in [&mut beta, &mut gamma, &mut delta] {
            let r = peer.handle_signed_rpc(rv.clone()).unwrap().unwrap();
            let sr = peer.sign_rpc(r);
            alpha.handle_signed_rpc(sr).unwrap();
        }
        assert_eq!(alpha.role, Role::Leader);

        // Step 2: 【ネットワーク分断発生】 alpha (1台) vs [beta, gamma, delta] (3台 = BFT Quorum)
        let suspect_ip = Ipv4Addr::new(10, 0, 0, 50);
        alpha
            .propose(MeshCommand::set_trust(suspect_ip, false))
            .unwrap();
        assert_eq!(
            alpha.commit_index, 0,
            "Isolated minority leader MUST NOT commit entry without 3/4 QC!"
        );
        assert_eq!(alpha.check_xdp_packet(suspect_ip), XdpVerdict::Pass);

        // Step 3: 多数派パーティション [beta, gamma, delta] で beta が Term 2 で当選
        let rv_beta = beta.start_signed_election();
        assert_eq!(beta.current_term, 2);
        for peer in [&mut gamma, &mut delta] {
            let r = peer.handle_signed_rpc(rv_beta.clone()).unwrap().unwrap();
            let sr = peer.sign_rpc(r);
            beta.handle_signed_rpc(sr).unwrap();
        }
        assert_eq!(beta.role, Role::Leader);

        // Step 4: 新リーダー beta (Term 2) が「10.0.0.200 を遮断」を提案し、gamma & delta と 3/4 マルチシグ QC でコミット
        let malicious_ip = Ipv4Addr::new(10, 0, 0, 200);
        beta.propose(MeshCommand::set_trust(malicious_ip, false))
            .unwrap();

        for _round in 0..2 {
            for (peer_name, peer) in [("node-gamma", &mut gamma), ("node-delta", &mut delta)] {
                let ae = beta.sign_rpc(beta.make_append_entries(peer_name).unwrap());
                let resp = peer.handle_signed_rpc(ae).unwrap().unwrap();
                let sresp = peer.sign_rpc(resp);
                beta.handle_signed_rpc(sresp).unwrap();
            }
        }
        assert_eq!(beta.commit_index, 1);
        assert_eq!(gamma.commit_index, 1);
        assert_eq!(delta.commit_index, 1);

        // Step 5: 【ネットワーク分断復旧（Heal）】
        let ae_heal = beta.sign_rpc(beta.make_append_entries("node-alpha").unwrap());
        let heal_resp = alpha.handle_signed_rpc(ae_heal).unwrap().unwrap();
        let sheal = alpha.sign_rpc(heal_resp);
        beta.handle_signed_rpc(sheal).unwrap();

        assert_eq!(alpha.role, Role::Follower);
        assert_eq!(alpha.current_term, 2);
        assert_eq!(alpha.commit_index, 1);
        assert_eq!(alpha.check_xdp_packet(suspect_ip), XdpVerdict::Pass);
        assert_eq!(alpha.check_xdp_packet(malicious_ip), XdpVerdict::Drop);
        assert_eq!(alpha.ebpf_policy_map, beta.ebpf_policy_map);
    }

    /// 証明4: カーネル eBPF ブリッジ (`EbpfBridge`) 連動と LSM ステルス GC シミュレーション発動の証明
    #[test]
    fn proof_4_ebpf_kernel_bridge_and_lsm_stealth_gc_zeroization() {
        let bridge_alpha = Arc::new(MockEbpfBridge::new_with_secret(vec![
            0xDE, 0xAD, 0xBE, 0xEF,
        ]));
        let bridge_beta = Arc::new(MockEbpfBridge::new_with_secret(vec![
            0xCA, 0xFE, 0xBA, 0xBE,
        ]));

        let (alpha_base, beta_base, mut gamma, mut delta) = make_four_node_cluster();
        let mut alpha = alpha_base.with_ebpf_bridge(bridge_alpha.clone());
        let mut beta = beta_base.with_ebpf_bridge(bridge_beta.clone());

        // Step 1: alpha がリーダーに選出される
        let rv = alpha.start_signed_election();
        for peer in [&mut beta, &mut gamma, &mut delta] {
            let resp = peer.handle_signed_rpc(rv.clone()).unwrap().unwrap();
            let sresp = peer.sign_rpc(resp);
            alpha.handle_signed_rpc(sresp).unwrap();
        }
        assert_eq!(alpha.role, Role::Leader);

        assert_eq!(alpha.check_lsm_socket_connect(), 0);
        assert_eq!(beta.check_lsm_socket_connect(), 0);
        assert_eq!(bridge_beta.sensitive_key_material.lock().unwrap().len(), 4);

        // Step 2: 侵害が疑われる `node-beta` に対して `SetNodeLockdown` を提案・合意
        let attacker_ip = Ipv4Addr::new(10, 0, 0, 77);
        alpha
            .propose(MeshCommand::set_trust(attacker_ip, false))
            .unwrap();
        alpha
            .propose(MeshCommand::set_lockdown("node-beta", true))
            .unwrap();

        for _round in 0..2 {
            for (peer_name, peer) in [
                ("node-beta", &mut beta),
                ("node-gamma", &mut gamma),
                ("node-delta", &mut delta),
            ] {
                let ae = alpha.sign_rpc(alpha.make_append_entries(peer_name).unwrap());
                let resp = peer.handle_signed_rpc(ae).unwrap().unwrap();
                let sresp = peer.sign_rpc(resp);
                alpha.handle_signed_rpc(sresp).unwrap();
            }
        }

        let kernel_peer_policy = *bridge_beta
            .kernel_peer_map
            .lock()
            .unwrap()
            .get(&u32::from(attacker_ip))
            .expect("PeerPolicy must be written into kernel eBPF map");
        assert_eq!(
            evaluate_peer_policy(Some(&kernel_peer_policy)),
            XdpVerdict::Drop
        );

        let kernel_lsm = *bridge_beta.kernel_lsm_policy.lock().unwrap();
        assert_eq!(evaluate_lsm_policy(Some(&kernel_lsm)), -1);
        assert_eq!(beta.check_lsm_socket_connect(), -1);
        assert!(bridge_beta
            .sensitive_key_material
            .lock()
            .unwrap()
            .is_empty());
        assert_eq!(bridge_beta.stealth_gc_purge_count(), 1);
        assert_eq!(alpha.check_lsm_socket_connect(), 0);
    }

    /// 証明5: ビザンチン障害耐性（Ed25519 暗号署名による偽造投票・なりすまし・ペイロード改ざんの完全阻止）の証明
    #[test]
    fn proof_5_byzantine_fault_tolerance_ed25519_signature_verification() {
        let (mut alpha, mut beta, mut gamma, mut delta) = make_four_node_cluster();
        let id_attacker = NodeCryptoIdentity::from_seed("node-mallory", &[99u8; 32]);

        // 正規フロー: node-alpha が Ed25519 署名付き RequestVote を送り、3/4 票で Leader 当選
        let signed_rv = alpha.start_signed_election();
        for peer in [&mut beta, &mut gamma, &mut delta] {
            let resp = peer.handle_signed_rpc(signed_rv.clone()).unwrap().unwrap();
            let signed_resp = peer.sign_rpc(resp);
            alpha.handle_signed_rpc(signed_resp).unwrap();
        }
        assert_eq!(alpha.role, Role::Leader);

        // --- 攻撃A: 侵害された node-gamma が、リーダー `node-alpha` になりすまして偽の AppendEntries を送信 ---
        let forged_ae = ConsensusRpc::AppendEntries {
            term: 99,
            leader_id: "node-alpha".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
            commit_qc: None,
        };
        let spoofed_envelope = gamma.sign_rpc(forged_ae.clone());
        let err_a = beta.handle_signed_rpc(spoofed_envelope).unwrap_err();
        assert!(err_a.contains("Identity Spoofing"));

        // --- 攻撃B: 部外者 `node-mallory` が `signer_id = node-alpha` と偽って自身の秘密鍵で署名 ---
        let mut fake_sig_envelope = id_attacker.sign_rpc(forged_ae);
        fake_sig_envelope.signer_id = "node-alpha".into();
        let err_b = beta.handle_signed_rpc(fake_sig_envelope).unwrap_err();
        assert!(err_b.contains("invalid Ed25519 signature"));

        // --- 攻撃C: 正規リーダー `node-alpha` が署名した遮断コマンドを中間者が `trusted = true` に1ビット改ざん ---
        let target_ip = Ipv4Addr::new(10, 0, 0, 88);
        alpha
            .propose(MeshCommand::set_trust(target_ip, false))
            .unwrap();
        let valid_ae = alpha.make_append_entries("node-beta").unwrap();
        let mut tampered_envelope: SignedConsensusRpc = alpha.sign_rpc(valid_ae);

        if let ConsensusRpc::AppendEntries {
            ref mut entries, ..
        } = tampered_envelope.rpc
        {
            entries[0].command = MeshCommand::set_trust(target_ip, true);
        }

        let err_c = beta.handle_signed_rpc(tampered_envelope).unwrap_err();
        assert!(err_c.contains("invalid Ed25519 signature or tampered RPC payload"));

        assert_eq!(beta.rejected_byzantine_packets, 3);
        assert_eq!(beta.current_term, 1);
        assert!(beta.ebpf_policy_map.is_empty());
    }

    /// 証明6: TUI アリーナにおける実 OS モック攻撃プロセスの起動と 4ノード QC 合意 + eBPF 駆逐の証明
    #[test]
    fn proof_6_tui_arena_mock_attack_process_spawn_and_extermination() -> Result<()> {
        let mut app = tui::TuiArenaApp::new();
        assert_eq!(app.nodes.len(), 4);
        assert_eq!(app.rogues.len(), 0);

        app.spawn_rogue_attacker()?;
        assert_eq!(app.rogues.len(), 1);
        let pid = app.rogues[0].pid;
        let attacker_ip = app.rogues[0].attacker_ip;
        assert!(pid > 0);
        assert_eq!(app.rogues[0].status, tui::RogueStatus::Infiltrating);

        app.on_tick();
        assert_eq!(
            app.nodes["node-beta"].check_xdp_packet(attacker_ip),
            XdpVerdict::Pass
        );

        app.inject_byzantine_spoof();
        for name in ["node-alpha", "node-beta", "node-gamma", "node-delta"] {
            assert_eq!(app.nodes[name].rejected_byzantine_packets, 1);
        }

        app.exterminate_rogues_by_consensus();

        assert_eq!(app.rogues[0].status, tui::RogueStatus::Exterminated);
        for name in ["node-alpha", "node-beta", "node-gamma", "node-delta"] {
            assert_eq!(
                app.nodes[name].check_xdp_packet(attacker_ip),
                XdpVerdict::Drop
            );
            assert!(app.nodes[name].commit_qcs.contains_key(&2));
        }
        assert_eq!(app.nodes["node-beta"].check_lsm_socket_connect(), -1);
        assert!(app.bridges["node-beta"]
            .sensitive_key_material
            .lock()
            .unwrap()
            .is_empty());

        Ok(())
    }

    /// 証明7 (Linux 実機 root 実行時のみ): 実 Linux カーネル (`veth-victim`) に `veil_xdp_kern.o` をアタッチし、
    /// `ns-veil-rogue (10.99.0.66)` からの実 ICMP/UDP パケットがコンセンサス合意によってカーネル NIC 層で 100% `XDP_DROP` されることを検証！
    #[cfg(target_os = "linux")]
    #[test]
    fn proof_7_live_linux_kernel_xdp_drop_on_veth() -> Result<()> {
        let obj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("assets/veil_xdp_kern.o");
        if !obj_path.exists() || !std::path::Path::new("/sys/class/net/veth-victim").exists() {
            return Ok(());
        }

        let bridge = match ebpf_bridge::AyaKernelBridge::load_and_attach(
            obj_path.to_str().unwrap(),
            "veth-victim",
        ) {
            Ok(b) => Arc::new(b),
            Err(_) => return Ok(()),
        };

        let attacker_ip = Ipv4Addr::new(10, 99, 0, 66);

        let ping_before = std::process::Command::new("ip")
            .args([
                "netns",
                "exec",
                "ns-veil-rogue",
                "ping",
                "-c",
                "1",
                "-W",
                "1",
                "10.99.0.1",
            ])
            .status()?;
        assert!(ping_before.success());

        bridge.sync_peer_policy(
            u32::from(attacker_ip),
            veil_common::PeerPolicy::new(false, 1),
        )?;

        let ping_blocked = std::process::Command::new("ip")
            .args([
                "netns",
                "exec",
                "ns-veil-rogue",
                "ping",
                "-c",
                "1",
                "-W",
                "1",
                "10.99.0.1",
            ])
            .status()?;
        assert!(!ping_blocked.success());

        let (_pass_cnt, drop_cnt) = bridge.read_xdp_counters().unwrap_or((0, 0));
        assert!(drop_cnt >= 1);

        bridge.sync_peer_policy(
            u32::from(attacker_ip),
            veil_common::PeerPolicy::new(true, 2),
        )?;
        let ping_restored = std::process::Command::new("ip")
            .args([
                "netns",
                "exec",
                "ns-veil-rogue",
                "ping",
                "-c",
                "1",
                "-W",
                "1",
                "10.99.0.1",
            ])
            .status()?;
        assert!(ping_restored.success());

        Ok(())
    }

    /// 証明8: リーダー自身が乗っ取られて二枚舌（Equivocation）や単独捏造コミットを働いた場合でも、
    /// 本番の `handle_signed_rpc` 経路上で `QuorumCertificate` ($2f+1 = 3/4$) 必須化とハッシュ整合性検証により阻止されることの証明！
    #[test]
    fn proof_8_leader_equivocation_and_multisig_quorum_certificate_defense() {
        let (alpha, mut beta, gamma, _delta) = make_four_node_cluster();
        assert_eq!(beta.quorum_size(), 3);

        // 1. リーダー alpha が正規エントリ (10.0.0.99 を遮断) を署名付き RPC で beta に送信
        let honest_entry = LogEntry {
            term: 1,
            index: 1,
            command: MeshCommand::set_trust(Ipv4Addr::new(10, 0, 0, 99), false),
        };
        let ae_honest = ConsensusRpc::AppendEntries {
            term: 1,
            leader_id: "node-alpha".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![honest_entry.clone()],
            leader_commit: 0,
            commit_qc: None,
        };
        let resp = beta
            .handle_signed_rpc(alpha.sign_rpc(ae_honest))
            .unwrap()
            .unwrap();
        if let ConsensusRpc::AppendEntriesResponse {
            success,
            entry_hash,
            ..
        } = resp
        {
            assert!(success);
            assert_eq!(entry_hash, honest_entry.digest());
        }

        // 2. 【攻撃1：リーダーの二枚舌 (Equivocation)】
        // 乗っ取られたリーダー alpha が、同じ (term=1, index=1) なのに中身を「味方ノード 10.0.0.1 を遮断」にすり替えた別エントリを送りつける！
        let conflicting_entry = LogEntry {
            term: 1,
            index: 1,
            command: MeshCommand::set_trust(Ipv4Addr::new(10, 0, 0, 1), false),
        };
        let ae_equivocate = ConsensusRpc::AppendEntries {
            term: 1,
            leader_id: "node-alpha".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![conflicting_entry],
            leader_commit: 0,
            commit_qc: None,
        };
        let resp_eq = beta
            .handle_signed_rpc(alpha.sign_rpc(ae_equivocate))
            .unwrap()
            .unwrap();
        if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_eq {
            assert!(
                !success,
                "Follower MUST reject conflicting log entry at same (term, index)!"
            );
        }
        assert_eq!(beta.rejected_byzantine_packets, 1);

        // 3. 【攻撃2：リーダーが commit_qc: None のまま leader_commit: 1 を送りつけて単独コミットを強要】
        let ae_no_qc = ConsensusRpc::AppendEntries {
            term: 1,
            leader_id: "node-alpha".into(),
            prev_log_index: 1,
            prev_log_term: 1,
            entries: vec![],
            leader_commit: 1,
            commit_qc: None,
        };
        let resp_no_qc = beta
            .handle_signed_rpc(alpha.sign_rpc(ae_no_qc))
            .unwrap()
            .unwrap();
        if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_no_qc {
            assert!(
                !success,
                "Follower MUST reject leader_commit advancement when commit_qc is None!"
            );
        }
        assert_eq!(beta.commit_index, 0);
        assert_eq!(beta.rejected_byzantine_packets, 2);

        // 4. 【攻撃3：リーダー単独での捏造コミット (1台分の署名しかない偽 QC を添付して AppendEntries を送信)】
        let hash = honest_entry.digest();
        let unilateral_qc = QuorumCertificate {
            term: 1,
            index: 1,
            entry_hash: hash,
            signatures: BTreeMap::from([(
                "node-alpha".into(),
                alpha.identity.sign_qc_vote(1, 1, &hash),
            )]),
        };
        let ae_unilateral_qc = ConsensusRpc::AppendEntries {
            term: 1,
            leader_id: "node-alpha".into(),
            prev_log_index: 1,
            prev_log_term: 1,
            entries: vec![],
            leader_commit: 1,
            commit_qc: Some(unilateral_qc),
        };
        let resp_unilateral = beta
            .handle_signed_rpc(alpha.sign_rpc(ae_unilateral_qc))
            .unwrap()
            .unwrap();
        if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_unilateral {
            assert!(
                !success,
                "Follower MUST reject QuorumCertificate with fewer than 2f+1 (3) signatures!"
            );
        }
        assert_eq!(beta.commit_index, 0);
        assert_eq!(beta.rejected_byzantine_packets, 3);

        // 5. 【正規のマルチシグ QC (alpha, beta, gamma の 3/4 署名) による安全な合意コミット】
        let valid_qc = QuorumCertificate {
            term: 1,
            index: 1,
            entry_hash: hash,
            signatures: BTreeMap::from([
                (
                    "node-alpha".into(),
                    alpha.identity.sign_qc_vote(1, 1, &hash),
                ),
                ("node-beta".into(), beta.identity.sign_qc_vote(1, 1, &hash)),
                (
                    "node-gamma".into(),
                    gamma.identity.sign_qc_vote(1, 1, &hash),
                ),
            ]),
        };
        let ae_valid_commit = ConsensusRpc::AppendEntries {
            term: 1,
            leader_id: "node-alpha".into(),
            prev_log_index: 1,
            prev_log_term: 1,
            entries: vec![],
            leader_commit: 1,
            commit_qc: Some(valid_qc),
        };
        let resp_valid = beta
            .handle_signed_rpc(alpha.sign_rpc(ae_valid_commit))
            .unwrap()
            .unwrap();
        if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_valid {
            assert!(success);
        }
        assert_eq!(beta.commit_index, 1);
    }

    /// 証明9 (監査指摘 #1 & #2 回帰テスト):
    /// 1. 信頼されていない自己署名 TLS 証明書を持つ外部 QUIC クライアントは mTLS ハンドシェイク段階で遮断されること
    /// 2. ネットワーク経路 (`handle_connection`) で署名偽造された高 Term の `AppendEntries (SetNodeLockdown)` を送っても弾かれ、ロックダウンされないこと
    #[tokio::test]
    async fn proof_9_network_quic_mtls_and_signed_rpc_rejects_untrusted_tls_and_forged_wire_packets(
    ) -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let ep_victim = make_quic_endpoint("127.0.0.1:0".parse()?)?;
        let victim_addr = ep_victim.local_addr()?;
        let victim_node = MeshNode {
            node_id: "node-beta".into(),
            endpoint: ep_victim,
            state: Arc::new(Mutex::new(ConsensusState::new(
                "node-beta",
                vec![
                    "node-alpha".into(),
                    "node-gamma".into(),
                    "node-delta".into(),
                ],
            ))),
            peer_addrs: BTreeMap::new(),
        };
        let _srv = victim_node.spawn_server();

        // (A) 別の独立した CA (攻撃者の自己署名 PKI) から作られた QUIC エンドポイントで接続を試みる -> mTLS で拒絶される！
        let rogue_pki = ClusterMtlsPki::generate()?;
        let ep_rogue_tls = rogue_pki.make_endpoint("127.0.0.1:0".parse()?)?;
        let rogue_conn_res = tokio::time::timeout(
            Duration::from_millis(250),
            ep_rogue_tls.connect(victim_addr, "veil.mesh")?,
        )
        .await;
        if let Ok(Ok(conn)) = rogue_conn_res {
            // TLS 1.3 ClientAuth 検証失敗によりストリーム通信は必ずエラーになる
            let bi_res = conn.open_bi().await;
            if let Ok((mut s, mut r)) = bi_res {
                let _ = s.write_all(b"test").await;
                let _ = s.finish();
                assert!(
                    r.read_to_end(1024).await.is_err(),
                    "Untrusted mTLS client certificate MUST be rejected by server!"
                );
            }
        }

        // (B) 仮に mTLS ネットワーク内部からでも、偽造署名で高 Term の Lockdown AppendEntries を送りつけた場合 -> handle_signed_rpc で弾かれる！
        let ep_insider = make_quic_endpoint("127.0.0.1:0".parse()?)?;
        let attacker_identity = NodeCryptoIdentity::from_seed("mallory", &[0x77; 32]);
        let forged_lockdown_rpc = ConsensusRpc::AppendEntries {
            term: 500,
            leader_id: "node-alpha".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![LogEntry {
                term: 500,
                index: 1,
                command: MeshCommand::set_lockdown("*", true),
            }],
            leader_commit: 1,
            commit_qc: None,
        };
        let mut forged_signed = attacker_identity.sign_rpc(forged_lockdown_rpc);
        forged_signed.signer_id = "node-alpha".into();

        let _ = send_rpc_over_quic(&ep_insider, victim_addr, forged_signed).await;

        let st = victim_node.state.lock().await;
        assert_eq!(
            st.commit_index, 0,
            "Network path MUST NOT commit forged AppendEntries!"
        );
        assert_eq!(
            st.check_lsm_socket_connect(),
            0,
            "Node MUST NOT be locked down by forged network packet!"
        );
        assert_eq!(st.rejected_byzantine_packets, 1);

        Ok(())
    }

    /// 証明10 (監査指摘 #3 & #4 回帰テスト):
    /// 裏切りフォロワー1台が嘘の `match_index` や偽の `entry_hash` / `qc_vote_signature` を返しても、
    /// リーダー + 裏切りフォロワーの2台だけでは絶対にコミットが成立しないことを証明！
    #[test]
    fn proof_10_lying_follower_match_index_cannot_trigger_two_node_commit() {
        let (mut alpha, mut beta, mut gamma, mut delta) = make_four_node_cluster();

        // alpha を Term 1 のリーダーに選出
        let rv = alpha.start_signed_election();
        for peer in [&mut beta, &mut gamma, &mut delta] {
            let r = peer.handle_signed_rpc(rv.clone()).unwrap().unwrap();
            let sr = peer.sign_rpc(r);
            alpha.handle_signed_rpc(sr).unwrap();
        }
        assert_eq!(alpha.role, Role::Leader);

        // alpha がエントリ1を提案（この時点で alpha 自身の QC 署名 1/4 のみ）
        alpha
            .propose(MeshCommand::set_trust(Ipv4Addr::new(10, 0, 0, 55), false))
            .unwrap();
        assert_eq!(alpha.commit_index, 0);

        // 裏切りフォロワー gamma が、エントリを受け取っていないのに「match_index=1, 適当なハッシュ/署名」の嘘応答を返す
        let lying_resp = ConsensusRpc::AppendEntriesResponse {
            term: 1,
            follower_id: "node-gamma".into(),
            success: true,
            match_index: 1,
            entry_hash: [0xFF; 32],
            qc_vote_signature: vec![0x11; 64],
        };
        alpha.handle_signed_rpc(gamma.sign_rpc(lying_resp)).unwrap();
        assert_eq!(
            alpha.commit_index, 0,
            "Lying match_index with fake hash/sig MUST be rejected!"
        );
        assert_eq!(alpha.rejected_byzantine_packets, 1);

        // さらに gamma が正しいハッシュと QC 署名を返したとしても、alpha + gamma の 2/4 票では BFT Quorum (3/4) に満たないためコミットされない！
        let ae_gamma = alpha.sign_rpc(alpha.make_append_entries("node-gamma").unwrap());
        let valid_gamma_resp = gamma.handle_signed_rpc(ae_gamma).unwrap().unwrap();
        alpha
            .handle_signed_rpc(gamma.sign_rpc(valid_gamma_resp))
            .unwrap();
        assert_eq!(
            alpha.commit_index, 0,
            "2 nodes (leader + 1 follower) out of 4 MUST NOT be enough to commit!"
        );

        // 3台目 (beta) の検証済み QC 投票署名が揃って初めて 3/4 BFT Quorum が成立しコミットされる！
        let ae_beta = alpha.sign_rpc(alpha.make_append_entries("node-beta").unwrap());
        let valid_beta_resp = beta.handle_signed_rpc(ae_beta).unwrap().unwrap();
        alpha
            .handle_signed_rpc(beta.sign_rpc(valid_beta_resp))
            .unwrap();
        assert_eq!(
            alpha.commit_index, 1,
            "3 verified Ed25519 QC signatures out of 4 nodes must succeed!"
        );
    }

    /// 証明11 (監査指摘 #5 回帰テスト):
    /// 1. `index = 0` の不正な `LogEntry` を送りつけてもフォロワーが Panic せず安全に拒否すること
    /// 2. `term = u64::MAX` の投票要求を送りつけても `MAX_SAFE_TERM` 境界チェックで拒否され、Term が上限に張り付かないこと
    #[test]
    fn proof_11_boundary_validation_index_zero_and_term_max_no_panic_or_saturation() {
        let (alpha, mut beta, _gamma, _delta) = make_four_node_cluster();

        // (A) index = 0 の不正な LogEntry を含む AppendEntries を送信 -> Panic せず拒否される！
        let bad_index_zero_ae = ConsensusRpc::AppendEntries {
            term: 1,
            leader_id: "node-alpha".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![LogEntry {
                term: 1,
                index: 0, // 不正な 0-index エントリ
                command: MeshCommand::NoOp,
            }],
            leader_commit: 0,
            commit_qc: None,
        };
        let resp = beta
            .handle_signed_rpc(alpha.sign_rpc(bad_index_zero_ae))
            .unwrap()
            .unwrap();
        if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp {
            assert!(
                !success,
                "Entry with index=0 MUST be rejected safely without panicking!"
            );
        }
        assert_eq!(beta.rejected_byzantine_packets, 1);
        assert!(beta.log.is_empty());

        // (B) term = u64::MAX の RequestVote を送信 -> MAX_SAFE_TERM 境界で弾かれ、beta の Term は 0 のまま保護される！
        let term_max_rv = ConsensusRpc::RequestVote {
            term: u64::MAX,
            candidate_id: "node-alpha".into(),
            last_log_index: 0,
            last_log_term: 0,
        };
        let err = beta
            .handle_signed_rpc(alpha.sign_rpc(term_max_rv))
            .unwrap_err();
        assert!(
            err.contains("MAX_SAFE_TERM"),
            "term=u64::MAX MUST be rejected at handle_signed_rpc boundary!"
        );
        assert_eq!(
            beta.current_term, 0,
            "current_term MUST NOT saturate to u64::MAX!"
        );
        assert_eq!(beta.rejected_byzantine_packets, 2);
    }
}
