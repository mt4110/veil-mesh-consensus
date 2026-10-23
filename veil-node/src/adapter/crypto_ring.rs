use crate::domain::{qc_vote_payload, ConsensusRpc, NodeId, SignedConsensusRpc};
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

    /// PKCS#8 バイト列（ファイル保存用）から鍵ペアを復元、または新規生成して `0600` パーミッションで保存する
    pub fn load_or_generate_file(
        node_id: impl Into<String>,
        path: &std::path::Path,
    ) -> std::io::Result<Self> {
        let id = node_id.into();
        if path.exists() {
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
        } else {
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
                .map_err(|_| std::io::Error::other("CSPRNG key generation failed"))?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, pkcs8.as_ref())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            }
            let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("valid PKCS#8");
            let public_key_bytes = keypair.public_key().as_ref().to_vec();
            Ok(Self {
                node_id: id,
                keypair,
                public_key_bytes,
            })
        }
    }

    /// `ConsensusRpc` をシリアライズして Ed25519 署名を付与する
    pub fn sign_rpc(&self, rpc: ConsensusRpc) -> SignedConsensusRpc {
        let payload = bincode::serialize(&rpc).expect("ConsensusRpc serialization must succeed");
        let sig = self.keypair.sign(&payload);
        SignedConsensusRpc {
            signer_id: self.node_id.clone(),
            rpc,
            signature: sig.as_ref().to_vec(),
        }
    }

    /// 特定のログエントリ `(term, index, entry_hash)` に対する QC 投票署名を計算する
    pub fn sign_qc_vote(&self, term: u64, index: u64, entry_hash: &[u8; 32]) -> Vec<u8> {
        let payload = qc_vote_payload(term, index, entry_hash);
        self.keypair.sign(&payload).as_ref().to_vec()
    }

    /// 公開鍵バイト列を用いて任意のメッセージに対する Ed25519 署名を検証する
    pub fn verify_signature(pub_key_bytes: &[u8], payload: &[u8], sig_bytes: &[u8]) -> bool {
        let peer_pub_key = UnparsedPublicKey::new(&signature::ED25519, pub_key_bytes);
        peer_pub_key.verify(payload, sig_bytes).is_ok()
    }
}
