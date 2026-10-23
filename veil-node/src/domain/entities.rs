use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

pub type NodeId = String;

/// メッシュネットワーク内で合意対象となるコマンド（eBPF XDP 遮断ポリシー & LSM 制御）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshCommand {
    /// ピアの信頼状態（`XDP_PASS` / `XDP_DROP`）を合意によって更新する
    SetPeerTrust { peer_ip: u32, trusted: bool },
    /// 特定ノード（または全ノード）の LSM ロックダウンを発動・解除する
    SetNodeLockdown { target_node: NodeId, lockdown: bool },
    /// リーダーによる定期的な生存確認とコミット進行用の NoOp
    NoOp,
}

impl MeshCommand {
    pub fn set_trust(ip: Ipv4Addr, trusted: bool) -> Self {
        Self::SetPeerTrust {
            peer_ip: u32::from(ip),
            trusted,
        }
    }

    pub fn set_lockdown(target_node: impl Into<String>, lockdown: bool) -> Self {
        Self::SetNodeLockdown {
            target_node: target_node.into(),
            lockdown,
        }
    }
}

/// レプリケートされるコンセンサスログのエントリ
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub term: u64,
    pub index: u64,
    pub command: MeshCommand,
}

impl LogEntry {
    /// ログエントリ `(term, index, command)` の決定論的 SHA-256 ダイジェストを計算する（Index 1 / 単体用）
    pub fn digest(&self) -> [u8; 32] {
        self.digest_with_parent(&[0u8; 32])
    }

    /// 直前エントリまでの累積チェーンハッシュ `parent_hash` を暗号学的に束縛した SHA-256 ダイジェストを計算する（R1 Prefix 改ざん防止）
    pub fn digest_with_parent(&self, parent_hash: &[u8; 32]) -> [u8; 32] {
        use ring::digest;
        let bytes = bincode::serialize(self).expect("LogEntry serialization");
        let mut ctx = digest::Context::new(&digest::SHA256);
        if *parent_hash != [0u8; 32] {
            ctx.update(b"VEIL_LOG_CHAIN_V2:");
            ctx.update(parent_hash);
        }
        ctx.update(&bytes);
        let hash = ctx.finish();
        let mut out = [0u8; 32];
        out.copy_from_slice(hash.as_ref());
        out
    }
}

/// $\lfloor 2N/3 \rfloor + 1$ の独立したノードが「同一の `(term, index, entry_hash)` に署名した」ことを証明する
/// マルチシグ合意証明書（Quorum Certificate: QC）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumCertificate {
    pub term: u64,
    pub index: u64,
    pub entry_hash: [u8; 32],
    /// `voter_id -> Ed25519 Signature over (term, index, entry_hash)`
    pub signatures: BTreeMap<NodeId, Vec<u8>>,
}

/// QC 署名対象のバイト列 `(term, index, entry_hash)` を構築する（ドメイン分離プレフィックス付き）
pub fn qc_vote_payload(term: u64, index: u64, entry_hash: &[u8; 32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(16 + 8 + 8 + 32);
    buf.extend_from_slice(b"VEIL_QC_VOTE_V1:");
    buf.extend_from_slice(&term.to_be_bytes());
    buf.extend_from_slice(&index.to_be_bytes());
    buf.extend_from_slice(entry_hash);
    buf
}

/// QUIC 双方向ストリーム上で交換されるコンセンサス RPC メッセージ
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusRpc {
    /// 候補者によるリーダー選出の投票要求 (RequestVote)
    RequestVote {
        term: u64,
        candidate_id: NodeId,
        last_log_index: u64,
        last_log_term: u64,
    },
    /// 投票要求への応答
    RequestVoteResponse {
        term: u64,
        voter_id: NodeId,
        vote_granted: bool,
    },
    /// リーダーからのログ複製およびハートビート (AppendEntries)
    AppendEntries {
        term: u64,
        leader_id: NodeId,
        prev_log_index: u64,
        prev_log_term: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
        /// リーダー単独のコミット捏造および二枚舌（Equivocation）を封じる必須マルチシグ合意証明書（Quorum Certificate）
        commit_qc: Option<QuorumCertificate>,
    },
    /// ログ複製への応答（合意されたエントリの暗号ダイジェスト + QC 用 Ed25519 投票署名付き）
    AppendEntriesResponse {
        term: u64,
        follower_id: NodeId,
        success: bool,
        match_index: u64,
        entry_hash: [u8; 32],
        /// フォロワーが `(term, match_index, entry_hash)` に対して署名した Ed25519 QC 投票署名
        qc_vote_signature: Vec<u8>,
    },
}

impl ConsensusRpc {
    /// RPC メッセージの発信者として主張されている `NodeId` を取得する
    pub fn sender_id(&self) -> &str {
        match self {
            ConsensusRpc::RequestVote { candidate_id, .. } => candidate_id,
            ConsensusRpc::RequestVoteResponse { voter_id, .. } => voter_id,
            ConsensusRpc::AppendEntries { leader_id, .. } => leader_id,
            ConsensusRpc::AppendEntriesResponse { follower_id, .. } => follower_id,
        }
    }

    /// RPC メッセージに含まれる Term 値を取得する
    pub fn term(&self) -> u64 {
        match self {
            ConsensusRpc::RequestVote { term, .. }
            | ConsensusRpc::RequestVoteResponse { term, .. }
            | ConsensusRpc::AppendEntries { term, .. }
            | ConsensusRpc::AppendEntriesResponse { term, .. } => *term,
        }
    }
}

/// なりすまし・改ざん・偽造投票を防ぐ Ed25519 暗号署名付き RPC エンベロープ
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedConsensusRpc {
    pub signer_id: NodeId,
    pub rpc: ConsensusRpc,
    pub signature: Vec<u8>,
}

/// ノードのコンセンサスロール
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}
