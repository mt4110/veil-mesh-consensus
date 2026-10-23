use crate::adapter::NodeCryptoIdentity;
use crate::domain::{
    qc_vote_payload, ConsensusRpc, LogEntry, MeshCommand, NodeId, QuorumCertificate, Role,
    SecurityPolicyGuard, SignedConsensusRpc, MAX_SAFE_TERM, MAX_WIRE_MESSAGE_BYTES,
};
use crate::usecase::ports::EbpfBridge;
use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::Arc;
use veil_common::{
    evaluate_lsm_policy, evaluate_peer_policy, NodeLsmPolicy, PeerPolicy, XdpVerdict,
};

/// 【UseCase Layer】: 決定論的コンセンサス・ステートマシン（Ed25519 署名付き RPC + マルチシグ QC 必須化 + ドメイン安全方針ガード + eBPF ブリッジ同期）
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

    votes_received: BTreeSet<NodeId>,

    pub next_index: BTreeMap<NodeId, u64>,
    pub match_index: BTreeMap<NodeId, u64>,

    /// 各ログ Index ごとに収集された検証済み Ed25519 QC 署名 (`log_index -> (voter_id -> signature)`)
    pub qc_votes: BTreeMap<u64, BTreeMap<NodeId, Vec<u8>>>,

    /// コミット確定時に生成された各ログ Index のマルチシグ合意証明書 (`log_index -> QuorumCertificate`)
    pub commit_qcs: BTreeMap<u64, QuorumCertificate>,

    /// 確定（Commit & Apply）したコンセンサス結果から同期される eBPF `PEER_POLICY_MAP` のミラー状態
    pub ebpf_policy_map: BTreeMap<u32, PeerPolicy>,

    /// 自ノードの LSM ロックダウンポリシー状態 (`LSM_POLICY_MAP[0]`)
    pub lsm_policy: NodeLsmPolicy,

    /// カーネル内の実 Aya eBPF マップ（またはテスト用 MockEbpfBridge）へのリアルタイム書き込みポート
    pub ebpf_bridge: Option<Arc<dyn EbpfBridge>>,

    /// 自ノードの Ed25519 署名鍵アイデンティティ
    pub identity: Arc<NodeCryptoIdentity>,

    /// 信頼済みピア公開鍵レジストリ (`NodeId -> 32-byte Ed25519 PublicKey`)
    pub peer_public_keys: BTreeMap<NodeId, Vec<u8>>,

    /// ドメイン層の安全方針（保護 IP の誤遮断防止・境界値ガード）
    pub policy_guard: SecurityPolicyGuard,

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
    /// 新しい `ConsensusState` を構築する。
    /// デフォルトでは OS 乱数 (`SystemRandom`) による予測不可能な Ed25519 秘密鍵を生成する。
    pub fn new(node_id: impl Into<String>, peers: Vec<NodeId>) -> Self {
        let id_str = node_id.into();
        let identity = Arc::new(NodeCryptoIdentity::generate_random(&id_str));
        let mut peer_public_keys = BTreeMap::new();
        peer_public_keys.insert(id_str.clone(), identity.public_key_bytes.clone());

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
            policy_guard: SecurityPolicyGuard::default(),
            rejected_byzantine_packets: 0,
        }
    }

    pub fn with_ebpf_bridge(mut self, bridge: Arc<dyn EbpfBridge>) -> Self {
        self.ebpf_bridge = Some(bridge);
        self
    }

    pub fn set_crypto_identity(&mut self, identity: NodeCryptoIdentity) {
        self.peer_public_keys
            .insert(identity.node_id.clone(), identity.public_key_bytes.clone());
        self.identity = Arc::new(identity);
    }

    pub fn register_peer_public_keys(&mut self, keys: BTreeMap<NodeId, Vec<u8>>) {
        self.peer_public_keys = keys;
    }

    pub fn sign_rpc(&self, rpc: ConsensusRpc) -> SignedConsensusRpc {
        self.identity.sign_rpc(rpc)
    }

    /// 署名検証・境界値検証・ドメイン方針検証を行う唯一の RPC 入口
    pub fn handle_signed_rpc(
        &mut self,
        signed: SignedConsensusRpc,
    ) -> Result<Option<ConsensusRpc>, &'static str> {
        if signed.signer_id != signed.rpc.sender_id() {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine Fault Detected: signer_id does not match RPC sender_id (Identity Spoofing)");
        }

        let pub_key_bytes = match self.peer_public_keys.get(&signed.signer_id) {
            Some(pk) => pk,
            None => {
                self.rejected_byzantine_packets += 1;
                return Err(
                    "Byzantine Fault Detected: unknown signer_id not in trusted mesh registry",
                );
            }
        };

        let payload = bincode::serialize(&signed.rpc).map_err(|_| "Serialization error")?;
        if payload.len() > MAX_WIRE_MESSAGE_BYTES {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine Fault Detected: RPC payload exceeds MAX_WIRE_MESSAGE_BYTES");
        }
        if !NodeCryptoIdentity::verify_signature(pub_key_bytes, &payload, &signed.signature) {
            self.rejected_byzantine_packets += 1;
            return Err(
                "Byzantine Fault Detected: invalid Ed25519 signature or tampered RPC payload",
            );
        }

        let rpc_term = signed.rpc.term();
        if rpc_term == 0 || rpc_term >= MAX_SAFE_TERM {
            self.rejected_byzantine_packets += 1;
            return Err(
                "Byzantine Fault Detected: RPC term out of safe bounds (0 or >= MAX_SAFE_TERM)",
            );
        }

        Ok(self.process_verified_rpc(signed.rpc))
    }

    pub fn cluster_size(&self) -> usize {
        self.peers.len() + 1
    }

    /// 定足数（Quorum）に必要なノード数: $\lfloor 2N / 3 \rfloor + 1$
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

    pub fn start_election(&mut self) -> ConsensusRpc {
        if self.current_term < MAX_SAFE_TERM - 1 {
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

    /// リーダーとして新しいコマンドを提案する（保護 IP ガード検証を通過したもののみ）
    pub fn propose(&mut self, command: MeshCommand) -> Result<LogEntry, &'static str> {
        if self.role != Role::Leader {
            return Err("Only leader can propose commands");
        }
        self.policy_guard.validate_command(&command)?;

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

    pub fn verify_quorum_certificate(
        &mut self,
        qc: &QuorumCertificate,
    ) -> Result<(), &'static str> {
        if qc.index == 0 || qc.term == 0 || qc.term >= MAX_SAFE_TERM {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine QC Invalid: index or term out of valid bounds");
        }

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

        let payload = qc_vote_payload(qc.term, qc.index, &qc.entry_hash);
        let mut valid_signers = BTreeSet::new();

        for (signer_id, sig_bytes) in &qc.signatures {
            if let Some(pk_bytes) = self.peer_public_keys.get(signer_id) {
                if NodeCryptoIdentity::verify_signature(pk_bytes, &payload, sig_bytes) {
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

        if self
            .policy_guard
            .validate_entries(prev_log_index, term, &entries)
            .is_err()
        {
            self.rejected_byzantine_packets += 1;
            return self.make_append_response(false);
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
                let existing = &self.log[(idx - 1) as usize];
                if existing.term == entry.term && existing.digest() != entry.digest() {
                    self.rejected_byzantine_packets += 1;
                    return self.make_append_response(false);
                }
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
                if !NodeCryptoIdentity::verify_signature(pk_bytes, &payload, &qc_vote_signature) {
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

    pub fn check_xdp_packet(&self, src_ip: Ipv4Addr) -> XdpVerdict {
        let ip_u32 = u32::from(src_ip);
        let policy = self.ebpf_policy_map.get(&ip_u32);
        evaluate_peer_policy(policy)
    }

    pub fn check_lsm_socket_connect(&self) -> i32 {
        evaluate_lsm_policy(Some(&self.lsm_policy))
    }
}
