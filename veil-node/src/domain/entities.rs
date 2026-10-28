use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

pub type NodeId = String;

/// Static signature namespace. Epochs identify configurations, not Raft terms.
/// Operators must use a unique cluster ID and increment the epoch on config changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningContext {
    pub protocol_version: u32,
    pub cluster_id: String,
    pub config_epoch: u64,
    pub config_digest: [u8; 32],
}

impl SigningContext {
    pub fn new(cluster_id: impl Into<String>, config_epoch: u64) -> Result<Self, &'static str> {
        let context = Self {
            protocol_version: 3,
            cluster_id: cluster_id.into(),
            config_epoch,
            config_digest: [0; 32],
        };
        context.validate()?;
        Ok(context)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.protocol_version, 3..=6)
            || self.config_epoch == 0
            || self.cluster_id.is_empty()
            || self.cluster_id.len() > 128
            || !self
                .cluster_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err("Invalid signature context: require supported protocol, positive epoch and 1..128 ASCII ID characters");
        }
        Ok(())
    }

    /// Only the simulator uses this shared namespace. Daemon startup requires explicit values.
    pub fn simulation() -> Self {
        Self::new("veil-simulation", 1).expect("static context")
    }
}

/// RPC schema v4 binds AppendEntries to its parent prefix hash. The RPC domain
/// is separate from QC/context/WAL versions: legacy RPC signatures fail closed,
/// while existing commit certificates and durable vote fences keep their meaning.
pub fn rpc_signing_payload(context: &SigningContext, signer: &str, rpc: &ConsensusRpc) -> Vec<u8> {
    bincode::serialize(&("VEIL_RPC_V4", context, signer, rpc)).expect("RPC payload serialization")
}

/// TTL 付きピア信頼・遮断ルール（マイクロバッチ合意 `SetPeerTrustBatch` 用）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerTrustRule {
    pub peer_ip: u32,
    pub trusted: bool,
    /// Requested duration. Proposals with TTL > 0 are stamped with a signed
    /// absolute deadline before entering the log. Zero is allowed only for unban.
    pub ttl_secs: u32,
}

/// A rule and its explicit signed UTC/POSIX deadline. Zero is canonical only
/// for ttl_secs == 0. The batch carries the common issuance timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadlinePeerTrustRule {
    pub rule: PeerTrustRule,
    pub not_after_unix_ns: u64,
}

impl PeerTrustRule {
    pub fn new(ip: Ipv4Addr, trusted: bool, ttl_secs: u32) -> Self {
        Self {
            peer_ip: u32::from(ip),
            trusted,
            ttl_secs,
        }
    }
}

/// メッシュネットワーク内で合意対象となるコマンド（eBPF XDP 遮断ポリシー & LSM 制御）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshCommand {
    /// ピアの信頼状態（`XDP_PASS` / `XDP_DROP`）を合意によって更新する
    SetPeerTrust {
        peer_ip: u32,
        trusted: bool,
    },
    /// 複数のピア信頼ルール（TTL 付き）をマイクロバッチで一括合意・更新する（v0.5.0）
    SetPeerTrustBatch {
        rules: Vec<PeerTrustRule>,
    },
    /// 特定ノード（または全ノード）の LSM ロックダウンを発動・解除する
    SetNodeLockdown {
        target_node: NodeId,
        lockdown: bool,
    },
    /// リーダーによる定期的な生存確認とコミット進行用の NoOp
    NoOp,
    // Append only: keep the bincode tags and hashes of historical commands.
    SetPeerTrustLeaseBatch {
        issued_at_unix_ns: u64,
        rules: Vec<DeadlinePeerTrustRule>,
    },
    /// Consensus-authenticated retirement of already expired policy tombstones.
    RetireExpired {
        through_unix_ns: u64,
    },
    AuthorizedPolicy(Box<super::ApprovedPolicy>),
}

impl MeshCommand {
    pub fn batch_rules(&self) -> impl Iterator<Item = &PeerTrustRule> {
        let (plain, leased): (&[PeerTrustRule], &[DeadlinePeerTrustRule]) = match self {
            Self::SetPeerTrustBatch { rules } => (rules, &[]),
            Self::SetPeerTrustLeaseBatch { rules, .. } => (&[], rules),
            Self::AuthorizedPolicy(a) => (&[], std::slice::from_ref(&a.rule)),
            _ => (&[], &[]),
        };
        plain.iter().chain(leased.iter().map(|r| &r.rule))
    }

    pub fn lease_parts(&self) -> Option<(u64, &[DeadlinePeerTrustRule])> {
        match self {
            Self::SetPeerTrustLeaseBatch {
                issued_at_unix_ns,
                rules,
            } => Some((*issued_at_unix_ns, rules)),
            Self::AuthorizedPolicy(a) => Some((a.issued_at_unix_ns, std::slice::from_ref(&a.rule))),
            _ => None,
        }
    }

    pub fn set_trust(ip: Ipv4Addr, trusted: bool) -> Self {
        Self::SetPeerTrust {
            peer_ip: u32::from(ip),
            trusted,
        }
    }

    pub fn set_trust_with_ttl(ip: Ipv4Addr, trusted: bool, ttl_secs: u32) -> Self {
        Self::SetPeerTrustBatch {
            rules: vec![PeerTrustRule::new(ip, trusted, ttl_secs)],
        }
    }

    pub fn set_trust_batch(rules: Vec<PeerTrustRule>) -> Self {
        Self::SetPeerTrustBatch { rules }
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
pub fn qc_vote_payload(
    context: &SigningContext,
    term: u64,
    index: u64,
    entry_hash: &[u8; 32],
) -> Vec<u8> {
    bincode::serialize(&("VEIL_QC_VOTE_V3", context, term, index, entry_hash))
        .expect("QC payload serialization")
}

/// Timeout votes have a separate domain and the same static context as RPCs and QCs.
pub fn timeout_vote_payload(
    context: &SigningContext,
    term: u64,
    high_qc_term: u64,
    high_qc_index: u64,
) -> Vec<u8> {
    bincode::serialize(&(
        "VEIL_TC_VOTE_V3",
        context,
        term,
        high_qc_term,
        high_qc_index,
    ))
    .expect("TC payload serialization")
}

/// $\lfloor 2N/3 \rfloor + 1$ 台のノードが特定 Term のタイムアウトに合意したことを証明する View-Change 証明書 (`TimeoutCertificate`: TC)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutCertificate {
    pub term: u64,
    pub highest_qc: Option<QuorumCertificate>,
    /// `voter_id -> (high_qc_term, high_qc_index, Ed25519 Signature)`
    pub votes: BTreeMap<NodeId, (u64, u64, Vec<u8>)>,
}

/// A member-witnessed immutable finite BAN, independent of a leader/log index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanRequest {
    pub origin: NodeId,
    pub command: MeshCommand,
    pub signature: Vec<u8>,
}
impl BanRequest {
    pub fn payload(&self, context: &SigningContext) -> Vec<u8> {
        bincode::serialize(&("VEIL_BAN_REQUEST_V1", context, &self.origin, &self.command))
            .expect("request payload")
    }
    pub fn id(&self, context: &SigningContext) -> [u8; 32] {
        ring::digest::digest(&ring::digest::SHA256, &self.payload(context))
            .as_ref()
            .try_into()
            .unwrap()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressVote {
    pub term: u64,
    pub leader: NodeId,
    pub voter: NodeId,
    pub signature: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressCertificate {
    pub term: u64,
    pub leader: NodeId,
    pub request: BanRequest,
    pub votes: BTreeMap<NodeId, Vec<u8>>,
}
pub fn progress_payload(
    context: &SigningContext,
    term: u64,
    leader: &str,
    request: &[u8; 32],
) -> Vec<u8> {
    bincode::serialize(&("VEIL_REQUEST_TIMEOUT_V1", context, term, leader, request))
        .expect("progress payload")
}

/// Admission evidence for a term, not a leader election or a log commit.
/// Pre-votes are non-exclusive: durable RequestVote and QC rules still apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TermCertificate {
    PreVote {
        term: u64,
        votes: BTreeMap<NodeId, Vec<u8>>,
    },
    Timeout(TimeoutCertificate),
    Progress(ProgressCertificate),
}

impl TermCertificate {
    pub fn term(&self) -> u64 {
        match self {
            Self::PreVote { term, .. } => *term,
            Self::Timeout(tc) => tc.term.saturating_add(1),
            Self::Progress(pc) => pc.term.saturating_add(1),
        }
    }
}

pub fn term_support_payload(context: &SigningContext, term: u64) -> Vec<u8> {
    bincode::serialize(&("VEIL_TERM_SUPPORT_V1", context, term)).expect("term payload")
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
        prev_log_hash: [u8; 32],
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
    /// リーダー沈黙・検閲時にフォロワーがブロードキャストする View-Change タイムアウト投票 (v0.4.0)
    TimeoutVote {
        term: u64,
        voter_id: NodeId,
        highest_qc: Option<QuorumCertificate>,
        tc_vote_signature: Vec<u8>,
    },
    // Append-only wire tags. Old implementations cannot drive the new protocol.
    PreVote {
        term: u64,
        candidate_id: NodeId,
        last_log_index: u64,
        last_log_term: u64,
    },
    PreVoteResponse {
        term: u64,
        voter_id: NodeId,
        support_signature: Vec<u8>,
    },
    TermCertified {
        term: u64,
        sender_id: NodeId,
        certificate: TermCertificate,
        rpc_bytes: Vec<u8>,
    },
    LivenessProbe {
        term: u64,
        sender_id: NodeId,
        nonce: [u8; 32],
    },
    LivenessAck {
        term: u64,
        sender_id: NodeId,
        nonce: [u8; 32],
    },
    ProgressGossip {
        term: u64,
        sender_id: NodeId,
        request: Option<BanRequest>,
        vote: Option<ProgressVote>,
        certificate: Option<ProgressCertificate>,
    },
    TwoPhase {
        term: u64,
        sender_id: NodeId,
        message: Box<super::PhaseMessage>,
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
            ConsensusRpc::TimeoutVote { voter_id, .. }
            | ConsensusRpc::PreVoteResponse { voter_id, .. } => voter_id,
            ConsensusRpc::PreVote { candidate_id, .. } => candidate_id,
            ConsensusRpc::TwoPhase { sender_id, .. }
            | ConsensusRpc::TermCertified { sender_id, .. }
            | ConsensusRpc::ProgressGossip { sender_id, .. } => sender_id,
            ConsensusRpc::LivenessProbe { sender_id, .. }
            | ConsensusRpc::LivenessAck { sender_id, .. } => sender_id,
        }
    }

    /// RPC メッセージに含まれる Term 値を取得する
    pub fn term(&self) -> u64 {
        match self {
            ConsensusRpc::RequestVote { term, .. }
            | ConsensusRpc::RequestVoteResponse { term, .. }
            | ConsensusRpc::AppendEntries { term, .. }
            | ConsensusRpc::AppendEntriesResponse { term, .. }
            | ConsensusRpc::TimeoutVote { term, .. }
            | ConsensusRpc::PreVote { term, .. }
            | ConsensusRpc::PreVoteResponse { term, .. }
            | ConsensusRpc::LivenessProbe { term, .. }
            | ConsensusRpc::LivenessAck { term, .. } => *term,
            ConsensusRpc::TwoPhase { term, .. }
            | ConsensusRpc::TermCertified { term, .. }
            | ConsensusRpc::ProgressGossip { term, .. } => *term,
        }
    }
}

/// なりすまし・改ざん・偽造投票を防ぐ Ed25519 暗号署名付き RPC エンベロープ
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedConsensusRpc {
    pub context: SigningContext,
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
