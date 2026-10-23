use crate::ebpf_bridge::EbpfBridge;
use ring::signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::Arc;
use veil_common::{
    evaluate_lsm_policy, evaluate_peer_policy, NodeLsmPolicy, PeerPolicy, XdpVerdict,
};

pub type NodeId = String;

/// `u64::MAX` による Term 飽和（インクリメント不能・DoS）を防ぐための安全な Term 上限値
pub const MAX_SAFE_TERM: u64 = u64::MAX / 2;

/// 1回の `AppendEntries` RPC に含められる最大ログエントリ数（メモリ枯渇 DoS 防止）
pub const MAX_LOG_ENTRIES_PER_RPC: usize = 1024;

/// QUIC ストリーム上で許容する最大メッセージサイズ（64 KiB）
pub const MAX_WIRE_MESSAGE_BYTES: usize = 64 * 1024;

/// メッシュネットワーク内で合意対象となるコマンド（eBPF XDP 遮断ポリシー & LSM ステルス GC 制御）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshCommand {
    /// ピアの信頼状態（`XDP_PASS` / `XDP_DROP`）を合意によって更新する
    SetPeerTrust { peer_ip: u32, trusted: bool },
    /// 特定ノード（または全ノード）の LSM ステルス GC ロックダウンを発動・解除する
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

/// QUIC 双方向ストリーム上で交換される低レイヤー・コンセンサス RPC メッセージ
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

/// $2f+1$（BFT 定足数）の独立したノードが「同一の `(term, index, entry_hash)` に署名した」ことを証明する
/// 改ざん不可能なマルチシグ合意証明書（Quorum Certificate: QC）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumCertificate {
    pub term: u64,
    pub index: u64,
    pub entry_hash: [u8; 32],
    /// `voter_id -> Ed25519 Signature over (term, index, entry_hash)`
    pub signatures: BTreeMap<NodeId, Vec<u8>>,
}

impl LogEntry {
    /// ログエントリ `(term, index, command)` の決定論的 SHA-256 ダイジェストを計算する
    pub fn digest(&self) -> [u8; 32] {
        use ring::digest;
        let bytes = bincode::serialize(self).expect("LogEntry serialization");
        let hash = digest::digest(&digest::SHA256, &bytes);
        let mut out = [0u8; 32];
        out.copy_from_slice(hash.as_ref());
        out
    }
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

/// ノード ID から決定論的な 32 バイト Ed25519 シードを導出する（テストおよびデフォルト識別子用）
pub fn derive_default_node_seed(node_id: &str) -> [u8; 32] {
    use ring::digest;
    let mut input = Vec::from(&b"VEIL_MESH_NODE_DEFAULT_SEED_V1:"[..]);
    input.extend_from_slice(node_id.as_bytes());
    let digest = digest::digest(&digest::SHA256, &input);
    let mut seed = [0u8; 32];
    seed.copy_from_slice(digest.as_ref());
    seed
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

/// ビザンチン障害（なりすまし・改ざん・偽造投票）を防ぐ Ed25519 暗号署名付き RPC エンベロープ
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedConsensusRpc {
    pub signer_id: NodeId,
    pub rpc: ConsensusRpc,
    pub signature: Vec<u8>,
}

/// ノードの Ed25519 署名鍵ペアおよびクラスタ内ピア公開鍵ディレクトリ
pub struct NodeCryptoIdentity {
    pub node_id: NodeId,
    keypair: Ed25519KeyPair,
    pub public_key_bytes: Vec<u8>,
}

impl NodeCryptoIdentity {
    /// 決定論的シード（32バイト）から Ed25519 鍵ペアを生成する
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

    /// ノード ID からデフォルトの決定論的鍵ペアを生成する
    pub fn default_for_node(node_id: impl Into<String>) -> Self {
        let id = node_id.into();
        let seed = derive_default_node_seed(&id);
        Self::from_seed(id, &seed)
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
}

/// ノードのコンセンサスロール
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

/// 決定論的コンセンサス・ステートマシン（Ed25519 署名付き RPC + マルチシグ QC 必須化 + eBPF ブリッジ同期）
pub struct ConsensusState {
    pub node_id: NodeId,
    pub peers: Vec<NodeId>,
    pub current_term: u64,
    pub voted_for: Option<NodeId>,
    pub log: Vec<LogEntry>, // 1-indexed 相当（内部 Vec の index 0 が log index 1）
    pub commit_index: u64,
    pub last_applied: u64,
    pub role: Role,
    pub leader_id: Option<NodeId>,

    // Candidate 状態での得票セット
    votes_received: BTreeSet<NodeId>,

    // Leader 状態での各フォロワー進捗管理
    pub next_index: BTreeMap<NodeId, u64>,
    pub match_index: BTreeMap<NodeId, u64>,

    /// 各ログ Index ごとに収集された検証済み Ed25519 QC 署名 (`log_index -> (voter_id -> signature)`)
    pub qc_votes: BTreeMap<u64, BTreeMap<NodeId, Vec<u8>>>,

    /// コミット確定時に生成された各ログ Index のマルチシグ合意証明書 (`log_index -> QuorumCertificate`)
    pub commit_qcs: BTreeMap<u64, QuorumCertificate>,

    /// 確定（Commit & Apply）したコンセンサス結果から同期される eBPF `PEER_POLICY_MAP` のミラー状態
    pub ebpf_policy_map: BTreeMap<u32, PeerPolicy>,

    /// 自ノードの LSM ステルス GC ロックダウンポリシー状態 (`LSM_POLICY_MAP[0]`)
    pub lsm_policy: NodeLsmPolicy,

    /// カーネル内の実 Aya eBPF マップ（またはテスト用 MockEbpfBridge）へのリアルタイム書き込みブリッジ
    pub ebpf_bridge: Option<Arc<dyn EbpfBridge>>,

    /// 自ノードの Ed25519 署名鍵アイデンティティ
    pub identity: Arc<NodeCryptoIdentity>,

    /// ビザンチン耐性（Ed25519 署名検証）用の信頼済みピア公開鍵レジストリ (`NodeId -> 32-byte Ed25519 PublicKey`)
    pub peer_public_keys: BTreeMap<NodeId, Vec<u8>>,

    /// 署名不正・なりすまし・二枚舌・QC 欠落・境界値違反により棄却されたパケットのカウンタ
    pub rejected_byzantine_packets: u64,
}

impl std::fmt::Debug for ConsensusState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsensusState")
            .field("node_id", &self.node_id)
            .field("current_term", &self.current_term)
            .field("role", &self.role)
            .field("commit_index", &self.commit_index)
            .field("last_applied", &self.last_applied)
            .field("ebpf_policy_map", &self.ebpf_policy_map)
            .field("lsm_policy", &self.lsm_policy)
            .field(
                "rejected_byzantine_packets",
                &self.rejected_byzantine_packets,
            )
            .finish()
    }
}

impl ConsensusState {
    pub fn new(node_id: impl Into<String>, peers: Vec<NodeId>) -> Self {
        let id_str = node_id.into();
        let identity = Arc::new(NodeCryptoIdentity::default_for_node(&id_str));
        let mut peer_public_keys = BTreeMap::new();
        peer_public_keys.insert(id_str.clone(), identity.public_key_bytes.clone());
        for peer in &peers {
            let peer_id = NodeCryptoIdentity::default_for_node(peer);
            peer_public_keys.insert(peer.clone(), peer_id.public_key_bytes);
        }

        Self {
            node_id: id_str,
            peers,
            current_term: 0,
            voted_for: None,
            log: Vec::new(),
            commit_index: 0,
            last_applied: 0,
            role: Role::Follower,
            leader_id: None,
            votes_received: BTreeSet::new(),
            next_index: BTreeMap::new(),
            match_index: BTreeMap::new(),
            qc_votes: BTreeMap::new(),
            commit_qcs: BTreeMap::new(),
            ebpf_policy_map: BTreeMap::new(),
            lsm_policy: NodeLsmPolicy::new(false, 0),
            ebpf_bridge: None,
            identity,
            peer_public_keys,
            rejected_byzantine_packets: 0,
        }
    }

    /// カーネル eBPF マップへの同期ブリッジを設定する
    pub fn with_ebpf_bridge(mut self, bridge: Arc<dyn EbpfBridge>) -> Self {
        self.ebpf_bridge = Some(bridge);
        self
    }

    /// カスタムの Ed25519 鍵アイデンティティを設定する
    pub fn set_crypto_identity(&mut self, identity: NodeCryptoIdentity) {
        self.peer_public_keys
            .insert(identity.node_id.clone(), identity.public_key_bytes.clone());
        self.identity = Arc::new(identity);
    }

    /// 信頼済みノードの Ed25519 公開鍵群を登録する
    pub fn register_peer_public_keys(&mut self, keys: BTreeMap<NodeId, Vec<u8>>) {
        self.peer_public_keys = keys;
    }

    /// 自ノードの秘密鍵で `ConsensusRpc` に Ed25519 署名を付与する
    pub fn sign_rpc(&self, rpc: ConsensusRpc) -> SignedConsensusRpc {
        self.identity.sign_rpc(rpc)
    }

    /// ビザンチン耐性検証付き RPC ハンドラ（唯一の外部 RPC 入口）：
    /// 1. `signer_id` と RPC 内部の主張送信者 (`sender_id()`) が一致することを確認（なりすまし防止）
    /// 2. `peer_public_keys` に登録された公開鍵で Ed25519 署名を検証（改ざん・偽造防止）
    /// 3. Term の上限・0 チェックによるオーバーフロー / 飽和攻撃の遮断
    pub fn handle_signed_rpc(
        &mut self,
        signed: SignedConsensusRpc,
    ) -> Result<Option<ConsensusRpc>, &'static str> {
        // 1. エンベロープの署名者 ID と RPC ペイロード内の送信者 ID の一致チェック
        if signed.signer_id != signed.rpc.sender_id() {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine Fault Detected: signer_id does not match RPC sender_id (Identity Spoofing)");
        }

        // 2. 登録済み公開鍵の取得
        let pub_key_bytes = match self.peer_public_keys.get(&signed.signer_id) {
            Some(pk) => pk,
            None => {
                self.rejected_byzantine_packets += 1;
                return Err(
                    "Byzantine Fault Detected: unknown signer_id not in trusted mesh registry",
                );
            }
        };

        // 3. Ed25519 署名の暗号学的検証
        let payload = bincode::serialize(&signed.rpc).map_err(|_| "Serialization error")?;
        if payload.len() > MAX_WIRE_MESSAGE_BYTES {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine Fault Detected: RPC payload exceeds MAX_WIRE_MESSAGE_BYTES");
        }
        let peer_pub_key = UnparsedPublicKey::new(&signature::ED25519, pub_key_bytes);
        if peer_pub_key.verify(&payload, &signed.signature).is_err() {
            self.rejected_byzantine_packets += 1;
            return Err(
                "Byzantine Fault Detected: invalid Ed25519 signature or tampered RPC payload",
            );
        }

        // 4. Term 境界値チェック（term == 0 または term > MAX_SAFE_TERM の異常値を拒否）
        let rpc_term = signed.rpc.term();
        if rpc_term == 0 || rpc_term > MAX_SAFE_TERM {
            self.rejected_byzantine_packets += 1;
            return Err(
                "Byzantine Fault Detected: RPC term out of safe bounds (0 or > MAX_SAFE_TERM)",
            );
        }

        Ok(self.process_verified_rpc(signed.rpc))
    }

    /// クラスタの総ノード数（自身 + ピア）
    pub fn cluster_size(&self) -> usize {
        self.peers.len() + 1
    }

    /// BFT 定足数（Quorum）に必要なノード数: $\lfloor 2N / 3 \rfloor + 1$
    /// - $N = 4$ ($3f+1, f=1$) のとき $2f+1 = 3$ 台の署名が必要
    /// - 任意の2つの Quorum ($3$ 台) は最低 $2$ 台で交差し、少なくとも $1$ 台の誠実なノードを含む
    pub fn quorum_size(&self) -> usize {
        (2 * self.cluster_size()) / 3 + 1
    }

    pub fn last_log_index(&self) -> u64 {
        self.log.len() as u64
    }

    pub fn last_log_term(&self) -> u64 {
        self.log.last().map(|e| e.term).unwrap_or(0)
    }

    pub fn term_at(&self, index: u64) -> u64 {
        if index == 0 {
            return 0;
        }
        self.log
            .get((index - 1) as usize)
            .map(|e| e.term)
            .unwrap_or(0)
    }

    /// 選挙タイムアウト発火時：新しい Term へ遷移し Candidate となって投票要求を生成する
    pub fn start_election(&mut self) -> ConsensusRpc {
        if self.current_term < MAX_SAFE_TERM {
            self.current_term += 1;
        }
        self.role = Role::Candidate;
        self.voted_for = Some(self.node_id.clone());
        self.leader_id = None;
        self.votes_received.clear();
        self.votes_received.insert(self.node_id.clone());

        if self.votes_received.len() >= self.quorum_size() {
            self.become_leader();
        }

        ConsensusRpc::RequestVote {
            term: self.current_term,
            candidate_id: self.node_id.clone(),
            last_log_index: self.last_log_index(),
            last_log_term: self.last_log_term(),
        }
    }

    /// 選挙を開始し、Ed25519 署名付きの `RequestVote` RPC を返す
    pub fn start_signed_election(&mut self) -> SignedConsensusRpc {
        let rv = self.start_election();
        self.sign_rpc(rv)
    }

    fn become_leader(&mut self) {
        self.role = Role::Leader;
        self.leader_id = Some(self.node_id.clone());
        let next = self.last_log_index() + 1;
        self.next_index.clear();
        self.match_index.clear();
        for peer in &self.peers {
            self.next_index.insert(peer.clone(), next);
            self.match_index.insert(peer.clone(), 0);
        }
    }

    fn step_down(&mut self, new_term: u64) {
        self.current_term = new_term;
        self.role = Role::Follower;
        self.voted_for = None;
        self.votes_received.clear();
    }

    /// リーダーとして新しいコマンドを提案し、自身の Ed25519 QC 投票署名を記録してログへ追加する
    pub fn propose(&mut self, command: MeshCommand) -> Result<LogEntry, &'static str> {
        if self.role != Role::Leader {
            return Err("Only leader can propose commands");
        }
        let index = self.last_log_index() + 1;
        let entry = LogEntry {
            term: self.current_term,
            index,
            command,
        };
        let digest = entry.digest();
        let my_qc_sig = self.identity.sign_qc_vote(entry.term, index, &digest);
        self.log.push(entry.clone());
        self.qc_votes
            .entry(index)
            .or_default()
            .insert(self.node_id.clone(), my_qc_sig);

        self.advance_commit_index();
        Ok(entry)
    }

    /// 特定のフォロワーへ送信すべき `AppendEntries` RPC を構築する（最新の `QuorumCertificate` 付き）
    pub fn make_append_entries(&self, peer_id: &str) -> Option<ConsensusRpc> {
        if self.role != Role::Leader {
            return None;
        }
        let next_idx = *self.next_index.get(peer_id).unwrap_or(&1);
        let prev_log_index = next_idx.saturating_sub(1);
        let prev_log_term = self.term_at(prev_log_index);

        let entries = if next_idx <= self.last_log_index() {
            self.log[(next_idx - 1) as usize..].to_vec()
        } else {
            Vec::new()
        };

        let commit_qc = if self.commit_index > 0 {
            self.commit_qcs.get(&self.commit_index).cloned()
        } else {
            None
        };

        Some(ConsensusRpc::AppendEntries {
            term: self.current_term,
            leader_id: self.node_id.clone(),
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit: self.commit_index,
            commit_qc,
        })
    }

    /// マルチシグ合意証明書（Quorum Certificate: QC）および二枚舌（Equivocation）の厳密検証：
    /// 1. `qc.index` が有効範囲 (`1..=last_log_index`) にあり、自身の該当ログエントリと `term` および `entry_hash` が完全一致することを確認
    /// 2. `QuorumCertificate` に含まれる異なる信頼済みノードからの Ed25519 署名が BFT 定足数（`quorum_size()` = $\lfloor 2N/3 \rfloor + 1$）以上あり、
    ///    すべて同一の `(term, index, entry_hash)` に対する有効な署名であることを検証する
    pub fn verify_quorum_certificate(
        &mut self,
        qc: &QuorumCertificate,
    ) -> Result<(), &'static str> {
        if qc.index == 0 || qc.term == 0 || qc.term > MAX_SAFE_TERM {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine QC Invalid: index or term out of valid bounds");
        }

        // 1. 自身のログに該当 index が存在し、ハッシュが一致するか確認（二枚舌 Equivocation の検知）
        match self.log.get((qc.index - 1) as usize) {
            Some(local_entry) => {
                if local_entry.term != qc.term || local_entry.digest() != qc.entry_hash {
                    self.rejected_byzantine_packets += 1;
                    return Err("Byzantine Equivocation Detected: Leader sent conflicting log entries or mismatched QC hash!");
                }
            }
            None => {
                self.rejected_byzantine_packets += 1;
                return Err(
                    "Byzantine QC Invalid: QC refers to a log index not present on follower",
                );
            }
        }

        // 2. 重複のない有効な署名数が BFT Quorum 以上あるか検証
        let payload = qc_vote_payload(qc.term, qc.index, &qc.entry_hash);
        let mut valid_signers = BTreeSet::new();

        for (signer_id, sig_bytes) in &qc.signatures {
            if let Some(pk_bytes) = self.peer_public_keys.get(signer_id) {
                let pub_key = UnparsedPublicKey::new(&signature::ED25519, pk_bytes);
                if pub_key.verify(&payload, sig_bytes).is_ok() {
                    valid_signers.insert(signer_id.clone());
                } else {
                    self.rejected_byzantine_packets += 1;
                    return Err("Byzantine QC Forgery Detected: Invalid Ed25519 signature inside QuorumCertificate!");
                }
            } else {
                self.rejected_byzantine_packets += 1;
                return Err(
                    "Byzantine QC Forgery Detected: Unknown signer inside QuorumCertificate!",
                );
            }
        }

        if valid_signers.len() < self.quorum_size() {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine Unilateral Commit Rejected: QuorumCertificate lacks required multi-signature quorum!");
        }

        Ok(())
    }

    /// 署名検証を通過したコンセンサス RPC の内部処理（直接の外部呼び出し不可）
    fn process_verified_rpc(&mut self, rpc: ConsensusRpc) -> Option<ConsensusRpc> {
        match rpc {
            ConsensusRpc::RequestVote {
                term,
                candidate_id,
                last_log_index,
                last_log_term,
            } => Some(self.on_request_vote(term, candidate_id, last_log_index, last_log_term)),

            ConsensusRpc::RequestVoteResponse {
                term,
                voter_id,
                vote_granted,
            } => {
                self.on_request_vote_response(term, voter_id, vote_granted);
                None
            }

            ConsensusRpc::AppendEntries {
                term,
                leader_id,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
                commit_qc,
            } => Some(self.on_append_entries(
                term,
                leader_id,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
                commit_qc,
            )),

            ConsensusRpc::AppendEntriesResponse {
                term,
                follower_id,
                success,
                match_index,
                entry_hash,
                qc_vote_signature,
            } => {
                self.on_append_entries_response(
                    term,
                    follower_id,
                    success,
                    match_index,
                    entry_hash,
                    qc_vote_signature,
                );
                None
            }
        }
    }

    fn on_request_vote(
        &mut self,
        term: u64,
        candidate_id: NodeId,
        last_log_index: u64,
        last_log_term: u64,
    ) -> ConsensusRpc {
        if term > self.current_term {
            self.step_down(term);
        }

        let my_last_term = self.last_log_term();
        let my_last_index = self.last_log_index();

        let log_is_up_to_date = (last_log_term > my_last_term)
            || (last_log_term == my_last_term && last_log_index >= my_last_index);

        let can_vote = match &self.voted_for {
            None => true,
            Some(id) => id == &candidate_id,
        };

        let vote_granted = term == self.current_term && can_vote && log_is_up_to_date;
        if vote_granted {
            self.voted_for = Some(candidate_id);
        }

        ConsensusRpc::RequestVoteResponse {
            term: self.current_term,
            voter_id: self.node_id.clone(),
            vote_granted,
        }
    }

    fn on_request_vote_response(&mut self, term: u64, voter_id: NodeId, vote_granted: bool) {
        if term > self.current_term {
            self.step_down(term);
            return;
        }
        if self.role != Role::Candidate || term != self.current_term {
            return;
        }
        if vote_granted && (voter_id == self.node_id || self.peers.contains(&voter_id)) {
            self.votes_received.insert(voter_id);
            if self.votes_received.len() >= self.quorum_size() {
                self.become_leader();
            }
        }
    }

    fn make_append_response(&self, success: bool) -> ConsensusRpc {
        let match_index = self.last_log_index();
        let (entry_term, entry_hash) = self
            .log
            .last()
            .map(|e| (e.term, e.digest()))
            .unwrap_or((self.current_term, [0u8; 32]));
        let qc_vote_signature = if success && match_index > 0 {
            self.identity
                .sign_qc_vote(entry_term, match_index, &entry_hash)
        } else {
            Vec::new()
        };
        ConsensusRpc::AppendEntriesResponse {
            term: self.current_term,
            follower_id: self.node_id.clone(),
            success,
            match_index,
            entry_hash,
            qc_vote_signature,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_append_entries(
        &mut self,
        term: u64,
        leader_id: NodeId,
        prev_log_index: u64,
        prev_log_term: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
        commit_qc: Option<QuorumCertificate>,
    ) -> ConsensusRpc {
        if term < self.current_term {
            return self.make_append_response(false);
        }

        // エントリ配列のサイズ・連番 Index (index >= 1)・Term 整合性を事前検証（index=0 アンダーフロー Panic 防止）
        if entries.len() > MAX_LOG_ENTRIES_PER_RPC {
            self.rejected_byzantine_packets += 1;
            return self.make_append_response(false);
        }
        for (offset, entry) in entries.iter().enumerate() {
            let expected_idx = match prev_log_index.checked_add((offset as u64) + 1) {
                Some(idx) if idx >= 1 => idx,
                _ => {
                    self.rejected_byzantine_packets += 1;
                    return self.make_append_response(false);
                }
            };
            if entry.index != expected_idx
                || entry.term == 0
                || entry.term > term
                || entry.term > MAX_SAFE_TERM
            {
                self.rejected_byzantine_packets += 1;
                return self.make_append_response(false);
            }
        }

        if term > self.current_term || self.role == Role::Candidate {
            self.step_down(term);
        }
        self.leader_id = Some(leader_id);

        if prev_log_index > self.last_log_index() || self.term_at(prev_log_index) != prev_log_term {
            return self.make_append_response(false);
        }

        for entry in entries {
            let idx = entry.index;
            if idx <= self.last_log_index() {
                // 同一 Term・同一 Index なのに中身（ハッシュ）が違うエントリをリーダーが送ってきた場合は「二枚舌（Equivocation）」として拒否！
                let existing = &self.log[(idx - 1) as usize];
                if existing.term == entry.term && existing.digest() != entry.digest() {
                    self.rejected_byzantine_packets += 1;
                    return self.make_append_response(false);
                }
                // コミット済み領域（idx <= commit_index）の書き換え試行もビザンチン違反として拒否
                if idx <= self.commit_index && existing.digest() != entry.digest() {
                    self.rejected_byzantine_packets += 1;
                    return self.make_append_response(false);
                }
                if self.term_at(idx) != entry.term {
                    self.log.truncate((idx - 1) as usize);
                    self.log.push(entry);
                }
            } else {
                self.log.push(entry);
            }
        }

        // リーダーが commit_index を進めるよう要求している場合、有効なマルチシグ QuorumCertificate を必須とする！
        let target_commit = leader_commit.min(self.last_log_index());
        if target_commit > self.commit_index {
            match commit_qc {
                Some(ref qc) if qc.index == target_commit => {
                    if self.verify_quorum_certificate(qc).is_err() {
                        return self.make_append_response(false);
                    }
                    self.commit_qcs.insert(qc.index, qc.clone());
                    self.commit_index = target_commit;
                    self.apply_committed_entries();
                }
                _ => {
                    // commit_qc が None、または対象 index と不一致の単独コミット要求は拒否！
                    self.rejected_byzantine_packets += 1;
                    return self.make_append_response(false);
                }
            }
        } else if let Some(ref qc) = commit_qc {
            if self.verify_quorum_certificate(qc).is_err() {
                return self.make_append_response(false);
            }
        }

        self.make_append_response(true)
    }

    fn on_append_entries_response(
        &mut self,
        term: u64,
        follower_id: NodeId,
        success: bool,
        match_index: u64,
        entry_hash: [u8; 32],
        qc_vote_signature: Vec<u8>,
    ) {
        if term > self.current_term {
            self.step_down(term);
            return;
        }
        if self.role != Role::Leader || term != self.current_term {
            return;
        }
        if !self.peers.contains(&follower_id) {
            self.rejected_byzantine_packets += 1;
            return;
        }

        if success {
            if match_index > self.last_log_index() {
                self.rejected_byzantine_packets += 1;
                return;
            }
            if match_index > 0 {
                let leader_entry = &self.log[(match_index - 1) as usize];
                if leader_entry.digest() != entry_hash {
                    self.rejected_byzantine_packets += 1;
                    return;
                }
                let pk_bytes = match self.peer_public_keys.get(&follower_id) {
                    Some(pk) => pk,
                    None => {
                        self.rejected_byzantine_packets += 1;
                        return;
                    }
                };
                let payload = qc_vote_payload(leader_entry.term, match_index, &entry_hash);
                let pub_key = UnparsedPublicKey::new(&signature::ED25519, pk_bytes);
                if pub_key.verify(&payload, &qc_vote_signature).is_err() {
                    self.rejected_byzantine_packets += 1;
                    return;
                }
                self.qc_votes
                    .entry(match_index)
                    .or_default()
                    .insert(follower_id.clone(), qc_vote_signature);
            }

            self.match_index.insert(follower_id.clone(), match_index);
            self.next_index.insert(follower_id, match_index + 1);
            self.advance_commit_index();
        } else {
            let next = self.next_index.entry(follower_id).or_insert(1);
            if *next > 1 {
                *next -= 1;
            }
        }
    }

    fn advance_commit_index(&mut self) {
        if self.role != Role::Leader {
            return;
        }
        for n in (self.commit_index + 1)..=self.last_log_index() {
            if self.term_at(n) != self.current_term {
                continue;
            }
            let entry = &self.log[(n - 1) as usize];
            let entry_hash = entry.digest();
            let entry_term = entry.term;

            let sig_map = match self.qc_votes.get(&n) {
                Some(m) => m.clone(),
                None => continue,
            };

            if sig_map.len() >= self.quorum_size() {
                let qc = QuorumCertificate {
                    term: entry_term,
                    index: n,
                    entry_hash,
                    signatures: sig_map,
                };
                if self.verify_quorum_certificate(&qc).is_ok() {
                    self.commit_qcs.insert(n, qc);
                    self.commit_index = n;
                }
            }
        }
        self.apply_committed_entries();
    }

    /// コミット済みログを順番に適用し、eBPF `PEER_POLICY_MAP` および `LSM_POLICY_MAP` の状態を更新・カーネル同期する
    fn apply_committed_entries(&mut self) {
        while self.last_applied < self.commit_index {
            self.last_applied += 1;
            let entry = &self.log[(self.last_applied - 1) as usize];
            match &entry.command {
                MeshCommand::SetPeerTrust { peer_ip, trusted } => {
                    let policy = PeerPolicy::new(*trusted, entry.index as u32);
                    self.ebpf_policy_map.insert(*peer_ip, policy);
                    if let Some(ref bridge) = self.ebpf_bridge {
                        let _ = bridge.sync_peer_policy(*peer_ip, policy);
                    }
                }
                MeshCommand::SetNodeLockdown {
                    target_node,
                    lockdown,
                } => {
                    if target_node == &self.node_id || target_node == "*" {
                        let policy = NodeLsmPolicy::new(*lockdown, entry.index as u32);
                        self.lsm_policy = policy;
                        if let Some(ref bridge) = self.ebpf_bridge {
                            let _ = bridge.sync_lsm_policy(policy);
                        }
                    }
                }
                MeshCommand::NoOp => {}
            }
        }
    }

    /// カーネル eBPF XDP プログラム (`veil_ebpf::try_veil_xdp_filter`) と同一のロジックでパケット通過・遮断を判定する
    pub fn check_xdp_packet(&self, src_ip: Ipv4Addr) -> XdpVerdict {
        let ip_u32 = u32::from(src_ip);
        let policy = self.ebpf_policy_map.get(&ip_u32);
        evaluate_peer_policy(policy)
    }

    /// カーネル eBPF LSM プログラム (`veil_ebpf::veil_lsm_socket_connect`) と同一のロジックでソケット接続の許可 (`0`) / 拒否 (`-EPERM = -1`) を判定する
    pub fn check_lsm_socket_connect(&self) -> i32 {
        evaluate_lsm_policy(Some(&self.lsm_policy))
    }
}
