use crate::domain::{
    qc_vote_payload, rpc_signing_payload, timeout_vote_payload, ConsensusRpc, NodeId,
    SignedConsensusRpc, SigningContext,
};
use ring::signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

/// 【Interface Adapter】: `ring` による Ed25519 署名鍵ペア管理（OS 乱数生成および `0600` 鍵ファイル永続化）
pub struct NodeCryptoIdentity {
    pub node_id: NodeId,
    keypair: Ed25519KeyPair,
    pub public_key_bytes: Vec<u8>,
}

impl NodeCryptoIdentity {
    /// 32バイトのシードから Ed25519 鍵ペアを生成する（テストまたはファイル復元用）
    pub fn from_seed(node_id: impl Into<String>, seed: &[u8; 32]) -> Self {
        let keypair =
            Ed25519KeyPair::from_seed_unchecked(seed).expect("valid 32-byte Ed25519 seed");
        let public_key_bytes = keypair.public_key().as_ref().to_vec();
        Self {
            node_id: node_id.into(),
            keypair,
            public_key_bytes,
        }
    }

    /// OS の暗号学的乱数生成器 (`SystemRandom`) を用いて予測不可能な Ed25519 鍵ペアを生成する
    pub fn generate_random(node_id: impl Into<String>) -> Self {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("CSPRNG Ed25519 keygen");
        let keypair =
            Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("valid PKCS#8 Ed25519 keypair");
        let public_key_bytes = keypair.public_key().as_ref().to_vec();
        Self {
            node_id: node_id.into(),
            keypair,
            public_key_bytes,
        }
    }

    /// PKCS#8 バイト列（ファイル保存用）から鍵ペアを復元、または新規生成して排他的 `0600` パーミッションで保存する（R8 対策）
    pub fn load_or_generate_file(
        node_id: impl Into<String>,
        path: &std::path::Path,
    ) -> std::io::Result<Self> {
        Self::load_file(node_id.into(), path, true)
    }

    /// Normal validators must never silently replace a missing signing identity.
    pub fn load_existing_file(
        node_id: impl Into<String>,
        path: &std::path::Path,
    ) -> std::io::Result<Self> {
        Self::load_file(node_id.into(), path, false)
    }

    fn load_file(id: String, path: &std::path::Path, allow_create: bool) -> std::io::Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "Refusing to load Ed25519 key from a symlink",
                    ));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = meta.permissions().mode() & 0o777;
                    if mode != 0o600 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!(
                                "Insecure key file permissions {:04o} on {}: must be 0600",
                                mode,
                                path.display()
                            ),
                        ));
                    }
                }
                let bytes = std::fs::read(path)?;
                if bytes.len() == 32 {
                    let mut seed = [0u8; 32];
                    seed.copy_from_slice(&bytes);
                    return Ok(Self::from_seed(id, &seed));
                }
                let keypair = Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Invalid Ed25519 PKCS#8 key",
                    )
                })?;
                let public_key_bytes = keypair.public_key().as_ref().to_vec();
                Ok(Self {
                    node_id: id,
                    keypair,
                    public_key_bytes,
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && allow_create => {
                Self::create_new_file(id, path)
            }
            Err(err) => Err(err),
        }
    }

    /// Exclusive key creation. Existing keys are never treated as unused identities.
    pub fn create_new_file(id: String, path: &std::path::Path) -> std::io::Result<Self> {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| std::io::Error::other("CSPRNG key generation failed"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            file.write_all(pkcs8.as_ref())?;
            file.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, pkcs8.as_ref())?;
        }
        let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("valid PKCS#8");
        let public_key_bytes = keypair.public_key().as_ref().to_vec();
        Ok(Self {
            node_id: id,
            keypair,
            public_key_bytes,
        })
    }

    /// `ConsensusRpc` をシリアライズして Ed25519 署名を付与する
    pub fn sign_rpc(&self, context: &SigningContext, rpc: ConsensusRpc) -> SignedConsensusRpc {
        let payload = rpc_signing_payload(context, &self.node_id, &rpc);
        let sig = self.keypair.sign(&payload);
        SignedConsensusRpc {
            context: context.clone(),
            signer_id: self.node_id.clone(),
            rpc,
            signature: sig.as_ref().to_vec(),
        }
    }

    pub(crate) fn sign_progress_payload(&self, payload: &[u8]) -> Vec<u8> {
        self.keypair.sign(payload).as_ref().to_vec()
    }

    pub fn sign_term_support(&self, context: &SigningContext, term: u64) -> Vec<u8> {
        self.keypair
            .sign(&crate::domain::term_support_payload(context, term))
            .as_ref()
            .to_vec()
    }

    /// 特定のログエントリ `(term, index, entry_hash)` に対する QC 投票署名を計算する
    pub fn sign_qc_vote(
        &self,
        context: &SigningContext,
        term: u64,
        index: u64,
        entry_hash: &[u8; 32],
    ) -> Vec<u8> {
        let payload = qc_vote_payload(context, term, index, entry_hash);
        self.keypair.sign(&payload).as_ref().to_vec()
    }

    /// View-Change 用の `TimeoutVote` `(term, high_qc_term, high_qc_index)` 署名を計算する（v0.4.0）
    pub fn sign_timeout_vote(
        &self,
        context: &SigningContext,
        term: u64,
        high_qc_term: u64,
        high_qc_index: u64,
    ) -> Vec<u8> {
        let payload = timeout_vote_payload(context, term, high_qc_term, high_qc_index);
        self.keypair.sign(&payload).as_ref().to_vec()
    }

    /// 公開鍵バイト列を用いて任意のメッセージに対する Ed25519 署名を検証する
    pub fn verify_signature(pub_key_bytes: &[u8], payload: &[u8], sig_bytes: &[u8]) -> bool {
        let peer_pub_key = UnparsedPublicKey::new(&signature::ED25519, pub_key_bytes);
        peer_pub_key.verify(payload, sig_bytes).is_ok()
    }
}
