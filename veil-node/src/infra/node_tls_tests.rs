use super::{
    handle_authenticated_connection,
    node_tls::{fingerprint, identity_name, NodeTls},
    send_authenticated_rpc,
};
use crate::{
    adapter::NodeCryptoIdentity,
    domain::{ConsensusRpc, FixedConfiguration, Member},
    usecase::ConsensusState,
};
use rustls::pki_types::CertificateDer;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) struct Authority {
    cert: rcgen::Certificate,
    key: rcgen::KeyPair,
}
impl Authority {
    pub(crate) fn new() -> Self {
        let mut params = rcgen::CertificateParams::new(vec![]).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
        params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
        let key = rcgen::KeyPair::generate().unwrap();
        Self {
            cert: params.self_signed(&key).unwrap(),
            key,
        }
    }
    pub(crate) fn bundle(&self, dir: &Path, config: &FixedConfiguration, id: &str) -> PathBuf {
        self.custom_bundle(dir, config, id, |_| {})
    }
    fn custom_bundle(
        &self,
        dir: &Path,
        config: &FixedConfiguration,
        id: &str,
        customize: impl FnOnce(&mut rcgen::CertificateParams),
    ) -> PathBuf {
        let mut params =
            rcgen::CertificateParams::new(vec![identity_name(config.context(), id)]).unwrap();
        params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            rcgen::ExtendedKeyUsagePurpose::ServerAuth,
            rcgen::ExtendedKeyUsagePurpose::ClientAuth,
        ];
        customize(&mut params);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.signed_by(&key, &self.cert, &self.key).unwrap();
        std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
        for (name, bytes) in [
            ("ca.cert.der", self.cert.der().to_vec()),
            ("node.cert.der", cert.der().to_vec()),
            ("node.key.der", key.serialize_der()),
            ("revoked.txt", vec![]),
        ] {
            std::fs::write(dir.join(name), bytes).unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        dir.to_owned()
    }
}
fn directory() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "veil-node-tls-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    dir
}
fn config() -> (FixedConfiguration, Vec<NodeCryptoIdentity>) {
    let keys: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|id| NodeCryptoIdentity::generate_random(*id))
        .collect();
    let members = keys
        .iter()
        .enumerate()
        .map(|(i, key)| Member {
            node_id: key.node_id.clone(),
            address: format!("127.0.0.1:{}", 5100 + i).parse().unwrap(),
            public_key: key.public_key_bytes.as_slice().try_into().unwrap(),
        })
        .collect();
    (
        FixedConfiguration::new("tls-test", 1, members, vec![], 1000).unwrap(),
        keys,
    )
}

#[test]
fn node_bundle_validates_identity_key_permissions_validity_and_revocation() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root = directory();
    let ca = Authority::new();
    let (config, _) = config();
    let good = ca.bundle(&root.join("good"), &config, "a");
    assert!(NodeTls::load(&good, &config, "a").is_ok());
    assert!(NodeTls::load(&good, &config, "b").is_err());
    let other_config = FixedConfiguration::new(
        "different-cluster",
        1,
        config.members().to_vec(),
        vec![],
        1000,
    )
    .unwrap();
    assert!(NodeTls::load(&good, &other_config, "a").is_err());
    let other_epoch =
        FixedConfiguration::new("tls-test", 2, config.members().to_vec(), vec![], 1000).unwrap();
    assert!(NodeTls::load(&good, &other_epoch, "a").is_err());
    let other_policy = FixedConfiguration::new(
        "tls-test",
        1,
        config.members().to_vec(),
        vec!["192.0.2.1".parse().unwrap()],
        1000,
    )
    .unwrap();
    assert!(NodeTls::load(&good, &other_policy, "a").is_err());
    for scenario in 0..10 {
        let dir = ca.bundle(&root.join(format!("bad-{scenario}")), &config, "a");
        match scenario {
            0 => std::fs::write(
                dir.join("node.key.der"),
                rcgen::KeyPair::generate().unwrap().serialize_der(),
            )
            .unwrap(),
            1 => std::fs::set_permissions(
                dir.join("node.key.der"),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap(),
            2 => std::fs::rename(
                dir.join("node.key.der"),
                root.join("retained-missing-key.der"),
            )
            .unwrap(),
            3 => std::fs::write(dir.join("revoked.txt"), b"bad fingerprint\n").unwrap(),
            4 => {
                let cert = std::fs::read(dir.join("node.cert.der")).unwrap();
                std::fs::write(dir.join("revoked.txt"), format!("{}\n", fingerprint(&cert)))
                    .unwrap();
            }
            5 => std::fs::write(dir.join("ca.key.der"), b"must never read a CA secret").unwrap(),
            6 => std::fs::write(dir.join("ca.cert.der"), Authority::new().cert.der()).unwrap(),
            7 => {
                std::fs::rename(
                    dir.join("node.key.der"),
                    root.join("retained-symlink-key.der"),
                )
                .unwrap();
                std::os::unix::fs::symlink(
                    root.join("retained-symlink-key.der"),
                    dir.join("node.key.der"),
                )
                .unwrap();
            }
            8 => std::fs::write(dir.join("node.cert.der"), vec![0u8; 16_385]).unwrap(),
            _ => {
                std::fs::rename(dir.join("revoked.txt"), root.join("retained-revoked.txt")).unwrap()
            }
        }
        assert!(
            NodeTls::load(&dir, &config, "a").is_err(),
            "scenario {scenario}"
        );
    }
    for scenario in 0..4 {
        let dir = ca.custom_bundle(
            &root.join(format!("validity-{scenario}")),
            &config,
            "a",
            |params| match scenario {
                0 => {
                    params.not_before = rcgen::date_time_ymd(2000, 1, 1);
                    params.not_after = rcgen::date_time_ymd(2001, 1, 1);
                }
                1 => {
                    params.not_before = rcgen::date_time_ymd(4090, 1, 1);
                    params.not_after = rcgen::date_time_ymd(4091, 1, 1);
                }
                2 => params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth],
                _ => params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth],
            },
        );
        assert!(
            NodeTls::load(&dir, &config, "a").is_err(),
            "validity {scenario}"
        );
    }
}

#[tokio::test]
async fn node_tls_rejects_cross_identity_rpc_wrong_server_and_revoked_leaf() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root = directory();
    let ca = Authority::new();
    let (config, keys) = config();
    let mut states = Vec::new();
    let mut credentials = Vec::new();
    let mut endpoints = Vec::new();
    for key in keys {
        let id = key.node_id.clone();
        let dir = ca.bundle(&root.join(&id), &config, &id);
        let tls = Arc::new(NodeTls::load(&dir, &config, &id)?);
        endpoints.push(tls.make_endpoint("127.0.0.1:0".parse()?)?);
        let mut state = ConsensusState::new(&id, vec![]);
        state
            .configure_fixed_membership(config.clone(), key)
            .unwrap();
        state
            .enable_persistence(Arc::new(crate::infra::wal::FileWal::open(
                &root.join(format!("{id}.wal")),
            )?))
            .unwrap();
        states.push(Arc::new(tokio::sync::Mutex::new(state)));
        credentials.push(tls);
    }
    let server = endpoints[1].clone();
    let state = states[1].clone();
    let tls = credentials[1].clone();
    let task = tokio::spawn(async move {
        while let Some(incoming) = server.accept().await {
            let state = state.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                if let Ok(conn) = incoming.await {
                    let _ = handle_authenticated_connection(conn, "b", state, tls).await;
                }
            });
        }
    });
    let request = states[0].lock().await.start_signed_election().unwrap();
    let before = std::fs::read(root.join("b.wal"))?;
    // Valid A consensus signature replayed over C's authenticated TLS identity.
    assert!(send_authenticated_rpc(
        &endpoints[2],
        endpoints[1].local_addr()?,
        request.clone(),
        &credentials[2],
        "b"
    )
    .await
    .is_err());
    assert_eq!(std::fs::read(root.join("b.wal"))?, before);
    // The server at B's address cannot masquerade as configured C.
    assert!(send_authenticated_rpc(
        &endpoints[0],
        endpoints[1].local_addr()?,
        request.clone(),
        &credentials[0],
        "c"
    )
    .await
    .is_err());
    let reply = send_authenticated_rpc(
        &endpoints[0],
        endpoints[1].local_addr()?,
        request,
        &credentials[0],
        "b",
    )
    .await?
    .unwrap();
    assert!(matches!(
        reply.rpc,
        ConsensusRpc::RequestVoteResponse {
            vote_granted: true,
            ..
        }
    ));
    // New certificate/key, same identity/configuration, works without changing
    // signing keys, WAL version or config digest. Explicitly deny the old leaf.
    let old = std::fs::read(root.join("a/node.cert.der"))?;
    let rotated = ca.bundle(&root.join("a-rotated"), &config, "a");
    let revoked = ca.bundle(&root.join("b-revoked"), &config, "b");
    std::fs::write(
        revoked.join("revoked.txt"),
        format!("{}\n", fingerprint(&old)),
    )?;
    let verifier = NodeTls::load(&revoked, &config, "b")?;
    assert!(verifier
        .verify_certificates(&[CertificateDer::from(old)], "a")
        .is_err());
    assert!(verifier
        .verify_certificates(
            &[CertificateDer::from(std::fs::read(
                rotated.join("node.cert.der")
            )?)],
            "a"
        )
        .is_ok());
    let verifier = Arc::new(verifier);
    let revoked_server = verifier.make_endpoint("127.0.0.1:0".parse()?)?;
    let revoked_addr = revoked_server.local_addr()?;
    let srv = revoked_server.clone();
    let state = states[1].clone();
    let auth = verifier.clone();
    let revoked_task = tokio::spawn(async move {
        while let Some(incoming) = srv.accept().await {
            let state = state.clone();
            let auth = auth.clone();
            tokio::spawn(async move {
                if let Ok(conn) = incoming.await {
                    let _ = handle_authenticated_connection(conn, "b", state, auth).await;
                }
            });
        }
    });
    // TLS revocation is independent of election admission; use a fresh pre-vote.
    let next = states[0].lock().await.start_pre_vote().unwrap();
    let before = std::fs::read(root.join("b.wal"))?;
    assert!(send_authenticated_rpc(
        &endpoints[0],
        revoked_addr,
        next.clone(),
        &credentials[0],
        "b"
    )
    .await
    .is_err());
    assert_eq!(std::fs::read(root.join("b.wal"))?, before);
    let fresh = NodeTls::load(&rotated, &config, "a")?;
    let fresh_endpoint = fresh.make_endpoint("127.0.0.1:0".parse()?)?;
    assert!(
        send_authenticated_rpc(&fresh_endpoint, revoked_addr, next, &fresh, "b")
            .await?
            .is_some()
    );
    fresh_endpoint.close(0u32.into(), b"done");
    revoked_server.close(0u32.into(), b"done");
    revoked_task.abort();
    for ep in &endpoints {
        ep.close(0u32.into(), b"test done");
    }
    task.abort();
    Ok(())
}
