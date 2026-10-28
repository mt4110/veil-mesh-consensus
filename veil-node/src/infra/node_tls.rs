//! Daemon TLS consumes a read-only node bundle. No CA private key or certificate
//! generation is linked into this path. The offline issuer is a separate tool.
use crate::domain::{FixedConfiguration, SigningContext};
use anyhow::{ensure, Context, Result};
use rustls::{
    client::danger::ServerCertVerifier,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
};
use std::{
    collections::BTreeSet,
    io::Read,
    net::SocketAddr,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    sync::Arc,
};

pub fn fingerprint(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Fixed-size DNS labels avoid ambiguous concatenation, invalid node-ID DNS
/// characters, and the 63-byte label limit. No DNS lookup uses this name.
pub fn identity_name(context: &SigningContext, node_id: &str) -> String {
    let node = fingerprint(node_id.as_bytes());
    let config: String = context
        .config_digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!(
        "n{}.n{}.c{}.c{}.veil.invalid",
        &node[..32],
        &node[32..],
        &config[..32],
        &config[32..]
    )
}

fn read_bounded(path: &Path, secret: bool, limit: u64) -> Result<Vec<u8>> {
    let before = std::fs::symlink_metadata(path)?;
    ensure!(
        before.is_file() && !before.file_type().is_symlink(),
        "TLS input must be a regular file: {}",
        path.display()
    );
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.dev() == before.dev() && meta.ino() == before.ino(),
        "TLS input changed while opening"
    );
    ensure!(
        meta.permissions().mode() & 0o022 == 0,
        "TLS input must not be group/other writable"
    );
    if secret {
        ensure!(
            meta.permissions().mode() & 0o777 == 0o600,
            "TLS key requires mode 0600"
        );
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "TLS input exceeds size limit");
    Ok(bytes)
}

pub struct NodeTls {
    context: SigningContext,
    members: BTreeSet<String>,
    revoked: BTreeSet<String>,
    verifier: Arc<rustls::client::WebPkiServerVerifier>,
    server: quinn::ServerConfig,
    client: quinn::ClientConfig,
    pub certificate_sha256: String,
}

impl NodeTls {
    pub fn load(dir: &Path, config: &FixedConfiguration, node_id: &str) -> Result<Self> {
        let meta = std::fs::symlink_metadata(dir)?;
        ensure!(
            meta.is_dir() && meta.permissions().mode() & 0o777 == 0o700,
            "TLS bundle must be a real 0700 directory"
        );
        let allowed = [
            "ca.cert.der",
            "node.cert.der",
            "node.key.der",
            "revoked.txt",
        ];
        for item in std::fs::read_dir(dir)? {
            let name = item?.file_name();
            ensure!(
                allowed.iter().any(|s| name == *s),
                "Unexpected file in TLS bundle (CA private keys must stay offline)"
            );
        }
        let ca = CertificateDer::from(read_bounded(&dir.join("ca.cert.der"), false, 16_384)?);
        let cert = CertificateDer::from(read_bounded(&dir.join("node.cert.der"), false, 16_384)?);
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(read_bounded(
            &dir.join("node.key.der"),
            true,
            16_384,
        )?));
        let revoked_text =
            String::from_utf8(read_bounded(&dir.join("revoked.txt"), false, 65 * 4096)?)?;
        let mut revoked = BTreeSet::new();
        for line in revoked_text.lines() {
            ensure!(
                line.len() == 64 && line.bytes().all(|b| b.is_ascii_hexdigit()),
                "Malformed certificate revocation fingerprint"
            );
            ensure!(
                revoked.insert(line.to_ascii_lowercase()),
                "Duplicate revocation fingerprint"
            );
        }
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca)?;
        let roots = Arc::new(roots);
        let verifier = rustls::client::WebPkiServerVerifier::builder(roots.clone()).build()?;
        let client_verifier =
            rustls::server::WebPkiClientVerifier::builder(roots.clone()).build()?;
        // Validate both EKUs, time, chain and the exact configured local name
        // before opening WAL, attaching XDP, or accepting any connections.
        client_verifier.verify_client_cert(&cert, &[], UnixTime::now())?;
        let server_crypto = rustls::ServerConfig::builder()
            .with_client_cert_verifier(client_verifier)
            .with_single_cert(vec![cert.clone()], key.clone_key())?;
        let client_crypto = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(vec![cert.clone()], key)?;
        let server = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
        ));
        let client = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)?,
        ));
        let tls = Self {
            context: config.context().clone(),
            members: config.members().iter().map(|m| m.node_id.clone()).collect(),
            revoked,
            verifier,
            server,
            client,
            certificate_sha256: fingerprint(&cert),
        };
        tls.verify_certificates(&[cert], node_id)?;
        Ok(tls)
    }

    pub fn peer_name(&self, node_id: &str) -> Result<String> {
        ensure!(
            self.members.contains(node_id),
            "TLS peer is not a configured member"
        );
        Ok(identity_name(&self.context, node_id))
    }

    pub fn verify_certificates(&self, certs: &[CertificateDer<'_>], node_id: &str) -> Result<()> {
        // Direct root-issued leaves only. Keeping this profile explicit avoids
        // accidentally treating an intermediate as an identity certificate.
        ensure!(certs.len() == 1, "Expected one node leaf certificate");
        ensure!(
            !self.revoked.contains(&fingerprint(&certs[0])),
            "TLS certificate is locally revoked"
        );
        let name = ServerName::try_from(self.peer_name(node_id)?)?;
        self.verifier
            .verify_server_cert(&certs[0], &[], &name, &[], UnixTime::now())
            .context("TLS certificate does not authenticate configured node/context")?;
        Ok(())
    }

    pub fn verify_peer(&self, conn: &quinn::Connection, node_id: &str) -> Result<()> {
        let identity = conn
            .peer_identity()
            .ok_or_else(|| anyhow::anyhow!("Missing mTLS peer identity"))?;
        let certs = identity
            .downcast::<Vec<CertificateDer<'static>>>()
            .map_err(|_| anyhow::anyhow!("Unsupported TLS identity"))?;
        self.verify_certificates(&certs, node_id)
    }

    pub fn make_endpoint(&self, address: SocketAddr) -> Result<quinn::Endpoint> {
        let mut endpoint = quinn::Endpoint::server(self.server.clone(), address)?;
        endpoint.set_default_client_config(self.client.clone());
        Ok(endpoint)
    }

    #[cfg(test)]
    pub fn configure_test_endpoint(&self, endpoint: &mut quinn::Endpoint) {
        endpoint.set_server_config(Some(self.server.clone()));
        endpoint.set_default_client_config(self.client.clone());
    }
}
