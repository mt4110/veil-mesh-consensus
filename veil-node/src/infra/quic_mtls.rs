use crate::adapter::WireMessage;
#[cfg(test)]
use crate::domain::MeshCommand;
use crate::domain::{SignedConsensusRpc, MAX_WIRE_MESSAGE_BYTES};
use crate::infra::node_tls::NodeTls;
use crate::usecase::ConsensusState;
use anyhow::{Context, Result};
#[cfg(test)]
use std::{collections::BTreeMap, sync::OnceLock};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tracing::{info, warn};

// Diagnostic correlation only. Never changes the signed wire data, and is not
// computed unless this target is enabled. Identical retries share the same ID.
fn timing_id(bytes: &[u8]) -> String {
    if !tracing::enabled!(target: "veil_rpc_timing", tracing::Level::DEBUG) {
        return String::new();
    }
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn timing_phase(rpc: &crate::domain::ConsensusRpc) -> (&'static str, u64) {
    use crate::domain::{ConsensusRpc, Phase, PhaseMessage};
    match rpc {
        ConsensusRpc::TwoPhase { message, .. } => match message.as_ref() {
            PhaseMessage::Proposal { block, .. } => ("proposal", block.entry.index),
            PhaseMessage::Prepared { certificate, .. } => {
                ("prepared", certificate.block.entry.index)
            }
            PhaseMessage::Vote { phase, index, .. } => (
                match phase {
                    Phase::Prepare => "prepare-vote",
                    Phase::Commit => "commit-vote",
                },
                *index,
            ),
            PhaseMessage::Commit(certificate) => ("commit", certificate.block.entry.index),
            PhaseMessage::Sync { index } => ("sync", *index),
            PhaseMessage::Receipt { index, .. } => ("receipt", *index),
            _ => ("view-or-checkpoint", 0),
        },
        _ => ("other", 0),
    }
}

/// 【Infrastructure Layer】: クラスタ内の相互 TLS (mTLS) 認証用 PKI（Root CA 証明書ピンニング + クライアント・サーバー双方の証明書検証）
#[cfg(test)]
pub struct ClusterMtlsPki {
    ca_cert: rcgen::Certificate,
    ca_key: rcgen::KeyPair,
    ca_cert_der: rustls::pki_types::CertificateDer<'static>,
}

#[cfg(test)]
impl ClusterMtlsPki {
    fn ca_certificate_params() -> Result<rcgen::CertificateParams> {
        let mut ca_params = rcgen::CertificateParams::new(vec!["veil.mesh.ca".to_string()])?;
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::DigitalSignature,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        Ok(ca_params)
    }

    pub fn generate() -> Result<Self> {
        let ca_params = Self::ca_certificate_params()?;
        let ca_key = rcgen::KeyPair::generate()?;
        let ca_cert = ca_params.self_signed(&ca_key)?;
        let ca_cert_der = ca_cert.der().clone();
        Ok(Self {
            ca_cert,
            ca_key,
            ca_cert_der,
        })
    }

    /// R3 対策: 独立した OS プロセス間で同一の Root CA を共有できるよう、指定ディレクトリから CA 鍵と証明書を読み込む（未作成時は排他生成）
    pub fn load_or_generate_dir(dir: &std::path::Path) -> Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let lock_path = dir.join("ca.init.lock");
        for path in [
            &lock_path,
            &dir.join("ca.key.der"),
            &dir.join("ca.cert.der"),
        ] {
            match std::fs::symlink_metadata(path) {
                Ok(meta) => anyhow::ensure!(
                    meta.is_file() && !meta.file_type().is_symlink(),
                    "PKI files must be regular files"
                ),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)?;
        // Stable OS lock, released by drop/crash. Concurrent process startup
        // must never generate/overwrite different CA pairs in the same directory.
        lock.lock()?;
        let key_path = dir.join("ca.key.der");
        let cert_path = dir.join("ca.cert.der");
        let ca_params = Self::ca_certificate_params()?;

        if key_path.exists() && cert_path.exists() {
            anyhow::ensure!(
                std::fs::metadata(&key_path)?.permissions().mode() & 0o777 == 0o600,
                "CA key requires mode 0600"
            );
            let key_der = std::fs::read(&key_path)?;
            let cert_der = std::fs::read(&cert_path)?;
            let pkcs8 = rustls::pki_types::PrivatePkcs8KeyDer::from(key_der);
            let ca_key = rcgen::KeyPair::try_from(&pkcs8)?;
            let ca_cert = ca_params.self_signed(&ca_key)?;
            let ca_cert_der = rustls::pki_types::CertificateDer::from(cert_der);
            return Ok(Self {
                ca_cert,
                ca_key,
                ca_cert_der,
            });
        }

        anyhow::ensure!(
            !key_path.exists() && !cert_path.exists(),
            "Incomplete PKI initialization; preserve files and recover the CA pair explicitly"
        );
        let ca_key = rcgen::KeyPair::generate()?;
        let ca_cert = ca_params.self_signed(&ca_key)?;
        let ca_cert_der = ca_cert.der().clone();
        let mut key_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&key_path)?;
        key_file.write_all(&ca_key.serialize_der())?;
        key_file.sync_all()?;
        let mut cert_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&cert_path)?;
        cert_file.write_all(ca_cert_der.as_ref())?;
        cert_file.sync_all()?;
        std::fs::File::open(dir)?.sync_all()?;

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

#[cfg(test)]
pub fn default_cluster_pki() -> &'static ClusterMtlsPki {
    static PKI: OnceLock<ClusterMtlsPki> = OnceLock::new();
    PKI.get_or_init(|| {
        let shared_dir = std::env::var_os("VEIL_CLUSTER_PKI_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("veil-mesh-cluster-pki-v2"));
        ClusterMtlsPki::load_or_generate_dir(&shared_dir)
            .or_else(|_| ClusterMtlsPki::generate())
            .expect("Failed to initialize ClusterMtlsPki")
    })
}

#[cfg(test)]
pub fn make_quic_endpoint(bind_addr: SocketAddr) -> Result<quinn::Endpoint> {
    default_cluster_pki().make_endpoint(bind_addr)
}

/// 【Infrastructure Layer】: mTLS QUIC エンドポイント + コンセンサス・ステートマシンを束ねるネットワークノード
#[cfg(test)]
pub struct MeshNode {
    pub node_id: String,
    pub endpoint: quinn::Endpoint,
    pub state: Arc<Mutex<ConsensusState>>,
    pub peer_addrs: BTreeMap<String, SocketAddr>,
}

#[cfg(test)]
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
            st.start_signed_election().map_err(anyhow::Error::msg)?
        };

        for (peer_id, &addr) in &self.peer_addrs {
            match send_rpc_over_quic(&self.endpoint, addr, signed_req.clone()).await {
                Ok(Some(signed_resp)) => {
                    let mut st = self.state.lock().await;
                    st.handle_signed_rpc(signed_resp)
                        .map_err(anyhow::Error::msg)?;
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
                let mut st = self.state.lock().await;
                st.make_append_entries(peer_id)
                    .map(|rpc| st.sign_rpc_durable(rpc))
                    .transpose()
                    .map_err(anyhow::Error::msg)?
            };

            if let Some(signed_rpc) = maybe_signed_rpc {
                if let Ok(Some(signed_resp)) =
                    send_rpc_over_quic(&self.endpoint, addr, signed_rpc).await
                {
                    let mut st = self.state.lock().await;
                    st.handle_signed_rpc(signed_resp)
                        .map_err(anyhow::Error::msg)?;
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

        // R4 対策: 定足数コミットおよび eBPF 適用が完了していない場合は未コミット成功として扱わずエラーを返す
        let st = self.state.lock().await;
        if st.commit_index < index {
            anyhow::bail!(
                "Proposal at index {index} failed to reach quorum commit (commit_index={})",
                st.commit_index
            );
        }
        if st.last_applied < index {
            anyhow::bail!(
                "Proposal at index {index} committed but kernel apply failed (last_applied={}, error={:?})",
                st.last_applied,
                st.last_apply_error
            );
        }

        Ok(index)
    }
}

#[cfg(test)]
pub async fn handle_connection(
    conn: quinn::Connection,
    node_id: &str,
    state: Arc<Mutex<ConsensusState>>,
) -> Result<()> {
    handle_connection_inner(conn, node_id, state, None).await
}

pub async fn handle_authenticated_connection(
    conn: quinn::Connection,
    node_id: &str,
    state: Arc<Mutex<ConsensusState>>,
    tls: Arc<NodeTls>,
) -> Result<()> {
    handle_connection_inner(conn, node_id, state, Some(tls)).await
}

async fn handle_connection_inner(
    conn: quinn::Connection,
    node_id: &str,
    state: Arc<Mutex<ConsensusState>>,
    tls: Option<Arc<NodeTls>>,
) -> Result<()> {
    let remote = conn.remote_address();
    while let Ok(Ok((mut send, mut recv))) =
        tokio::time::timeout(Duration::from_secs(2), conn.accept_bi()).await
    {
        let buf = tokio::time::timeout(
            Duration::from_secs(2),
            recv.read_to_end(MAX_WIRE_MESSAGE_BYTES),
        )
        .await??;
        let msg: WireMessage = bincode::deserialize(&buf)?;
        match msg {
            WireMessage::Ping { from_node, epoch } => {
                anyhow::ensure!(
                    tls.is_none(),
                    "Daemon TLS accepts authenticated consensus RPCs only"
                );
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
                let rpc_id = timing_id(&buf);
                let started = std::time::Instant::now();
                let (phase, index) = timing_phase(&signed_rpc.rpc);
                tracing::debug!(target: "veil_rpc_timing", %rpc_id, node = node_id, phase, index, term = signed_rpc.rpc.term(), "RPC received");
                if let Some(tls) = &tls {
                    // Re-check identity, expiration and revocation for every RPC,
                    // including on an already-established connection.
                    tls.verify_peer(&conn, &signed_rpc.signer_id)?;
                }
                let maybe_signed_resp = super::state_worker::run(state.clone(), move |st| {
                    let response = match st.handle_signed_rpc(*signed_rpc) {
                        Ok(Some(resp)) => {
                            Some(st.sign_rpc_durable(resp).map_err(anyhow::Error::msg)?)
                        }
                        Ok(None) => None,
                        Err(err) => {
                            warn!(remote = %remote, error = %err, "Consensus RPC rejected or persistence unavailable");
                            None
                        }
                    };
                    Ok::<_, anyhow::Error>(response)
                }).await??;
                tracing::debug!(target: "veil_rpc_timing", %rpc_id, node = node_id, elapsed_us = started.elapsed().as_micros(), "RPC durable handling completed");
                if let Some(signed_resp) = maybe_signed_resp {
                    let wire = WireMessage::Consensus(signed_resp.into());
                    let bytes = bincode::serialize(&wire)?;
                    anyhow::ensure!(
                        bytes.len() <= MAX_WIRE_MESSAGE_BYTES,
                        "RPC reply exceeds wire size limit"
                    );
                    send.write_all(&bytes).await?;
                }
                send.finish()?;
                tracing::debug!(target: "veil_rpc_timing", %rpc_id, node = node_id, elapsed_us = started.elapsed().as_micros(), "RPC response written");
            }
            WireMessage::Ack { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
pub async fn send_rpc_over_quic(
    endpoint: &quinn::Endpoint,
    peer_addr: SocketAddr,
    signed_rpc: SignedConsensusRpc,
) -> Result<Option<SignedConsensusRpc>> {
    send_rpc_inner(endpoint, peer_addr, signed_rpc, None).await
}

pub async fn send_authenticated_rpc(
    endpoint: &quinn::Endpoint,
    peer_addr: SocketAddr,
    signed_rpc: SignedConsensusRpc,
    tls: &NodeTls,
    peer_id: &str,
) -> Result<Option<SignedConsensusRpc>> {
    send_rpc_inner(endpoint, peer_addr, signed_rpc, Some((tls, peer_id))).await
}

async fn send_rpc_inner(
    endpoint: &quinn::Endpoint,
    peer_addr: SocketAddr,
    signed_rpc: SignedConsensusRpc,
    tls: Option<(&NodeTls, &str)>,
) -> Result<Option<SignedConsensusRpc>> {
    let (phase, index) = timing_phase(&signed_rpc.rpc);
    let term = signed_rpc.rpc.term();
    let wire = WireMessage::Consensus(signed_rpc.into());
    let bytes = bincode::serialize(&wire)?;
    let rpc_id = timing_id(&bytes);
    let started = std::time::Instant::now();
    tracing::debug!(target: "veil_rpc_timing", %rpc_id, %peer_addr, phase, index, term, "RPC send started");
    anyhow::ensure!(
        bytes.len() <= MAX_WIRE_MESSAGE_BYTES,
        "RPC wire envelope exceeds size limit"
    );
    let name = tls
        .map(|(tls, id)| tls.peer_name(id))
        .transpose()?
        .unwrap_or_else(|| "veil.mesh".into());
    let conn = tokio::time::timeout(
        Duration::from_millis(300),
        endpoint.connect(peer_addr, &name)?,
    )
    .await
    .context("QUIC connect deadline elapsed")??;
    tracing::debug!(target: "veil_rpc_timing", %rpc_id, %peer_addr, elapsed_us = started.elapsed().as_micros(), "QUIC connected");
    if let Some((tls, id)) = tls {
        tls.verify_peer(&conn, id)?;
    }

    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&bytes).await?;
    send.finish()?;
    tracing::debug!(target: "veil_rpc_timing", %rpc_id, %peer_addr, elapsed_us = started.elapsed().as_micros(), "RPC request written");

    let buf = tokio::time::timeout(
        Duration::from_millis(300),
        recv.read_to_end(MAX_WIRE_MESSAGE_BYTES),
    )
    .await
    .context("QUIC response deadline elapsed")??;
    tracing::debug!(target: "veil_rpc_timing", %rpc_id, %peer_addr, elapsed_us = started.elapsed().as_micros(), "RPC response received");
    if buf.is_empty() {
        return Ok(None);
    }
    if let WireMessage::Consensus(signed_resp) = bincode::deserialize(&buf)? {
        if let Some((tls, id)) = tls {
            tls.verify_peer(&conn, id)?;
            anyhow::ensure!(
                signed_resp.signer_id == id,
                "Response signer differs from TLS peer"
            );
        }
        Ok(Some(*signed_resp))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
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
