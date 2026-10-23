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

/// メッシュネットワーク内で合意対象となるコマンド（eBPF XDP 遮断ポリシー & LSM ステルス GC 制御）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshCommand {
    /// ピアの信頼状態（`XDP_PASS` / `XDP_DROP`）を合意によって更新する
    SetPeerTrust {
        peer_ip: u32,
        trusted: bool,
    },
    /// 特定ノード（または全ノード）の LSM ステルス GC ロックダウンを発動・解除する
    SetNodeLockdown {
        target_node: NodeId,
        lockdown: bool,
    },
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
        /// リーダー自身の裏切り・二枚舌（Equivocation）を封じるマルチシグ合意証明書（Quorum Certificate）
        commit_qc: Option<QuorumCertificate>,
    },
    /// ログ複製への応答（合意されたエントリの暗号ダイジェスト付き）
    AppendEntriesResponse {
        term: u64,
        follower_id: NodeId,
        success: bool,
        match_index: u64,
        entry_hash: [u8; 32],
    },
}

/// 過半数（または $2f+1$）の独立したノードが「同一の `(term, index, entry_hash)` に署名した」ことを証明する
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

/// QC 署名対象のバイト列 `(term, index, entry_hash)` を構築する
pub fn qc_vote_payload(term: u64, index: u64, entry_hash: &[u8; 32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + 8 + 32);
    buf.extend_from_slice(&term.to_be_bytes());
    buf.extend_from_slice(&index.to_be_bytes());
    buf.extend_from_slice(entry_hash);
    buf
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
    /// 決定論的シード（32バイト）または乱数から Ed25519 鍵ペアを生成する
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

/// 決定論的コンセンサス・ステートマシン（Raft + Ed25519 BFT 署名検証 + eBPF ブリッジ同期）
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

    /// 確定（Commit & Apply）したコンセンサス結果から同期される eBPF `PEER_POLICY_MAP` のミラー状態
    pub ebpf_policy_map: BTreeMap<u32, PeerPolicy>,

    /// 自ノードの LSM ステルス GC ロックダウンポリシー状態 (`LSM_POLICY_MAP[0]`)
    pub lsm_policy: NodeLsmPolicy,

    /// カーネル内の実 Aya eBPF マップ（またはテスト用 MockEbpfBridge）へのリアルタイム書き込みブリッジ
    pub ebpf_bridge: Option<Arc<dyn EbpfBridge>>,

    /// ビザンチン耐性（Ed25519 署名検証）用の信頼済みピア公開鍵レジストリ (`NodeId -> 32-byte Ed25519 PublicKey`)
    pub peer_public_keys: BTreeMap<NodeId, Vec<u8>>,

    /// 署名不正やなりすましにより棄却されたビザンチンパケットのカウンタ
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
            .field("rejected_byzantine_packets", &self.rejected_byzantine_packets)
            .finish()
    }
}

impl ConsensusState {
    pub fn new(node_id: impl Into<String>, peers: Vec<NodeId>) -> Self {
        Self {
            node_id: node_id.into(),
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
            ebpf_policy_map: BTreeMap::new(),
            lsm_policy: NodeLsmPolicy::new(false, 0),
            ebpf_bridge: None,
            peer_public_keys: BTreeMap::new(),
            rejected_byzantine_packets: 0,
        }
    }

    /// カーネル eBPF マップへの同期ブリッジを設定する
    pub fn with_ebpf_bridge(mut self, bridge: Arc<dyn EbpfBridge>) -> Self {
        self.ebpf_bridge = Some(bridge);
        self
    }

    /// 信頼済みノードの Ed25519 公開鍵群を登録する
    pub fn register_peer_public_keys(&mut self, keys: BTreeMap<NodeId, Vec<u8>>) {
        self.peer_public_keys = keys;
    }

    /// ビザンチン耐性検証付き RPC ハンドラ：
    /// 1. `signer_id` と RPC 内部の主張送信者 (`sender_id()`) が一致することを確認（なりすまし防止）
    /// 2. `peer_public_keys` に登録された公開鍵で Ed25519 署名を検証（改ざん・偽造防止）
    pub fn handle_signed_rpc(&mut self, signed: SignedConsensusRpc) -> Result<Option<ConsensusRpc>, &'static str> {
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
                return Err("Byzantine Fault Detected: unknown signer_id not in trusted mesh registry");
            }
        };

        // 3. Ed25519 署名の暗号学的検証
        let payload = bincode::serialize(&signed.rpc).map_err(|_| "Serialization error")?;
        let peer_pub_key = UnparsedPublicKey::new(&signature::ED25519, pub_key_bytes);
        if peer_pub_key.verify(&payload, &signed.signature).is_err() {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine Fault Detected: invalid Ed25519 signature or tampered RPC payload");
        }

        Ok(self.handle_rpc(signed.rpc))
    }

    /// クラスタの総ノード数（自身 + ピア）
    pub fn cluster_size(&self) -> usize {
        self.peers.len() + 1
    }

    /// 過半数（Quorum）に必要なノード数: $\lfloor N / 2 \rfloor + 1$
    pub fn quorum_size(&self) -> usize {
        (self.cluster_size() / 2) + 1
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
        self.current_term += 1;
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

    /// リーダーとして新しいコマンドを提案しログへ追加する
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
        self.log.push(entry.clone());

        self.advance_commit_index();
        Ok(entry)
    }

    /// 特定のフォロワーへ送信すべき `AppendEntries` RPC を構築する
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

        Some(ConsensusRpc::AppendEntries {
            term: self.current_term,
            leader_id: self.node_id.clone(),
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit: self.commit_index,
            commit_qc: None,
        })
    }

    /// マルチシグ合意証明書（Quorum Certificate: QC）および二枚舌（Equivocation）の厳密検証：
    /// 1. リーダーが同じ `(term, index)` で異なる `entry_hash` を送ってきた場合（二枚舌攻撃）、即座に検知して拒否する
    /// 2. `QuorumCertificate` に含まれる異なるノードからの Ed25519 署名が過半数（または $2f+1$）以上あり、
    ///    すべて同一の `(term, index, entry_hash)` に対する有効な署名であることを検証する
    pub fn verify_quorum_certificate(&mut self, qc: &QuorumCertificate) -> Result<(), &'static str> {
        // 1. 自身のログに該当 index が存在し、ハッシュが一致するか確認（二枚舌 Equivocation の検知）
        if let Some(local_entry) = self.log.get((qc.index.saturating_sub(1)) as usize) {
            if local_entry.term == qc.term && local_entry.digest() != qc.entry_hash {
                self.rejected_byzantine_packets += 1;
                return Err("Byzantine Equivocation Detected: Leader sent conflicting log entries for the same (term, index)!");
            }
        }

        // 2. 重複のない有効な署名数が Quorum 以上あるか検証
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
                return Err("Byzantine QC Forgery Detected: Unknown signer inside QuorumCertificate!");
            }
        }

        if valid_signers.len() < self.quorum_size() {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine Unilateral Commit Rejected: QuorumCertificate lacks required multi-signature quorum!");
        }

        Ok(())
    }

    /// 受信したコンセンサス RPC を処理し、必要に応じて応答 RPC を返す
    pub fn handle_rpc(&mut self, rpc: ConsensusRpc) -> Option<ConsensusRpc> {
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
                entry_hash: _,
            } => {
                self.on_append_entries_response(term, follower_id, success, match_index);
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
        if vote_granted {
            self.votes_received.insert(voter_id);
            if self.votes_received.len() >= self.quorum_size() {
                self.become_leader();
            }
        }
    }

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
        let last_hash = self.log.last().map(|e| e.digest()).unwrap_or([0u8; 32]);
        if term < self.current_term {
            return ConsensusRpc::AppendEntriesResponse {
                term: self.current_term,
                follower_id: self.node_id.clone(),
                success: false,
                match_index: self.last_log_index(),
                entry_hash: last_hash,
            };
        }

        if term > self.current_term || self.role == Role::Candidate {
            self.step_down(term);
        }
        self.leader_id = Some(leader_id);

        if prev_log_index > self.last_log_index() || self.term_at(prev_log_index) != prev_log_term {
            return ConsensusRpc::AppendEntriesResponse {
                term: self.current_term,
                follower_id: self.node_id.clone(),
                success: false,
                match_index: self.last_log_index(),
                entry_hash: last_hash,
            };
        }

        for entry in entries {
            let idx = entry.index;
            if idx <= self.last_log_index() {
                // 同一 Term・同一 Index なのに中身（ハッシュ）が違うエントリをリーダーが送ってきた場合は「二枚舌（Equivocation）」として拒否！
                let existing = &self.log[(idx - 1) as usize];
                if existing.term == entry.term && existing.digest() != entry.digest() {
                    self.rejected_byzantine_packets += 1;
                    return ConsensusRpc::AppendEntriesResponse {
                        term: self.current_term,
                        follower_id: self.node_id.clone(),
                        success: false,
                        match_index: self.last_log_index(),
                        entry_hash: existing.digest(),
                    };
                }
                if self.term_at(idx) != entry.term {
                    self.log.truncate((idx - 1) as usize);
                    self.log.push(entry);
                }
            } else {
                self.log.push(entry);
            }
        }

        // マルチシグ QC が添付されている場合は暗号検証を実行し、不正なら Commit を拒否する！
        if let Some(ref qc) = commit_qc {
            if self.verify_quorum_certificate(qc).is_err() {
                let cur_hash = self.log.last().map(|e| e.digest()).unwrap_or([0u8; 32]);
                return ConsensusRpc::AppendEntriesResponse {
                    term: self.current_term,
                    follower_id: self.node_id.clone(),
                    success: false,
                    match_index: self.last_log_index(),
                    entry_hash: cur_hash,
                };
            }
        }

        if leader_commit > self.commit_index {
            self.commit_index = leader_commit.min(self.last_log_index());
            self.apply_committed_entries();
        }

        let current_hash = self.log.last().map(|e| e.digest()).unwrap_or([0u8; 32]);
        ConsensusRpc::AppendEntriesResponse {
            term: self.current_term,
            follower_id: self.node_id.clone(),
            success: true,
            match_index: self.last_log_index(),
            entry_hash: current_hash,
        }
    }

    fn on_append_entries_response(
        &mut self,
        term: u64,
        follower_id: NodeId,
        success: bool,
        match_index: u64,
    ) {
        if term > self.current_term {
            self.step_down(term);
            return;
        }
        if self.role != Role::Leader || term != self.current_term {
            return;
        }

        if success {
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
            let replicated_count = 1
                + self
                    .match_index
                    .values()
                    .filter(|&&m| m >= n)
                    .count();

            if replicated_count >= self.quorum_size() {
                self.commit_index = n;
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
