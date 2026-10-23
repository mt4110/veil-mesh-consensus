use crate::adapter::WireMessage;
use crate::domain::{MeshCommand, SignedConsensusRpc, MAX_WIRE_MESSAGE_BYTES};
use crate::usecase::ConsensusState;
use anyhow::Result;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// 【Infrastructure Layer】: クラスタ内の相互 TLS (mTLS) 認証用 PKI（Root CA 証明書ピンニング + クライアント・サーバー双方の証明書検証）
pub struct ClusterMtlsPki {
    ca_cert: rcgen::Certificate,
    ca_key: rcgen::KeyPair,
    ca_cert_der: rustls::pki_types::CertificateDer<'static>,
}

impl ClusterMtlsPki {
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

pub fn default_cluster_pki() -> &'static ClusterMtlsPki {
    static PKI: OnceLock<ClusterMtlsPki> = OnceLock::new();
    PKI.get_or_init(|| ClusterMtlsPki::generate().expect("Failed to initialize ClusterMtlsPki"))
}

pub fn make_quic_endpoint(bind_addr: SocketAddr) -> Result<quinn::Endpoint> {
    default_cluster_pki().make_endpoint(bind_addr)
}

/// 【Infrastructure Layer】: mTLS QUIC エンドポイント + コンセンサス・ステートマシンを束ねるネットワークノード
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

    pub async fn campaign(&self) -> Result<bool> {
        let signed_req = {
            let mut st = self.state.lock().await;
            st.start_signed_election()
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

        let is_leader = self.state.lock().await.role == crate::domain::Role::Leader;
        Ok(is_leader)
    }

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

    pub async fn propose_and_commit(&self, cmd: MeshCommand) -> Result<u64> {
        let index = {
            let mut st = self.state.lock().await;
            st.propose(cmd).map_err(|e| anyhow::anyhow!(e))?.index
        };

        self.replicate_round().await?;
        self.replicate_round().await?;

        Ok(index)
    }
}

pub async fn handle_connection(
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

pub async fn send_rpc_over_quic(
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

pub async fn send_ping_round(
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
