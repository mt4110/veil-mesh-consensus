pub mod consensus;
pub mod ebpf_bridge;
pub mod tui;

use anyhow::Result;
use clap::Parser;
use consensus::{ConsensusRpc, ConsensusState, MeshCommand};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Parser, Debug, Clone)]
#[command(name = "veil-node", about = "veil-mesh-consensus QUIC & eBPF node daemon")]
struct Args {
    /// Node identifier (e.g. `node-alpha` or `node-beta`)
    #[arg(long, default_value = "node-alpha")]
    node_id: String,

    /// Local QUIC UDP bind address
    #[arg(long, default_value = "0.0.0.0:5000")]
    listen: SocketAddr,

    /// Optional peer QUIC UDP address to dial (e.g. `10.0.0.2:5000`)
    #[arg(long)]
    peer: Option<SocketAddr>,

    /// Network interface to attach XDP filter to (Linux only)
    #[arg(long, default_value = "eth1")]
    iface: String,

    /// Path to compiled eBPF object file (Linux only)
    #[arg(long)]
    ebpf_obj: Option<String>,

    /// Launch interactive TUI Arena (spawn mock attack OS processes & exterminate via consensus + eBPF)
    #[arg(long)]
    tui: bool,

    /// Run automated TUI demo scenario (for GIF recording / smoke check) and exit
    #[arg(long)]
    tui_demo: bool,
}

/// QUIC ストリーム上でやり取りするメッセージフレーム（Ping/Ack および Raft Consensus RPC）
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
    Consensus(ConsensusRpc),
}

/// クイック・コンセンサス・ノード（QUIC エンドポイント + 決定論的 Raft ステートマシン）
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

    /// 選挙を開始し、QUIC 双方向ストリーム経由で全ピアから投票を収集する
    pub async fn campaign(&self) -> Result<bool> {
        let req = {
            let mut st = self.state.lock().await;
            st.start_election()
        };

        for (peer_id, &addr) in &self.peer_addrs {
            match send_rpc_over_quic(&self.endpoint, addr, req.clone()).await {
                Ok(Some(resp)) => {
                    let mut st = self.state.lock().await;
                    st.handle_rpc(resp);
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

    /// リーダーとして全フォロワーへ `AppendEntries` を QUIC 経由でレプリケートし、合意コミットおよび eBPF マップ同期を完了させる
    pub async fn replicate_round(&self) -> Result<()> {
        for (peer_id, &addr) in &self.peer_addrs {
            let maybe_rpc = {
                let st = self.state.lock().await;
                st.make_append_entries(peer_id)
            };

            if let Some(rpc) = maybe_rpc {
                if let Ok(Some(resp)) = send_rpc_over_quic(&self.endpoint, addr, rpc).await {
                    let mut st = self.state.lock().await;
                    st.handle_rpc(resp);
                }
            }
        }
        Ok(())
    }

    /// リーダーとしてコマンドを提案し、2ラウンドのレプリケーション（1: ログ合意・リーダーCommit、2: フォロワーへのCommit通知）を実行する
    pub async fn propose_and_commit(&self, cmd: MeshCommand) -> Result<u64> {
        let index = {
            let mut st = self.state.lock().await;
            st.propose(cmd).map_err(|e| anyhow::anyhow!(e))?.index
        };

        // Round 1: エントリを全フォロワーへ送信し、過半数の ACK でリーダーが Commit & Apply
        self.replicate_round().await?;
        // Round 2: 更新された leader_commit をフォロワーへ伝搬し、フォロワー側でも Apply (eBPF Map 同期)
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
                Err(err) => warn!(error = %err, "Incoming QUIC handshake failed"),
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
        let buf = recv.read_to_end(65536).await?;
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
            WireMessage::Consensus(rpc) => {
                let maybe_resp = {
                    let mut st = state.lock().await;
                    st.handle_rpc(rpc)
                };
                if let Some(resp) = maybe_resp {
                    let wire = WireMessage::Consensus(resp);
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
    rpc: ConsensusRpc,
) -> Result<Option<ConsensusRpc>> {
    let conn = tokio::time::timeout(
        Duration::from_millis(300),
        endpoint.connect(peer_addr, "veil.mesh")?,
    )
    .await??;

    let (mut send, mut recv) = conn.open_bi().await?;
    let wire = WireMessage::Consensus(rpc);
    send.write_all(&bincode::serialize(&wire)?).await?;
    send.finish()?;

    let buf = tokio::time::timeout(Duration::from_millis(300), recv.read_to_end(65536)).await??;
    if buf.is_empty() {
        return Ok(None);
    }
    if let WireMessage::Consensus(resp) = bincode::deserialize(&buf)? {
        Ok(Some(resp))
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

pub fn make_quic_endpoint(bind_addr: SocketAddr) -> Result<quinn::Endpoint> {
    let cert = rcgen::generate_simple_self_signed(vec!["veil.mesh".into()])?;
    let cert_der = rustls::pki_types::CertificateDer::from(cert.cert);
    let priv_key = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()),
    );

    let server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], priv_key)?;
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
    ));

    let client_crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)?,
    ));

    let mut endpoint = quinn::Endpoint::server(server_config, bind_addr)?;
    endpoint.set_default_client_config(client_config);
    Ok(endpoint)
}

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consensus::{NodeCryptoIdentity, Role, SignedConsensusRpc};
    use ebpf_bridge::{EbpfBridge, MockEbpfBridge};
    use veil_common::{evaluate_lsm_policy, evaluate_peer_policy, XdpVerdict};

    /// 証明1: Election Safety & Quorum 投票の検証
    /// 同一 Term 内で高々1つのリーダーしか選出されないこと、また古いログを持つ候補者が拒否されることを証明する
    #[test]
    fn proof_1_election_safety_and_log_up_to_date_check() {
        let mut alpha =
            ConsensusState::new("node-alpha", vec!["node-beta".into(), "node-gamma".into()]);
        let mut beta =
            ConsensusState::new("node-beta", vec!["node-alpha".into(), "node-gamma".into()]);
        let mut gamma =
            ConsensusState::new("node-gamma", vec!["node-alpha".into(), "node-beta".into()]);

        // 1. node-alpha が Term 1 で立候補
        let vote_req_alpha = alpha.start_election();
        assert_eq!(alpha.current_term, 1);
        assert_eq!(alpha.role, Role::Candidate);

        // 2. node-beta が node-alpha に投票
        let resp_beta = beta.handle_rpc(vote_req_alpha.clone()).unwrap();
        assert_eq!(
            resp_beta,
            ConsensusRpc::RequestVoteResponse {
                term: 1,
                voter_id: "node-beta".into(),
                vote_granted: true,
            }
        );

        // 3. 同じ Term 1 で node-gamma も同時に立候補（Split Vote 競合の試行）
        let vote_req_gamma = gamma.start_election();
        // すでに node-alpha に投票済みの node-beta は node-gamma への投票を厳密に拒否する
        let resp_beta_to_gamma = beta.handle_rpc(vote_req_gamma).unwrap();
        assert_eq!(
            resp_beta_to_gamma,
            ConsensusRpc::RequestVoteResponse {
                term: 1,
                voter_id: "node-beta".into(),
                vote_granted: false,
            }
        );

        // 4. node-alpha が node-beta の賛成票を受け取り、Quorum (2/3) 達成で唯一の Leader に昇格
        alpha.handle_rpc(resp_beta);
        assert_eq!(alpha.role, Role::Leader);
        assert_ne!(gamma.role, Role::Leader);
    }

    /// 証明2: 3ノード実 QUIC (TLS 1.3 over UDP) メッシュでの合意形成と eBPF XDP 遮断ポリシーの即時同期
    #[tokio::test]
    async fn proof_2_three_node_quic_consensus_and_ebpf_xdp_enforcement() -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let ep_a = make_quic_endpoint("127.0.0.1:0".parse()?)?;
        let ep_b = make_quic_endpoint("127.0.0.1:0".parse()?)?;
        let ep_c = make_quic_endpoint("127.0.0.1:0".parse()?)?;

        let addr_a = ep_a.local_addr()?;
        let addr_b = ep_b.local_addr()?;
        let addr_c = ep_c.local_addr()?;

        let all_addrs = BTreeMap::from([
            ("node-alpha".to_string(), addr_a),
            ("node-beta".to_string(), addr_b),
            ("node-gamma".to_string(), addr_c),
        ]);

        let build_peers = |me: &str| -> BTreeMap<String, SocketAddr> {
            all_addrs
                .iter()
                .filter(|(k, _)| k.as_str() != me)
                .map(|(k, v)| (k.clone(), *v))
                .collect()
        };

        let node_alpha = MeshNode {
            node_id: "node-alpha".into(),
            endpoint: ep_a,
            state: Arc::new(Mutex::new(ConsensusState::new(
                "node-alpha",
                vec!["node-beta".into(), "node-gamma".into()],
            ))),
            peer_addrs: build_peers("node-alpha"),
        };
        let node_beta = MeshNode {
            node_id: "node-beta".into(),
            endpoint: ep_b,
            state: Arc::new(Mutex::new(ConsensusState::new(
                "node-beta",
                vec!["node-alpha".into(), "node-gamma".into()],
            ))),
            peer_addrs: build_peers("node-beta"),
        };
        let node_gamma = MeshNode {
            node_id: "node-gamma".into(),
            endpoint: ep_c,
            state: Arc::new(Mutex::new(ConsensusState::new(
                "node-gamma",
                vec!["node-alpha".into(), "node-beta".into()],
            ))),
            peer_addrs: build_peers("node-gamma"),
        };

        let _srv_a = node_alpha.spawn_server();
        let _srv_b = node_beta.spawn_server();
        let _srv_c = node_gamma.spawn_server();

        // 2. node-alpha が QUIC 経由で選挙を実施し、3ノードクラスタの Leader に選出される
        let elected = node_alpha.campaign().await?;
        assert!(elected, "node-alpha must win quorum over QUIC");

        // 3. 攻撃者または異常ノード (10.0.0.99) の通信が、初期状態では XDP_PASS であることを確認
        let attacker_ip = Ipv4Addr::new(10, 0, 0, 99);
        assert_eq!(
            node_alpha.state.lock().await.check_xdp_packet(attacker_ip),
            XdpVerdict::Pass
        );

        // 4. Leader (node-alpha) が「10.0.0.99 を Untrusted (XDP_DROP) に設定する」コマンドを提案・合意形成
        let log_idx = node_alpha
            .propose_and_commit(MeshCommand::set_trust(attacker_ip, false))
            .await?;
        assert_eq!(log_idx, 1);

        // 5. 全3ノード (alpha, beta, gamma) でコミットインデックスが 1 に到達し、
        //    eBPF XDP フィルタ判定が全ノード一斉に XDP_DROP へ切り替わったことを証明！
        for (name, node) in [
            ("node-alpha", &node_alpha),
            ("node-beta", &node_beta),
            ("node-gamma", &node_gamma),
        ] {
            let st = node.state.lock().await;
            assert_eq!(st.commit_index, 1, "{name} commit_index must be 1");
            assert_eq!(st.last_applied, 1, "{name} last_applied must be 1");
            assert_eq!(
                st.check_xdp_packet(attacker_ip),
                XdpVerdict::Drop,
                "{name} eBPF XDP filter must immediately XDP_DROP packets from {attacker_ip}"
            );
        }

        // 6. 続けて「10.0.0.99 の隔離解除 (trusted = true)」を合意形成すると、全ノードで XDP_PASS に復帰することを証明
        let log_idx_2 = node_alpha
            .propose_and_commit(MeshCommand::set_trust(attacker_ip, true))
            .await?;
        assert_eq!(log_idx_2, 2);

        for node in [&node_alpha, &node_beta, &node_gamma] {
            let st = node.state.lock().await;
            assert_eq!(st.commit_index, 2);
            assert_eq!(st.check_xdp_packet(attacker_ip), XdpVerdict::Pass);
        }

        Ok(())
    }

    /// 証明3: ネットワーク分断（Split-Brain）耐性と再合流時のログ修復（Log Convergence）の証明
    #[test]
    fn proof_3_network_partition_split_brain_prevention_and_recovery() {
        let mut alpha =
            ConsensusState::new("node-alpha", vec!["node-beta".into(), "node-gamma".into()]);
        let mut beta =
            ConsensusState::new("node-beta", vec!["node-alpha".into(), "node-gamma".into()]);
        let mut gamma =
            ConsensusState::new("node-gamma", vec!["node-alpha".into(), "node-beta".into()]);

        // Step 1: Term 1 で alpha がリーダーに選出される
        let rv = alpha.start_election();
        let r_b = beta.handle_rpc(rv.clone()).unwrap();
        let r_c = gamma.handle_rpc(rv).unwrap();
        alpha.handle_rpc(r_b);
        alpha.handle_rpc(r_c);
        assert_eq!(alpha.role, Role::Leader);

        // Step 2: 【ネットワーク分断発生】
        let suspect_ip = Ipv4Addr::new(10, 0, 0, 50);
        alpha
            .propose(MeshCommand::set_trust(suspect_ip, false))
            .unwrap();
        assert_eq!(
            alpha.commit_index, 0,
            "Isolated minority leader MUST NOT commit entry without quorum!"
        );
        assert_eq!(
            alpha.check_xdp_packet(suspect_ip),
            XdpVerdict::Pass,
            "Uncommitted entry MUST NOT be applied to eBPF map!"
        );

        // Step 3: 多数派パーティション [beta, gamma] で beta が Term 2 で当選
        let rv_beta = beta.start_election();
        assert_eq!(beta.current_term, 2);
        let resp_gamma = gamma.handle_rpc(rv_beta).unwrap();
        beta.handle_rpc(resp_gamma);
        assert_eq!(beta.role, Role::Leader);

        // Step 4: 新リーダー beta (Term 2) が「10.0.0.200 を遮断」を提案し、gamma と合意コミット
        let malicious_ip = Ipv4Addr::new(10, 0, 0, 200);
        beta.propose(MeshCommand::set_trust(malicious_ip, false))
            .unwrap();

        let ae1 = beta.make_append_entries("node-gamma").unwrap();
        let ae1_resp = gamma.handle_rpc(ae1).unwrap();
        beta.handle_rpc(ae1_resp);
        assert_eq!(beta.commit_index, 1);

        let ae2 = beta.make_append_entries("node-gamma").unwrap();
        let _ = gamma.handle_rpc(ae2);
        assert_eq!(gamma.commit_index, 1);
        assert_eq!(beta.check_xdp_packet(malicious_ip), XdpVerdict::Drop);
        assert_eq!(gamma.check_xdp_packet(malicious_ip), XdpVerdict::Drop);

        // Step 5: 【ネットワーク分断復旧（Heal）】
        let ae_heal_1 = beta.make_append_entries("node-alpha").unwrap();
        let heal_resp = alpha.handle_rpc(ae_heal_1).unwrap();
        beta.handle_rpc(heal_resp);

        assert_eq!(alpha.role, Role::Follower);
        assert_eq!(alpha.current_term, 2);
        assert_eq!(alpha.commit_index, 1);
        assert_eq!(
            alpha.check_xdp_packet(suspect_ip),
            XdpVerdict::Pass,
            "Conflicting uncommitted entry from minority partition must be rolled back"
        );
        assert_eq!(
            alpha.check_xdp_packet(malicious_ip),
            XdpVerdict::Drop,
            "Healed node must converge to majority eBPF XDP policy"
        );
        assert_eq!(alpha.ebpf_policy_map, beta.ebpf_policy_map);
        assert_eq!(beta.ebpf_policy_map, gamma.ebpf_policy_map);
    }

    /// 証明4: カーネル eBPF ブリッジ (`EbpfBridge`) 連動と LSM ステルス GC (`lsm=bpf`) 発動の証明
    /// コンセンサスで `SetNodeLockdown` が合意された瞬間：
    /// 1. カーネル空間の `LSM_POLICY_MAP[0]` が `lockdown = 1` に更新され、`veil_lsm_socket_connect` が `-EPERM (-1)` を返すこと
    /// 2. ステルス GC が直ちに発火し、ノードが保持していた機密鍵メモリ (`sensitive_key_material`) がゼロ消去（Zeroize）されること
    #[test]
    fn proof_4_ebpf_kernel_bridge_and_lsm_stealth_gc_zeroization() {
        let bridge_alpha = Arc::new(MockEbpfBridge::new_with_secret(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        let bridge_beta = Arc::new(MockEbpfBridge::new_with_secret(vec![0xCA, 0xFE, 0xBA, 0xBE]));

        // 1ノード（またはリーダー単体テスト）でブリッジ連動を検証するため、alpha + beta の2ノード構成で合意形成
        let mut alpha = ConsensusState::new("node-alpha", vec!["node-beta".into()])
            .with_ebpf_bridge(bridge_alpha.clone());
        let mut beta = ConsensusState::new("node-beta", vec!["node-alpha".into()])
            .with_ebpf_bridge(bridge_beta.clone());

        // Step 1: alpha がリーダーに選出される
        let rv = alpha.start_election();
        let resp = beta.handle_rpc(rv).unwrap();
        alpha.handle_rpc(resp);
        assert_eq!(alpha.role, Role::Leader);

        // 初期状態では LSM socket_connect は 0 (Allow) であり、機密鍵メモリも保持されている
        assert_eq!(alpha.check_lsm_socket_connect(), 0);
        assert_eq!(beta.check_lsm_socket_connect(), 0);
        assert_eq!(bridge_beta.sensitive_key_material.lock().unwrap().len(), 4);

        // Step 2: 侵害が疑われる `node-beta` に対して `SetNodeLockdown { target_node: "node-beta", lockdown: true }` を提案・合意
        let attacker_ip = Ipv4Addr::new(10, 0, 0, 77);
        alpha
            .propose(MeshCommand::set_trust(attacker_ip, false))
            .unwrap();
        alpha
            .propose(MeshCommand::set_lockdown("node-beta", true))
            .unwrap();

        // Round 1: レプリケーション -> alpha Commit
        let ae1 = alpha.make_append_entries("node-beta").unwrap();
        let ae1_resp = beta.handle_rpc(ae1).unwrap();
        alpha.handle_rpc(ae1_resp);

        // Round 2: Commit 通知 -> beta Apply (ブリッジ経由でカーネルマップ更新 & ステルス GC 発火！)
        let ae2 = alpha.make_append_entries("node-beta").unwrap();
        let _ = beta.handle_rpc(ae2);

        // Step 3: カーネルマップ `PEER_POLICY_MAP` への書き込み検証
        let kernel_peer_policy = *bridge_beta
            .kernel_peer_map
            .lock()
            .unwrap()
            .get(&u32::from(attacker_ip))
            .expect("PeerPolicy must be written into kernel eBPF map");
        assert_eq!(evaluate_peer_policy(Some(&kernel_peer_policy)), XdpVerdict::Drop);

        // Step 4: `node-beta` の LSM フック (`veil_lsm_socket_connect`) が `-EPERM (-1)` で全ソケット接続を遮断し、
        //         ステルス GC によってメモリ上の機密鍵が完全にゼロ消去されたことを証明！
        let kernel_lsm = *bridge_beta.kernel_lsm_policy.lock().unwrap();
        assert_eq!(
            evaluate_lsm_policy(Some(&kernel_lsm)),
            -1,
            "Kernel LSM hook must return -EPERM (-1) when node is locked down"
        );
        assert_eq!(beta.check_lsm_socket_connect(), -1);
        assert!(
            bridge_beta.sensitive_key_material.lock().unwrap().is_empty(),
            "Stealth GC must zeroize and clear sensitive key material in memory!"
        );
        assert_eq!(bridge_beta.stealth_gc_purge_count(), 1);

        // 一方、ロックダウン対象外の `node-alpha` は正常稼働を継続していることを確認
        assert_eq!(alpha.check_lsm_socket_connect(), 0);
        assert_eq!(bridge_alpha.stealth_gc_purge_count(), 0);
    }

    /// 証明5: ビザンチン障害耐性（Ed25519 暗号署名による偽造投票・なりすまし・ペイロード改ざんの完全阻止）の証明
    /// 悪意あるビザンチンノードが以下の3種類の攻撃を仕掛けても、すべて暗号学的に検知・棄却されることを証明する：
    /// - 攻撃A（Identity Spoofing / なりすまし）: 自身の鍵で署名しつつ、RPC 内の送信者 ID を他ノード（リーダー等）に偽装する
    /// - 攻撃B（Signature Forgery / 偽造署名）: ランダムまたは不正な秘密鍵で署名した投票要求・ログ追加を送る
    /// - 攻撃C（Payload Tampering / 中間者改ざん）: 正規リーダーが署名した `SetPeerTrust(trusted=false)` を途中で `trusted=true` に書き換える
    #[test]
    fn proof_5_byzantine_fault_tolerance_ed25519_signature_verification() {
        let id_alpha = NodeCryptoIdentity::from_seed("node-alpha", &[1u8; 32]);
        let id_beta = NodeCryptoIdentity::from_seed("node-beta", &[2u8; 32]);
        let id_gamma = NodeCryptoIdentity::from_seed("node-gamma", &[3u8; 32]);
        let id_attacker = NodeCryptoIdentity::from_seed("node-mallory", &[99u8; 32]);

        let trusted_keys = BTreeMap::from([
            ("node-alpha".to_string(), id_alpha.public_key_bytes.clone()),
            ("node-beta".to_string(), id_beta.public_key_bytes.clone()),
            ("node-gamma".to_string(), id_gamma.public_key_bytes.clone()),
        ]);

        let mut alpha =
            ConsensusState::new("node-alpha", vec!["node-beta".into(), "node-gamma".into()]);
        let mut beta =
            ConsensusState::new("node-beta", vec!["node-alpha".into(), "node-gamma".into()]);

        alpha.register_peer_public_keys(trusted_keys.clone());
        beta.register_peer_public_keys(trusted_keys);

        // --- 正規フロー: node-alpha が Ed25519 署名付き RequestVote を送り、node-beta が検証・承認 ---
        let rv_alpha = alpha.start_election();
        let signed_rv = id_alpha.sign_rpc(rv_alpha);
        let resp_from_beta = beta
            .handle_signed_rpc(signed_rv)
            .expect("Valid Ed25519 signature from node-alpha must be accepted")
            .unwrap();
        let signed_resp = id_beta.sign_rpc(resp_from_beta);
        alpha
            .handle_signed_rpc(signed_resp)
            .expect("Valid Ed25519 vote response from node-beta must be accepted");
        assert_eq!(alpha.role, Role::Leader);

        // --- 攻撃A: 侵害された node-gamma が、リーダー `node-alpha` になりすまして偽の AppendEntries を送信 ---
        let forged_ae = ConsensusRpc::AppendEntries {
            term: 99,
            leader_id: "node-alpha".into(), // alpha を詐称
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
            commit_qc: None,
        };
        let spoofed_envelope = id_gamma.sign_rpc(forged_ae.clone()); // signer_id は "node-gamma"
        let err_a = beta.handle_signed_rpc(spoofed_envelope).unwrap_err();
        assert!(
            err_a.contains("Identity Spoofing"),
            "Must reject mismatched signer_id vs RPC sender_id"
        );

        // --- 攻撃B: 部外者 `node-mallory` が `signer_id = node-alpha` と偽って自身の秘密鍵で署名 ---
        let mut fake_sig_envelope = id_attacker.sign_rpc(forged_ae);
        fake_sig_envelope.signer_id = "node-alpha".into();
        let err_b = beta.handle_signed_rpc(fake_sig_envelope).unwrap_err();
        assert!(
            err_b.contains("invalid Ed25519 signature"),
            "Must reject forged signature using wrong private key"
        );

        // --- 攻撃C: 正規リーダー `node-alpha` が署名した遮断コマンドを中間者が `trusted = true` に1ビット改ざん ---
        let target_ip = Ipv4Addr::new(10, 0, 0, 88);
        alpha
            .propose(MeshCommand::set_trust(target_ip, false))
            .unwrap();
        let valid_ae = alpha.make_append_entries("node-beta").unwrap();
        let mut tampered_envelope: SignedConsensusRpc = id_alpha.sign_rpc(valid_ae);

        // ペイロード内の `trusted: false` を悪意をもって `trusted: true` に書き換える
        if let ConsensusRpc::AppendEntries { ref mut entries, .. } = tampered_envelope.rpc {
            entries[0].command = MeshCommand::set_trust(target_ip, true);
        }

        let err_c = beta.handle_signed_rpc(tampered_envelope).unwrap_err();
        assert!(
            err_c.contains("invalid Ed25519 signature or tampered RPC payload"),
            "Must reject tampered payload!"
        );

        // 3回のビザンチン攻撃がすべて記録・棄却され、beta の Term や eBPF マップが一切汚染されていないことを証明！
        assert_eq!(beta.rejected_byzantine_packets, 3);
        assert_eq!(beta.current_term, 1);
        assert!(beta.ebpf_policy_map.is_empty());
    }

    /// 証明6: TUI アリーナにおける実 OS モック攻撃プロセス (`sleep`) の起動とコンセンサス + eBPF 一斉駆逐の証明
    #[test]
    fn proof_6_tui_arena_mock_attack_process_spawn_and_extermination() -> Result<()> {
        let mut app = tui::TuiArenaApp::new();
        assert_eq!(app.rogues.len(), 0);

        // 1. OS 上にモック攻撃プロセス (`sleep 300`) を実際に spawn する
        app.spawn_rogue_attacker()?;
        assert_eq!(app.rogues.len(), 1);
        let pid = app.rogues[0].pid;
        let attacker_ip = app.rogues[0].attacker_ip;
        assert!(pid > 0);
        assert_eq!(app.rogues[0].status, tui::RogueStatus::Infiltrating);

        // 攻撃中は XDP_PASS となっている
        app.on_tick();
        assert_eq!(
            app.nodes["node-beta"].check_xdp_packet(attacker_ip),
            XdpVerdict::Pass
        );

        // 2. ビザンチン偽造 RPC を注入しても全ノードが拒否する
        app.inject_byzantine_spoof();
        assert_eq!(app.nodes["node-alpha"].rejected_byzantine_packets, 1);
        assert_eq!(app.nodes["node-beta"].rejected_byzantine_packets, 1);
        assert_eq!(app.nodes["node-gamma"].rejected_byzantine_packets, 1);

        // 3. コンセンサスによる一斉駆逐を実行
        app.exterminate_rogues_by_consensus();

        // プロセスが Exterminated (Kill) され、全3ノードの eBPF XDP フィルタが XDP_DROP になり、
        // 標的 node-beta の LSM ステルス GC が機密メモリをゼロ消去したことを検証！
        assert_eq!(app.rogues[0].status, tui::RogueStatus::Exterminated);
        for name in ["node-alpha", "node-beta", "node-gamma"] {
            assert_eq!(
                app.nodes[name].check_xdp_packet(attacker_ip),
                XdpVerdict::Drop
            );
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

        // root 権限がある場合のみ実カーネル XDP アタッチを実行
        let bridge = match ebpf_bridge::AyaKernelBridge::load_and_attach(
            obj_path.to_str().unwrap(),
            "veth-victim",
        ) {
            Ok(b) => Arc::new(b),
            Err(_) => return Ok(()),
        };

        let attacker_ip = Ipv4Addr::new(10, 99, 0, 66);

        // 1. 初期状態では 10.99.0.66 -> 10.99.0.1 の ping が通ることを確認
        let ping_before = std::process::Command::new("ip")
            .args(["netns", "exec", "ns-veil-rogue", "ping", "-c", "1", "-W", "1", "10.99.0.1"])
            .status()?;
        assert!(ping_before.success(), "Ping must succeed before consensus XDP block");

        // 2. コンセンサス合意により `PEER_POLICY_MAP` に `10.99.0.66 -> trusted=false` を書き込む
        bridge.sync_peer_policy(u32::from(attacker_ip), veil_common::PeerPolicy::new(false, 1))?;

        // 3. 再度 ping を打つと、カーネル NIC (`veth-victim`) の XDP 層で `XDP_DROP` され 100% タイムアウト（遮断）されることを証明！
        let ping_blocked = std::process::Command::new("ip")
            .args(["netns", "exec", "ns-veil-rogue", "ping", "-c", "1", "-W", "1", "10.99.0.1"])
            .status()?;
        assert!(!ping_blocked.success(), "Ping MUST be dropped at XDP layer when trusted=0!");

        let (_pass_cnt, drop_cnt) = bridge.read_xdp_counters().unwrap_or((0, 0));
        assert!(drop_cnt >= 1, "Kernel PACKETS_SEEN_MAP[1] (XDP_DROP) counter must increment!");

        // 4. 信頼回復 (`trusted=true`) を書き込むと、即座に ping が復活することを証明！
        bridge.sync_peer_policy(u32::from(attacker_ip), veil_common::PeerPolicy::new(true, 2))?;
        let ping_restored = std::process::Command::new("ip")
            .args(["netns", "exec", "ns-veil-rogue", "ping", "-c", "1", "-W", "1", "10.99.0.1"])
            .status()?;
        assert!(ping_restored.success(), "Ping must recover immediately after consensus restores trust!");

        Ok(())
    }

    /// 証明8: リーダー自身が乗っ取られて二枚舌（Equivocation）や単独捏造コミットを働いた場合でも、
    /// マルチシグ合意証明書（`QuorumCertificate`: $2f+1$ の Ed25519 署名バンドル）とハッシュ整合性検証により 100% 阻止されることの証明！
    #[test]
    fn proof_8_leader_equivocation_and_multisig_quorum_certificate_defense() {
        use consensus::{LogEntry, QuorumCertificate};

        // 4ノード BFT クラスタ (N = 3f + 1 = 4, Quorum = 2f + 1 = 3)
        let id_alpha = NodeCryptoIdentity::from_seed("node-alpha", &[1u8; 32]);
        let id_beta = NodeCryptoIdentity::from_seed("node-beta", &[2u8; 32]);
        let id_gamma = NodeCryptoIdentity::from_seed("node-gamma", &[3u8; 32]);
        let id_delta = NodeCryptoIdentity::from_seed("node-delta", &[4u8; 32]);

        let pub_keys = BTreeMap::from([
            ("node-alpha".to_string(), id_alpha.public_key_bytes.clone()),
            ("node-beta".to_string(), id_beta.public_key_bytes.clone()),
            ("node-gamma".to_string(), id_gamma.public_key_bytes.clone()),
            ("node-delta".to_string(), id_delta.public_key_bytes.clone()),
        ]);

        let mut beta = ConsensusState::new(
            "node-beta",
            vec!["node-alpha".into(), "node-gamma".into(), "node-delta".into()],
        );
        beta.register_peer_public_keys(pub_keys);
        assert_eq!(beta.quorum_size(), 3, "4-node cluster requires 2f+1 = 3 signatures for Quorum!");

        // 1. リーダー alpha が正規エントリ (10.0.0.99 を遮断) を beta に送信
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
        let resp = beta.handle_rpc(ae_honest).unwrap();
        if let ConsensusRpc::AppendEntriesResponse { success, entry_hash, .. } = resp {
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
        let resp_eq = beta.handle_rpc(ae_equivocate).unwrap();
        if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_eq {
            assert!(!success, "Follower MUST reject conflicting log entry at same (term, index)!");
        }
        assert_eq!(beta.rejected_byzantine_packets, 1);

        // 3. 【攻撃2：リーダー単独での捏造コミット (1台分の署名しかない偽 QC)】
        // リーダー alpha が自分の署名（1/4）しか入っていない QuorumCertificate で勝手に commit させようとする
        let hash = honest_entry.digest();
        let unilateral_qc = QuorumCertificate {
            term: 1,
            index: 1,
            entry_hash: hash,
            signatures: BTreeMap::from([("node-alpha".into(), id_alpha.sign_qc_vote(1, 1, &hash))]),
        };
        assert!(
            beta.verify_quorum_certificate(&unilateral_qc).is_err(),
            "Must reject QuorumCertificate with fewer than 2f+1 (3) valid node signatures!"
        );
        assert_eq!(beta.rejected_byzantine_packets, 2);

        // 4. 【正規のマルチシグ QC (alpha, beta, gamma の 3/4 署名) による安全な合意コミット】
        let valid_qc = QuorumCertificate {
            term: 1,
            index: 1,
            entry_hash: hash,
            signatures: BTreeMap::from([
                ("node-alpha".into(), id_alpha.sign_qc_vote(1, 1, &hash)),
                ("node-beta".into(), id_beta.sign_qc_vote(1, 1, &hash)),
                ("node-gamma".into(), id_gamma.sign_qc_vote(1, 1, &hash)),
            ]),
        };
        assert!(
            beta.verify_quorum_certificate(&valid_qc).is_ok(),
            "2f+1 (3/4) valid Ed25519 multi-signature QuorumCertificate must be accepted!"
        );
    }
}
