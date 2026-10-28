#[path = "authorization.rs"]
mod authorization;
#[path = "election.rs"]
mod election;
#[path = "progress.rs"]
mod progress;
#[path = "stable_checkpoint.rs"]
mod stable_checkpoint;
#[path = "two_phase.rs"]
mod two_phase;
use crate::adapter::NodeCryptoIdentity;
use crate::domain::{
    qc_vote_payload, rpc_signing_payload, timeout_vote_payload, ConsensusRpc,
    DeadlinePeerTrustRule, FixedConfiguration, LogEntry, MeshCommand, NodeId, PeerTrustRule,
    QuorumCertificate, Role, SecurityPolicyGuard, SignedConsensusRpc, SigningContext,
    TimeoutCertificate, MAX_LOG_ENTRIES_PER_RPC, MAX_SAFE_TERM, MAX_WIRE_MESSAGE_BYTES,
};
use crate::usecase::ports::{ConsensusPersistence, EbpfBridge, LeaseTime, PolicyClock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::Arc;
use veil_common::{
    evaluate_lsm_policy, evaluate_peer_policy_at, NodeLsmPolicy, PeerPolicy, XdpVerdict,
};

const WAL_MAGIC_VERSION: u32 = 6;
const SIMULATION_WALL_EPOCH_NS: u64 = 1_700_000_000_000_000_000;

/// `voter_id -> (high_qc_term, high_qc_index, Ed25519 Signature)`
pub type TermTimeoutVoteMap = BTreeMap<NodeId, (u64, u64, Vec<u8>)>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalPayload {
    version: u32,
    context: SigningContext,
    node_id: NodeId,
    current_term: u64,
    voted_for: Option<NodeId>,
    log: Vec<LogEntry>,
    commit_index: u64,
    commit_qcs: BTreeMap<u64, QuorumCertificate>,
    voted_entry_hashes: Vec<(u64, u64, [u8; 32])>,
    boot_id: String,
    // First deadlines, including expired tombstones; indexed by committed log position.
    lease_deadlines: BTreeMap<(u64, usize), u64>,
    emergency_local_bypass: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalEnvelope {
    payload_bytes: Vec<u8>,
    sha256_checksum: [u8; 32],
}

/// 【UseCase Layer】: 決定論的コンセンサス・ステートマシン（Ed25519 署名付き RPC + 累積ハッシュチェーン + ロック付きマルチシグ QC + View-Change TC + TTL 自動失効 + 緊急バイパス + eBPF ブリッジ同期）
pub struct ConsensusState {
    signing_context: SigningContext,
    fixed_configuration: Option<FixedConfiguration>,
    pub node_id: NodeId,
    peers: Vec<NodeId>,
    pub current_term: u64,
    pub voted_for: Option<NodeId>,
    pub log: Vec<LogEntry>, // 1-indexed 相当（内部 Vec の index 0 が log index 1）
    /// 各ログ Index の O(1) 累積チェーンハッシュキャッシュ（`log` と完全に同期）
    pub chain_hashes: Vec<[u8; 32]>,
    pub commit_index: u64,
    pub last_applied: u64,
    pub role: Role,
    pub leader_id: Option<NodeId>,

    votes_received: BTreeSet<NodeId>,
    /// Volatile timer signal, incremented only after durable acceptance of leader contact/a vote.
    election_activity: u64,
    election: election::ElectionState,
    progress: progress::ProgressState,
    two_phase: Option<two_phase::State>,
    #[cfg(test)]
    pub(crate) censor_ban_requests: bool,

    pub next_index: BTreeMap<NodeId, u64>,
    pub match_index: BTreeMap<NodeId, u64>,

    /// 各ログ Index ごとに収集された検証済み Ed25519 QC 署名 (`log_index -> (voter_id -> signature)`)
    pub qc_votes: BTreeMap<u64, BTreeMap<NodeId, Vec<u8>>>,

    /// コミット確定時に生成された各ログ Index のマルチシグ合意証明書 (`log_index -> QuorumCertificate`)
    pub commit_qcs: BTreeMap<u64, QuorumCertificate>,

    /// 同一 `(term, index)` に対する二重署名（Equivocation / Term Sandwich 攻撃）を防ぐ投票履歴レジストリ
    pub voted_entry_hashes: BTreeMap<(u64, u64), [u8; 32]>,

    /// 各 Term ごとに収集された View-Change `TimeoutVote` (`term -> TermTimeoutVoteMap`) (v0.4.0)
    pub timeout_votes: BTreeMap<u64, TermTimeoutVoteMap>,

    /// タイムアウト投票に添付された検証済み最高位 QC のキャッシュ (`term -> QuorumCertificate`) (v0.4.0)
    pub highest_observed_qc: Option<QuorumCertificate>,

    /// 直近で成立した View-Change `TimeoutCertificate` (`term -> TimeoutCertificate`) (v0.4.0)
    pub timeout_certificates: BTreeMap<u64, TimeoutCertificate>,

    /// 確定（Commit & Apply）したコンセンサス結果から同期される eBPF `PEER_POLICY_MAP` のミラー状態
    pub ebpf_policy_map: BTreeMap<u32, PeerPolicy>,

    /// 各遮断・信頼ルールの単調増加クロック上の失効時刻ナノ秒 (`peer_ip -> deadline_ns`) (v0.5.0)
    pub peer_rule_deadlines_ns: BTreeMap<u32, u64>,

    /// ノード内部の単調増加クロック（ナノ秒）。TTL 判定にのみ使用し NTP ステップの影響を受けない (v0.5.0)
    pub monotonic_now_ns: u64,

    /// 緊急ローカルバイパスフラグ（`true` の間はコンセンサス状態を保持したままローカル XDP 判定を `Pass` に退避する：v0.5.0）
    pub emergency_local_bypass: bool,

    /// 自ノードの LSM ロックダウンポリシー状態 (`LSM_POLICY_MAP[0]`)
    pub lsm_policy: NodeLsmPolicy,

    /// カーネル内の実 Aya eBPF マップ（またはテスト用 MockEbpfBridge）へのリアルタイム書き込みポート
    pub ebpf_bridge: Option<Arc<dyn EbpfBridge>>,

    /// 直近のカーネル eBPF 適用エラー（失敗を黙殺せず `Degraded` 状態として追跡する：R4 対策）
    pub last_apply_error: Option<String>,

    persistence: Option<Arc<dyn ConsensusPersistence>>,
    /// Latched: do not resume voting after an uncertain disk write.
    pub persistence_error: Option<String>,
    persisted_snapshot_hash: Option<[u8; 32]>,
    pub max_wal_bytes: u64,
    pub persisted_wal_bytes: u64,
    // Resume a partially applied batch without repeating successful rules.
    apply_rule_cursor: usize,
    policy_clock: Option<Arc<dyn PolicyClock>>,
    pub lease_clock_error: Option<String>,
    boot_id: String,
    lease_deadlines: BTreeMap<(u64, usize), u64>,
    recovery_latest_rules: Option<BTreeSet<(u64, usize)>>,
    recovery_through: u64,
    bypass_sync_pending: bool,

    /// 自ノードの Ed25519 署名鍵アイデンティティ
    identity: Arc<NodeCryptoIdentity>,

    /// 信頼済みピア公開鍵レジストリ (`NodeId -> 32-byte Ed25519 PublicKey`)
    peer_public_keys: BTreeMap<NodeId, Vec<u8>>,

    /// ドメイン層の安全方針（保護 IP の誤遮断防止・境界値ガード）
    policy_guard: SecurityPolicyGuard,

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
            .field("last_apply_error", &self.last_apply_error)
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
            signing_context: SigningContext::simulation(),
            fixed_configuration: None,
            node_id: id_str,
            peers,
            current_term: 0,
            voted_for: None,
            log: Vec::new(),
            chain_hashes: Vec::new(),
            commit_index: 0,
            last_applied: 0,
            role: Role::Follower,
            leader_id: None,
            votes_received: BTreeSet::new(),
            election_activity: 0,
            election: election::ElectionState::default(),
            progress: progress::ProgressState::default(),
            two_phase: None,
            #[cfg(test)]
            censor_ban_requests: false,
            next_index: BTreeMap::new(),
            match_index: BTreeMap::new(),
            qc_votes: BTreeMap::new(),
            commit_qcs: BTreeMap::new(),
            voted_entry_hashes: BTreeMap::new(),
            timeout_votes: BTreeMap::new(),
            highest_observed_qc: None,
            timeout_certificates: BTreeMap::new(),
            ebpf_policy_map: BTreeMap::new(),
            peer_rule_deadlines_ns: BTreeMap::new(),
            monotonic_now_ns: 0,
            emergency_local_bypass: false,
            lsm_policy: NodeLsmPolicy::new(false, 0),
            ebpf_bridge: None,
            last_apply_error: None,
            persistence: None,
            persistence_error: None,
            persisted_snapshot_hash: None,
            max_wal_bytes: crate::domain::DEFAULT_MAX_WAL_BYTES,
            persisted_wal_bytes: 0,
            apply_rule_cursor: 0,
            policy_clock: None,
            lease_clock_error: None,
            boot_id: "simulation".into(),
            lease_deadlines: BTreeMap::new(),
            recovery_latest_rules: None,
            recovery_through: 0,
            bypass_sync_pending: false,
            identity,
            peer_public_keys,
            policy_guard: SecurityPolicyGuard::default(),
            rejected_byzantine_packets: 0,
        }
    }

    pub fn signing_context(&self) -> &SigningContext {
        &self.signing_context
    }

    /// Static configuration only: never reinterpret existing votes/history under a new epoch.
    pub fn configure_signing_context(
        &mut self,
        context: SigningContext,
    ) -> Result<(), &'static str> {
        if context.config_digest != [0; 32] {
            return Err(
                "A configuration digest must be installed through validated fixed membership",
            );
        }
        self.install_signing_context(context)
    }

    fn install_signing_context(&mut self, context: SigningContext) -> Result<(), &'static str> {
        context.validate()?;
        if self.fixed_configuration.is_some()
            || self.current_term != 0
            || !self.log.is_empty()
            || self.persistence.is_some()
            || self.voted_for.is_some()
            || !self.voted_entry_hashes.is_empty()
            || !self.timeout_votes.is_empty()
            || !self.commit_qcs.is_empty()
        {
            return Err("Signature context must be configured before consensus or WAL recovery");
        }
        self.signing_context = context;
        Ok(())
    }

    pub fn with_ebpf_bridge(mut self, bridge: Arc<dyn EbpfBridge>) -> Self {
        self.ebpf_bridge = Some(bridge);
        self
    }

    /// Configure before any log/recovery. The default clock is simulation-only.
    pub fn configure_policy_clock(&mut self, clock: Arc<dyn PolicyClock>) -> std::io::Result<()> {
        if !self.log.is_empty() || self.persistence.is_some() {
            return Err(std::io::Error::other(
                "Configure clock before loading state",
            ));
        }
        self.monotonic_now_ns = clock.now_ns()?;
        self.boot_id = clock.boot_id().to_owned();
        self.policy_clock = Some(clock);
        Ok(())
    }

    fn refresh_policy_clock(&mut self) -> Result<(), String> {
        if let Some(clock) = &self.policy_clock {
            let now = clock.now_ns().map_err(|e| e.to_string())?;
            if now < self.monotonic_now_ns {
                return Err("Boot clock moved backwards".into());
            }
            self.monotonic_now_ns = now;
        }
        Ok(())
    }

    pub(crate) fn lease_time(&mut self) -> std::io::Result<LeaseTime> {
        let result = match &self.policy_clock {
            Some(clock) => clock.lease_time(),
            None => SIMULATION_WALL_EPOCH_NS
                .checked_add(self.monotonic_now_ns)
                .map(|unix_ns| LeaseTime {
                    boot_ns: self.monotonic_now_ns,
                    unix_ns,
                    uncertainty_ns: 0,
                    max_slow_rate_ppm: 0,
                })
                .ok_or_else(|| std::io::Error::other("Simulation clock overflow")),
        };
        match &result {
            Err(error) if self.lease_clock_error.is_none() => {
                tracing::warn!(node = %self.node_id, %error, "Lease clock quality unavailable; new approvals blocked");
            }
            Ok(_) if self.lease_clock_error.is_some() => {
                tracing::info!(node = %self.node_id, "Lease clock quality recovered");
            }
            _ => {}
        }
        self.lease_clock_error = result.as_ref().err().map(ToString::to_string);
        result
    }

    fn stamp_deadlines(&mut self, command: MeshCommand) -> Result<MeshCommand, &'static str> {
        let MeshCommand::SetPeerTrustBatch { rules } = &command else {
            return Ok(command);
        };
        if !rules.iter().any(|r| r.ttl_secs > 0) {
            return Ok(command);
        }
        let sample = self
            .lease_time()
            .map_err(|_| "Clock uncertainty is unavailable; refusing a new TTL proposal")?;
        let issued_at_unix_ns = sample.unix_ns;
        let rules = rules
            .iter()
            .map(|rule| {
                let not_after_unix_ns = if rule.ttl_secs == 0 {
                    0
                } else {
                    issued_at_unix_ns
                        .checked_add(u64::from(rule.ttl_secs) * 1_000_000_000)
                        .ok_or("Absolute deadline overflow")?
                };
                Ok(DeadlinePeerTrustRule {
                    rule: rule.clone(),
                    not_after_unix_ns,
                })
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        Ok(MeshCommand::SetPeerTrustLeaseBatch {
            issued_at_unix_ns,
            rules,
        })
    }

    pub fn identity(&self) -> &Arc<NodeCryptoIdentity> {
        &self.identity
    }
    pub fn peer_public_keys(&self) -> &BTreeMap<NodeId, Vec<u8>> {
        &self.peer_public_keys
    }

    pub fn set_crypto_identity(
        &mut self,
        identity: impl Into<Arc<NodeCryptoIdentity>>,
    ) -> Result<(), &'static str> {
        if self.fixed_configuration.is_some() {
            return Err("Fixed membership cannot change identity");
        }
        let identity = identity.into();
        self.peer_public_keys
            .insert(identity.node_id.clone(), identity.public_key_bytes.clone());
        self.identity = identity;
        Ok(())
    }

    pub fn register_peer_public_keys(
        &mut self,
        keys: BTreeMap<NodeId, Vec<u8>>,
    ) -> Result<(), &'static str> {
        if self.fixed_configuration.is_some() {
            return Err("Fixed membership cannot reload keys");
        }
        self.peer_public_keys = keys;
        Ok(())
    }

    /// Install a complete validated voter set atomically, before any votes or WAL.
    pub fn configure_fixed_membership(
        &mut self,
        config: FixedConfiguration,
        identity: impl Into<Arc<NodeCryptoIdentity>>,
    ) -> Result<(), &'static str> {
        let identity = identity.into();
        if identity.node_id != self.node_id {
            return Err("Local identity node ID mismatch");
        }
        config.validate_local(&self.node_id, &identity.public_key_bytes)?;
        self.install_signing_context(config.context().clone())?;
        self.peers = config.peer_addrs(&self.node_id).into_keys().collect();
        self.peer_public_keys = config.public_keys();
        self.policy_guard = config.policy_guard();
        self.identity = identity;
        self.fixed_configuration = Some(config);
        Ok(())
    }

    /// Install only after loading the existing WAL under its exclusive lock.
    pub fn enable_persistence(
        &mut self,
        store: Arc<dyn ConsensusPersistence>,
    ) -> Result<(), &'static str> {
        if self.persistence.is_some() {
            return Err("Persistence is already configured");
        }
        self.persistence = Some(store);
        self.checkpoint()
    }

    pub fn checkpoint(&mut self) -> Result<(), &'static str> {
        if self.persistence_error.is_some() {
            return Err("Persistence failed; restart and recover before voting");
        }
        if let Some(store) = self.persistence.clone() {
            let serialized_at = std::time::Instant::now();
            let serialized = self.wal_bytes();
            crate::infra::latency::record(
                "serialize",
                serialized_at.elapsed(),
                serialized.as_ref().map_or(0, Vec::len),
                serialized.is_ok(),
            );
            let result = serialized.and_then(|bytes| {
                let hash_started = std::time::Instant::now();
                let hash: [u8; 32] = ring::digest::digest(&ring::digest::SHA256, &bytes)
                    .as_ref()
                    .try_into()
                    .expect("SHA-256 length");
                crate::infra::latency::record(
                    "snapshot-hash",
                    hash_started.elapsed(),
                    bytes.len(),
                    true,
                );
                if self.persisted_snapshot_hash != Some(hash) {
                    store.persist(&bytes)?;
                    self.persisted_wal_bytes = bytes.len() as u64;
                    self.persisted_snapshot_hash = Some(hash);
                } else {
                    crate::infra::latency::record(
                        "unchanged-snapshot",
                        std::time::Duration::ZERO,
                        bytes.len(),
                        true,
                    );
                }
                Ok(())
            });
            if let Err(err) = result {
                self.persistence_error = Some(err.to_string());
                self.role = Role::Follower;
                return Err("Persistence failed; restart and recover before voting");
            }
        }
        Ok(())
    }

    /// Runtime send paths checkpoint before releasing any signed message.
    pub fn sign_rpc_durable(
        &mut self,
        rpc: ConsensusRpc,
    ) -> Result<SignedConsensusRpc, &'static str> {
        self.checkpoint()?;
        Ok(self.sign_rpc(rpc))
    }

    /// Raw signing primitive for deterministic simulations. Runtime send paths
    /// use sign_rpc_durable so a persistence failure cannot leak a vote.
    pub fn sign_rpc(&self, rpc: ConsensusRpc) -> SignedConsensusRpc {
        let rpc = self.attach_term_certificate(rpc);
        self.identity.sign_rpc(&self.signing_context, rpc)
    }

    /// 署名検証・境界値検証・ドメイン方針検証を行う唯一の RPC 入口
    pub fn handle_signed_rpc(
        &mut self,
        signed: SignedConsensusRpc,
    ) -> Result<Option<ConsensusRpc>, &'static str> {
        if self.persistence_error.is_some() {
            return Err("Persistence failed; restart and recover before voting");
        }
        if signed.context != self.signing_context {
            self.rejected_byzantine_packets += 1;
            return Err(
                "Signature context mismatch: protocol, cluster ID, config epoch or config digest",
            );
        }
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

        let payload = rpc_signing_payload(&self.signing_context, &signed.signer_id, &signed.rpc);
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

        // Fault injection exists only in test binaries: keep serving fresh ACKs
        // and the old log while ignoring both witnessed requests and handoff.
        #[cfg(test)]
        if self.censor_ban_requests
            && self.role == Role::Leader
            && (matches!(signed.rpc, ConsensusRpc::ProgressGossip { .. })
                || signed.rpc.term() > self.current_term)
        {
            return Ok(None);
        }
        let rpc = self.admit_rpc_term(signed.rpc)?;
        if self.uses_two_phase()
            && matches!(
                rpc,
                ConsensusRpc::RequestVote { .. }
                    | ConsensusRpc::RequestVoteResponse { .. }
                    | ConsensusRpc::AppendEntries { .. }
                    | ConsensusRpc::AppendEntriesResponse { .. }
                    | ConsensusRpc::TimeoutVote { .. }
            )
        {
            return Err("Legacy consensus message forbidden in two-phase mode");
        }
        let old_vote = (self.current_term, self.voted_for.clone());
        let response = if let ConsensusRpc::ProgressGossip {
            request,
            vote,
            certificate,
            ..
        } = rpc
        {
            self.receive_progress(request, vote, certificate)?;
            None
        } else if let ConsensusRpc::TwoPhase {
            term,
            sender_id,
            message,
        } = rpc
        {
            self.handle_phase_rpc(term, sender_id, *message)?
        } else {
            self.process_verified_rpc(rpc)
        };
        self.checkpoint()?;
        // A duplicate vote/heartbeat never renews the election deadline.
        if matches!(
            response,
            Some(ConsensusRpc::RequestVoteResponse {
                vote_granted: true,
                ..
            })
        ) && old_vote != (self.current_term, self.voted_for.clone())
        {
            self.election_activity = self.election_activity.wrapping_add(1);
        }
        Ok(response)
    }

    pub fn election_activity(&self) -> u64 {
        self.election_activity
    }

    pub fn cluster_size(&self) -> usize {
        self.peers.len() + 1
    }

    /// 定足数（Quorum）に必要なノード数: $\lfloor 2N / 3 \rfloor + 1$
    pub fn quorum_size(&self) -> usize {
        (2 * self.cluster_size()) / 3 + 1
    }

    pub fn last_log_index(&self) -> u64 {
        self.log_base() + self.log.len() as u64
    }

    pub fn last_log_term(&self) -> u64 {
        self.log.last().map(|e| e.term).unwrap_or_else(|| {
            self.two_phase
                .as_ref()
                .and_then(|s| s.durable.stable.as_ref())
                .map_or(0, |s| s.commit.block.entry.term)
        })
    }

    pub fn term_at(&self, index: u64) -> u64 {
        if index == 0 {
            return 0;
        }
        self.entry_at(index).map(|e| e.term).unwrap_or(0)
    }

    /// 指定された `index` (1-indexed) までの全プレフィックス `[1..=index]` を暗号学的に束縛した累積チェーンハッシュを O(1) で取得する（R1 対策）
    pub fn entry_hash_at(&self, index: u64) -> Option<[u8; 32]> {
        if index == 0 {
            return None;
        }
        if index == self.log_base() {
            return Some(self.base_hash());
        }
        index
            .checked_sub(self.log_base() + 1)
            .and_then(|i| self.chain_hashes.get(i as usize))
            .copied()
    }

    fn push_log_entry(&mut self, entry: LogEntry) -> [u8; 32] {
        let parent = self
            .chain_hashes
            .last()
            .copied()
            .unwrap_or_else(|| self.base_hash());
        let h = entry.digest_with_parent(&parent);
        self.log.push(entry);
        self.chain_hashes.push(h);
        h
    }

    fn truncate_log(&mut self, len: usize) {
        let len = len.saturating_sub(self.log_base() as usize);
        self.log.truncate(len);
        self.chain_hashes.truncate(len);
    }

    pub fn start_election(&mut self) -> ConsensusRpc {
        // R10 対策: Term 上限到達時に同一 Term 内で voted_for を上書きして二重投票しないよう保護する
        if self.can_start_election() {
            self.current_term += 1;
            self.election.certificate = self.election.prepared_certificate.take();
            self.role = Role::Candidate;
            self.voted_for = Some(self.node_id.clone());
            self.leader_id = None;
            self.votes_received.clear();
            self.votes_received.insert(self.node_id.clone());

            if self.uses_two_phase() {
                return self
                    .start_phase_election()
                    .unwrap_or_else(|_| ConsensusRpc::TwoPhase {
                        term: self.current_term,
                        sender_id: self.node_id.clone(),
                        message: Box::new(crate::domain::PhaseMessage::ViewRequest),
                    });
            }
            if self.votes_received.len() >= self.quorum_size() {
                self.become_leader();
            }
        }

        ConsensusRpc::RequestVote {
            term: self.current_term,
            candidate_id: self.node_id.clone(),
            last_log_index: self.last_log_index(),
            last_log_term: self.last_log_term(),
        }
    }

    pub fn start_signed_election(&mut self) -> Result<SignedConsensusRpc, &'static str> {
        self.checkpoint()?;
        if self.current_term >= MAX_SAFE_TERM - 1 {
            return Err("Election term limit reached; refusing an invalid term");
        }
        if !self.can_start_election() {
            return Err("Election requires quorum term support; run pre-vote first");
        }
        let rv = self.start_election();
        self.sign_rpc_durable(rv)
    }

    fn become_leader(&mut self) {
        if self.leader_excluded(&self.node_id) {
            return;
        }
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
        // A same-term leader contact must not erase the durable vote and allow
        // a second candidate to obtain another vote in that term.
        if new_term > self.current_term {
            self.voted_for = None;
        }
        self.current_term = new_term;
        self.election.probe = None;
        self.election.prepared_certificate = None;
        self.election.last_fresh_contact = None;
        self.role = Role::Follower;
        self.leader_id = None;
        self.votes_received.clear();
    }

    /// リーダーとして新しいコマンドを提案する（保護 IP ガード検証を通過したもののみ）
    pub fn propose(&mut self, command: MeshCommand) -> Result<LogEntry, &'static str> {
        if self.persistence_error.is_some() {
            return Err("Persistence failed; restart and recover before voting");
        }
        if self.role != Role::Leader || self.leader_excluded(&self.node_id) {
            return Err("Only eligible leader can propose commands");
        }
        self.policy_guard.validate_command(&command)?;
        let command = self.stamp_deadlines(command)?;
        self.verify_operator_command(&command)?;
        self.check_operator_vote_time(&command)?;
        if let Some((issued_at_unix_ns, rules)) = command.lease_parts() {
            let sample = self
                .lease_time()
                .map_err(|_| "Clock uncertainty is unavailable; refusing a new TTL proposal")?;
            for item in rules.iter().filter(|r| r.rule.ttl_secs > 0) {
                if sample
                    .boot_deadline(issued_at_unix_ns, item.not_after_unix_ns)
                    .map_err(|_| "Invalid deadline conversion")?
                    == 1
                {
                    return Err(
                        "Proposal is expired, too short for clock uncertainty, or future-dated",
                    );
                }
            }
        }

        let index = self.last_log_index() + 1;
        let entry = LogEntry {
            term: self.current_term,
            index,
            command,
        };
        if self.uses_two_phase() {
            return self.propose_phase(entry);
        }
        let probe = ConsensusRpc::AppendEntries {
            term: self.current_term,
            leader_id: self.node_id.clone(),
            prev_log_index: index - 1,
            prev_log_term: self.last_log_term(),
            prev_log_hash: self.entry_hash_at(index - 1).unwrap_or([0; 32]),
            entries: vec![entry.clone()],
            leader_commit: 0,
            commit_qc: None,
        };
        if !self.rpc_fits_wire(&probe) {
            return Err("Single proposal exceeds wire message limit");
        }
        let digest =
            entry.digest_with_parent(&self.chain_hashes.last().copied().unwrap_or([0; 32]));
        if self
            .voted_entry_hashes
            .get(&(entry.term, index))
            .is_some_and(|old| *old != digest)
        {
            return Err("Proposal conflicts with a durable vote; a later election is required");
        }
        self.push_log_entry(entry.clone());
        self.voted_entry_hashes.insert((entry.term, index), digest);
        self.checkpoint()?;
        let my_qc_sig =
            self.identity
                .sign_qc_vote(&self.signing_context, entry.term, index, &digest);
        self.qc_votes
            .entry(index)
            .or_default()
            .insert(self.node_id.clone(), my_qc_sig);

        self.advance_commit_index();
        self.checkpoint()?;
        Ok(entry)
    }

    pub fn make_append_entries(&self, peer_id: &str) -> Option<ConsensusRpc> {
        if self.uses_two_phase() {
            return self.phase_append(peer_id);
        }
        if self.role != Role::Leader {
            return None;
        }
        let next_idx = *self.next_index.get(peer_id).unwrap_or(&1);
        let prev_log_index = next_idx.saturating_sub(1);
        let prev_log_term = self.term_at(prev_log_index);

        let start = usize::try_from(next_idx.checked_sub(1)?).ok()?;
        if start > self.log.len() {
            return None;
        }
        let max_count = (self.log.len() - start).min(MAX_LOG_ENTRIES_PER_RPC);
        // Commit only a prefix for which an actual QC exists; sparse QCs must
        // not make earlier catch-up chunks claim an unprovable commit.
        let mut entry_bytes = vec![0u64];
        for entry in &self.log[start..start + max_count] {
            let size = bincode::serialized_size(entry).ok()?;
            entry_bytes.push(entry_bytes.last()?.checked_add(size)?);
        }
        let build = |count: usize| {
            let last = prev_log_index + count as u64;
            let qc = self
                .commit_qcs
                .range(..=self.commit_index.min(last))
                .next_back()
                .map(|(_, qc)| qc.clone());
            ConsensusRpc::AppendEntries {
                term: self.current_term,
                leader_id: self.node_id.clone(),
                prev_log_index,
                prev_log_term,
                prev_log_hash: self.entry_hash_at(prev_log_index).unwrap_or([0; 32]),
                entries: Vec::new(),
                leader_commit: qc.as_ref().map_or(0, |qc| qc.index),
                commit_qc: qc,
            }
        };
        // Search downward: QC sizes can vary with the chosen prefix, so size
        // is not assumed to be monotonic across different certificates.
        for count in (0..=max_count).rev() {
            let mut rpc = build(count);
            if self.rpc_wire_size(&rpc)?.checked_add(entry_bytes[count])?
                > MAX_WIRE_MESSAGE_BYTES as u64
                && count > 0
            {
                // An entry and its QC may fit separately but not together.
                // Send the entries first; deliver the commit QC on a later
                // heartbeat once the peer has acknowledged this prefix.
                if let ConsensusRpc::AppendEntries {
                    leader_commit,
                    commit_qc,
                    ..
                } = &mut rpc
                {
                    *leader_commit = 0;
                    *commit_qc = None;
                }
            }
            if self.rpc_wire_size(&rpc)?.checked_add(entry_bytes[count])?
                <= MAX_WIRE_MESSAGE_BYTES as u64
            {
                if let ConsensusRpc::AppendEntries { entries, .. } = &mut rpc {
                    *entries = self.log[start..start + count].to_vec();
                }
                return Some(rpc);
            }
        }
        None
    }

    fn rpc_fits_wire(&self, rpc: &ConsensusRpc) -> bool {
        self.rpc_wire_size(rpc)
            .is_some_and(|size| size <= MAX_WIRE_MESSAGE_BYTES as u64)
    }

    fn rpc_wire_size(&self, rpc: &ConsensusRpc) -> Option<u64> {
        let envelope = crate::adapter::WireMessage::Consensus(Box::new(SignedConsensusRpc {
            context: self.signing_context.clone(),
            signer_id: self.node_id.clone(),
            rpc: self.attach_term_certificate(rpc.clone()),
            signature: vec![0; 64],
        }));
        bincode::serialized_size(&envelope).ok()
    }

    /// QC 自体に含まれる Ed25519 マルチシグ定足数と境界値のみを検証する補助関数
    fn verify_qc_signatures_only(&self, qc: &QuorumCertificate) -> Result<(), &'static str> {
        if qc.index == 0 || qc.term == 0 || qc.term >= MAX_SAFE_TERM {
            return Err("Byzantine QC Invalid: index or term out of valid bounds");
        }
        let payload = qc_vote_payload(&self.signing_context, qc.term, qc.index, &qc.entry_hash);
        let mut valid_signers = BTreeSet::new();
        for (signer_id, sig_bytes) in &qc.signatures {
            if let Some(pk_bytes) = self.peer_public_keys.get(signer_id) {
                if NodeCryptoIdentity::verify_signature(pk_bytes, &payload, sig_bytes) {
                    valid_signers.insert(signer_id.clone());
                } else {
                    return Err("Byzantine QC Forgery Detected: Invalid Ed25519 signature inside QuorumCertificate!");
                }
            } else {
                return Err(
                    "Byzantine QC Forgery Detected: Unknown signer inside QuorumCertificate!",
                );
            }
        }
        if valid_signers.len() < self.quorum_size() {
            return Err("Byzantine Unilateral Commit Rejected: QuorumCertificate lacks required multi-signature quorum!");
        }
        Ok(())
    }

    pub fn verify_quorum_certificate(
        &mut self,
        qc: &QuorumCertificate,
    ) -> Result<(), &'static str> {
        if qc.index == 0 || qc.term == 0 || qc.term >= MAX_SAFE_TERM {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine QC Invalid: index or term out of valid bounds");
        }

        let local_chain_hash = match self.entry_hash_at(qc.index) {
            Some(h) => h,
            None => {
                self.rejected_byzantine_packets += 1;
                return Err(
                    "Byzantine QC Invalid: QC refers to a log index not present on follower",
                );
            }
        };

        let local_entry = &self.log[(qc.index - 1) as usize];
        if local_entry.term != qc.term || local_chain_hash != qc.entry_hash {
            self.rejected_byzantine_packets += 1;
            return Err("Byzantine Equivocation Detected: Leader sent conflicting log entries or mismatched QC chain hash!");
        }

        if let Err(err) = self.verify_qc_signatures_only(qc) {
            self.rejected_byzantine_packets += 1;
            return Err(err);
        }

        Ok(())
    }

    fn process_verified_rpc(&mut self, rpc: ConsensusRpc) -> Option<ConsensusRpc> {
        match rpc {
            ConsensusRpc::PreVote {
                term,
                candidate_id,
                last_log_index,
                last_log_term,
            } => self.on_pre_vote(term, &candidate_id, last_log_index, last_log_term),
            ConsensusRpc::PreVoteResponse {
                term,
                voter_id,
                support_signature,
            } => {
                self.on_pre_vote_response(term, voter_id, support_signature);
                None
            }
            ConsensusRpc::LivenessProbe { term, nonce, .. } => (self.role == Role::Leader
                && term == self.current_term)
                .then(|| ConsensusRpc::LivenessAck {
                    term,
                    sender_id: self.node_id.clone(),
                    nonce,
                }),
            ConsensusRpc::LivenessAck {
                term,
                sender_id,
                nonce,
            } => {
                self.on_liveness_ack(term, sender_id, nonce);
                None
            }
            ConsensusRpc::TwoPhase { .. }
            | ConsensusRpc::ProgressGossip { .. }
            | ConsensusRpc::TermCertified { .. } => {
                unreachable!("admission removes the wrapper")
            }
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
                prev_log_hash,
                entries,
                leader_commit,
                commit_qc,
            } => Some(self.on_append_entries(
                term,
                leader_id,
                prev_log_index,
                prev_log_term,
                prev_log_hash,
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

            ConsensusRpc::TimeoutVote {
                term,
                voter_id,
                highest_qc,
                tc_vote_signature,
            } => {
                self.on_timeout_vote(term, voter_id, highest_qc, tc_vote_signature);
                None
            }
        }
    }

    /// 指定された Term における決定論的ラウンドロビン・リーダー `NodeId` を算出する (v0.4.0 View-Change)
    pub fn deterministic_leader_for_term(&self, term: u64) -> NodeId {
        let mut all_nodes = self.peers.clone();
        all_nodes.push(self.node_id.clone());
        all_nodes.sort();
        all_nodes.dedup();
        let idx = ((term.saturating_sub(1)) as usize) % all_nodes.len();
        all_nodes[idx].clone()
    }

    /// リーダー沈黙・検閲検知時に現在の Term に対する `TimeoutVote` を生成・署名し、自ノードにも登録する (v0.4.0)
    pub fn trigger_view_timeout(&mut self) -> Result<SignedConsensusRpc, &'static str> {
        self.checkpoint()?;
        if self.current_term == 0 {
            self.current_term = 1;
        }
        let target_term = self.current_term;
        let highest_qc = self
            .commit_qcs
            .values()
            .max_by_key(|qc| (qc.term, qc.index))
            .cloned();
        let (high_qc_term, high_qc_index) = highest_qc
            .as_ref()
            .map(|qc| (qc.term, qc.index))
            .unwrap_or((0, 0));
        let sig = self.identity.sign_timeout_vote(
            &self.signing_context,
            target_term,
            high_qc_term,
            high_qc_index,
        );
        let rpc = ConsensusRpc::TimeoutVote {
            term: target_term,
            voter_id: self.node_id.clone(),
            highest_qc: highest_qc.clone(),
            tc_vote_signature: sig.clone(),
        };
        self.on_timeout_vote(target_term, self.node_id.clone(), highest_qc, sig);
        self.sign_rpc_durable(rpc)
    }

    /// `TimeoutCertificate` (TC) のマルチシグ定足数と添付 QC の正当性を検証する (v0.4.0)
    pub fn verify_timeout_certificate(&self, tc: &TimeoutCertificate) -> Result<(), &'static str> {
        if tc.term == 0 || tc.term >= MAX_SAFE_TERM - 2 {
            return Err("Byzantine TC Invalid: term out of valid bounds");
        }
        if let Some(ref qc) = tc.highest_qc {
            self.verify_qc_signatures_only(qc)?;
        }
        let mut valid_voters = BTreeSet::new();
        for (voter_id, (hq_term, hq_idx, sig_bytes)) in &tc.votes {
            let pk_bytes = self
                .peer_public_keys
                .get(voter_id)
                .ok_or("Byzantine TC Invalid: unknown voter inside TimeoutCertificate")?;
            let payload = timeout_vote_payload(&self.signing_context, tc.term, *hq_term, *hq_idx);
            if !NodeCryptoIdentity::verify_signature(pk_bytes, &payload, sig_bytes) {
                return Err(
                    "Byzantine TC Forgery Detected: Invalid Ed25519 signature in TimeoutCertificate",
                );
            }
            valid_voters.insert(voter_id.clone());
        }
        if valid_voters.len() < self.quorum_size() {
            return Err("Byzantine TC Invalid: lacks required 2f+1 quorum signatures");
        }
        Ok(())
    }

    /// 検証済み `TimeoutCertificate` を適用して強制的に View-Change (`tc.term + 1`) を完了させ、
    /// ラウンドロビン次期リーダーを確定させる (v0.4.0)
    pub fn apply_timeout_certificate(
        &mut self,
        tc: TimeoutCertificate,
    ) -> Result<NodeId, &'static str> {
        if let Err(e) = self.verify_timeout_certificate(&tc) {
            self.rejected_byzantine_packets += 1;
            return Err(e);
        }
        let next_term = (tc.term + 1).min(MAX_SAFE_TERM);
        let next_leader = self.deterministic_leader_for_term(next_term);
        self.timeout_certificates.insert(tc.term, tc.clone());

        if next_term > self.current_term {
            self.election.certificate = Some(crate::domain::TermCertificate::Timeout(tc));
            self.current_term = next_term;
            self.voted_for = Some(next_leader.clone());
            self.votes_received.clear();
            if self.leader_excluded(&next_leader) {
                self.role = Role::Follower;
                self.leader_id = None;
            } else if next_leader == self.node_id {
                self.become_leader();
            } else {
                self.role = Role::Follower;
                self.leader_id = Some(next_leader.clone());
            }
        }
        self.checkpoint()?;
        Ok(next_leader)
    }

    fn on_timeout_vote(
        &mut self,
        term: u64,
        voter_id: NodeId,
        highest_qc: Option<QuorumCertificate>,
        tc_vote_signature: Vec<u8>,
    ) {
        if term == 0 || term != self.current_term || term >= MAX_SAFE_TERM - 1 {
            self.rejected_byzantine_packets += 1;
            return;
        }
        if voter_id != self.node_id && !self.peers.contains(&voter_id) {
            self.rejected_byzantine_packets += 1;
            return;
        }
        if let Some(ref qc) = highest_qc {
            if self.verify_qc_signatures_only(qc).is_err() {
                self.rejected_byzantine_packets += 1;
                return;
            }
        }
        let (hq_term, hq_idx) = highest_qc
            .as_ref()
            .map(|qc| (qc.term, qc.index))
            .unwrap_or((0, 0));
        let pk_bytes = match self.peer_public_keys.get(&voter_id) {
            Some(pk) => pk,
            None => {
                self.rejected_byzantine_packets += 1;
                return;
            }
        };
        let payload = timeout_vote_payload(&self.signing_context, term, hq_term, hq_idx);
        if !NodeCryptoIdentity::verify_signature(pk_bytes, &payload, &tc_vote_signature) {
            self.rejected_byzantine_packets += 1;
            return;
        }

        if let Some(qc) = highest_qc {
            let replace = match &self.highest_observed_qc {
                None => true,
                Some(cur) => (qc.term, qc.index) > (cur.term, cur.index),
            };
            if replace {
                self.highest_observed_qc = Some(qc);
            }
        }

        let quorum = self.quorum_size();
        let maybe_tc = {
            let votes_for_term = self.timeout_votes.entry(term).or_default();
            votes_for_term.insert(voter_id, (hq_term, hq_idx, tc_vote_signature));
            if votes_for_term.len() >= quorum {
                Some(TimeoutCertificate {
                    term,
                    highest_qc: self.highest_observed_qc.clone(),
                    votes: votes_for_term.clone(),
                })
            } else {
                None
            }
        };

        if let Some(tc) = maybe_tc {
            let _ = self.apply_timeout_certificate(tc);
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

        let vote_granted = term == self.current_term
            && can_vote
            && log_is_up_to_date
            && !self.leader_excluded(&candidate_id);
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

    fn make_append_response(&mut self, success: bool) -> ConsensusRpc {
        let match_index = self.last_log_index();
        let (entry_term, entry_hash) = match (self.log.last(), self.entry_hash_at(match_index)) {
            (Some(e), Some(h)) => (e.term, h),
            _ => (self.current_term, [0u8; 32]),
        };
        // A committed prefix may differ from our historical minority vote.
        // Acknowledge its installation without issuing a conflicting signature.
        let conflicting_vote = self
            .voted_entry_hashes
            .get(&(entry_term, match_index))
            .is_some_and(|old| *old != entry_hash);
        let success = success && (!conflicting_vote || match_index <= self.commit_index);
        let qc_vote_signature = if success && match_index > 0 && !conflicting_vote {
            self.voted_entry_hashes
                .insert((entry_term, match_index), entry_hash);
            if self.checkpoint().is_err() {
                return ConsensusRpc::AppendEntriesResponse {
                    term: self.current_term,
                    follower_id: self.node_id.clone(),
                    success: false,
                    match_index,
                    entry_hash,
                    qc_vote_signature: Vec::new(),
                };
            }
            self.identity
                .sign_qc_vote(&self.signing_context, entry_term, match_index, &entry_hash)
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
        prev_log_hash: [u8; 32],
        entries: Vec<LogEntry>,
        leader_commit: u64,
        commit_qc: Option<QuorumCertificate>,
    ) -> ConsensusRpc {
        if term < self.current_term || self.leader_excluded(&leader_id) {
            return self.make_append_response(false);
        }

        if self
            .policy_guard
            .validate_entries(prev_log_index, prev_log_term, term, &entries)
            .is_err()
        {
            self.rejected_byzantine_packets += 1;
            return self.make_append_response(false);
        }

        if prev_log_index > self.last_log_index()
            || self.term_at(prev_log_index) != prev_log_term
            || self.entry_hash_at(prev_log_index).unwrap_or([0; 32]) != prev_log_hash
        {
            return self.make_append_response(false);
        }

        // Validate a candidate log without mutating the live log or vote history.
        // A rejected QC must never leave a branch that a later heartbeat can sign.
        let mut candidate_log = self.log.clone();
        let mut candidate_hashes = self.chain_hashes.clone();
        let mut required_qc_through = 0;
        for entry in &entries {
            let pos = (entry.index - 1) as usize;
            let parent = if pos == 0 {
                [0; 32]
            } else {
                candidate_hashes[pos - 1]
            };
            let hash = entry.digest_with_parent(&parent);
            if self
                .voted_entry_hashes
                .get(&(entry.term, entry.index))
                .is_some_and(|old| *old != hash)
            {
                required_qc_through = required_qc_through.max(entry.index);
            }
            if let Some(existing) = self.log.get(pos) {
                let conflicting = self.chain_hashes[pos] != hash;
                if conflicting && entry.index <= self.commit_index {
                    self.rejected_byzantine_packets += 1;
                    return self.make_append_response(false);
                }
                if conflicting || entry.term < existing.term {
                    required_qc_through = required_qc_through.max(entry.index);
                }
            }
            if candidate_hashes.get(pos) != Some(&hash) {
                candidate_log.truncate(pos);
                candidate_hashes.truncate(pos);
                candidate_log.push(entry.clone());
                candidate_hashes.push(hash);
            }
        }

        let target_commit = leader_commit.min(candidate_log.len() as u64);
        if target_commit > self.commit_index
            && !matches!(commit_qc.as_ref(), Some(qc) if qc.index == target_commit)
        {
            self.rejected_byzantine_packets += 1;
            return self.make_append_response(false);
        }
        if let Some(qc) = &commit_qc {
            let pos = qc
                .index
                .checked_sub(1)
                .and_then(|i| usize::try_from(i).ok());
            let matches_candidate = pos.is_some_and(|i| {
                candidate_log.get(i).is_some_and(|e| e.term == qc.term)
                    && candidate_hashes.get(i) == Some(&qc.entry_hash)
            });
            if !matches_candidate || self.verify_qc_signatures_only(qc).is_err() {
                self.rejected_byzantine_packets += 1;
                return self.make_append_response(false);
            }
        }
        // A verified commit certificate can replace a minority's uncommitted
        // branch. It cannot erase its old votes or authorize another vote in
        // the same slot. Every conflicting position must be covered by this QC.
        if let Some(tip) = candidate_log.last() {
            if self
                .voted_entry_hashes
                .get(&(tip.term, tip.index))
                .is_some_and(|h| Some(h) != candidate_hashes.last())
            {
                required_qc_through = required_qc_through.max(tip.index);
            }
        }
        if required_qc_through > 0
            && !(target_commit >= required_qc_through
                && commit_qc
                    .as_ref()
                    .is_some_and(|qc| qc.index >= required_qc_through))
        {
            self.rejected_byzantine_packets += 1;
            return self.make_append_response(false);
        }

        if term > self.current_term || self.role == Role::Candidate {
            self.step_down(term);
        }
        self.leader_id = Some(leader_id);
        self.log = candidate_log;
        self.chain_hashes = candidate_hashes;
        if target_commit > self.commit_index {
            let qc = commit_qc.expect("commit QC validated before mutation");
            self.commit_qcs.insert(qc.index, qc);
            self.commit_index = target_commit;
        }
        // A heartbeat retries a previously committed but unapplied entry too.
        self.apply_committed_entries();

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
                let expected_chain_hash = match self.entry_hash_at(match_index) {
                    Some(h) => h,
                    None => {
                        self.rejected_byzantine_packets += 1;
                        return;
                    }
                };
                if expected_chain_hash != entry_hash {
                    self.rejected_byzantine_packets += 1;
                    return;
                }
                if qc_vote_signature.is_empty() {
                    // Receipt of an already committed prefix is not a QC vote.
                    if match_index > self.commit_index {
                        self.rejected_byzantine_packets += 1;
                        return;
                    }
                } else {
                    let pk_bytes = match self.peer_public_keys.get(&follower_id) {
                        Some(pk) => pk,
                        None => {
                            self.rejected_byzantine_packets += 1;
                            return;
                        }
                    };
                    let payload = qc_vote_payload(
                        &self.signing_context,
                        leader_entry.term,
                        match_index,
                        &entry_hash,
                    );
                    if !NodeCryptoIdentity::verify_signature(pk_bytes, &payload, &qc_vote_signature)
                    {
                        self.rejected_byzantine_packets += 1;
                        return;
                    }
                    self.qc_votes
                        .entry(match_index)
                        .or_default()
                        .insert(follower_id.clone(), qc_vote_signature);
                }
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
            let entry_term = self.log[(n - 1) as usize].term;
            let entry_hash = match self.entry_hash_at(n) {
                Some(h) => h,
                None => continue,
            };

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

    fn apply_peer_rule_internal(
        &mut self,
        rule: &PeerTrustRule,
        index: u64,
        ordinal: usize,
    ) -> Result<(), String> {
        if index <= self.recovery_through
            && self
                .recovery_latest_rules
                .as_ref()
                .is_some_and(|latest| !latest.contains(&(index, ordinal)))
        {
            return Ok(());
        }
        self.refresh_policy_clock()?;
        let deadline = if rule.ttl_secs > 0 {
            *self
                .lease_deadlines
                .get(&(index, ordinal))
                .ok_or("Lease was not durably prepared")?
        } else {
            0
        };
        if !rule.trusted && (rule.ttl_secs == 0 || deadline == 0) {
            return Err("Refusing BAN without a finite prepared deadline".into());
        }
        // 1 is reserved as an expired recovery tombstone.
        let expired = deadline == 1 || (deadline != 0 && self.monotonic_now_ns >= deadline);
        let mut policy = PeerPolicy::new(rule.trusted || expired, index as u32);
        policy.expires_at_boot_ns = deadline;
        if let Some(ref bridge) = self.ebpf_bridge {
            bridge
                .sync_peer_policy(rule.peer_ip, policy)
                .map_err(|e| e.to_string())?;
        }
        if expired {
            self.ebpf_policy_map.remove(&rule.peer_ip);
            self.peer_rule_deadlines_ns.remove(&rule.peer_ip);
        } else {
            self.ebpf_policy_map.insert(rule.peer_ip, policy);
            if deadline != 0 {
                self.peer_rule_deadlines_ns.insert(rule.peer_ip, deadline);
            } else {
                self.peer_rule_deadlines_ns.remove(&rule.peer_ip);
            }
        }
        Ok(())
    }

    // Reserve every lease in a batch together, before any kernel write. Failed
    // applies and partial batches retain these deadlines rather than rearming.
    fn prepare_leases(&mut self, entry: &LogEntry) -> Result<(), String> {
        self.refresh_policy_clock()?;
        let sample = if entry.command.lease_parts().is_some() {
            // Time uncertainty affects local enforcement, not log agreement.
            // Unknown time permanently suppresses this local activation.
            self.lease_time().ok()
        } else {
            None
        };
        for (ordinal, rule) in entry
            .command
            .batch_rules()
            .enumerate()
            .skip(self.apply_rule_cursor)
        {
            if rule.ttl_secs == 0 {
                continue;
            }
            let candidate = match (entry.command.lease_parts(), sample) {
                (Some((issued_at_unix_ns, rules)), Some(time)) => time
                    .boot_deadline(issued_at_unix_ns, rules[ordinal].not_after_unix_ns)
                    .map_err(|e| e.to_string())?,
                // Legacy relative TTLs carry no signed expiry. Replicate their
                // history, but never start a new lease (even from an old WAL).
                _ => 1,
            };
            self.lease_deadlines
                .entry((entry.index, ordinal))
                .and_modify(|saved| *saved = (*saved).min(candidate))
                .or_insert(candidate);
        }
        self.checkpoint().map_err(str::to_owned)
    }

    pub fn retry_pending_apply(&mut self) {
        if let Err(err) = self.refresh_policy_clock() {
            self.last_apply_error = Some(err);
            return;
        }
        self.last_apply_error = None;
        if self.bypass_sync_pending {
            if let Err(err) = self.sync_bypass() {
                self.last_apply_error = Some(err);
                return;
            }
        }
        self.expire_stale_policies();
        self.apply_committed_entries();
    }

    fn apply_committed_entries(&mut self) {
        if let Err(error) = self.apply_snapshot_if_needed() {
            self.last_apply_error = Some(error);
            return;
        }
        if self.last_applied >= self.commit_index {
            return;
        }
        if self.checkpoint().is_err() {
            return;
        }
        while self.last_applied < self.commit_index {
            let next_apply = self.last_applied + 1;
            let apply_started = std::time::Instant::now();
            tracing::debug!(target: "veil_apply_timing", node = %self.node_id, index = next_apply, "Committed entry application started");
            let entry = self
                .entry_at(next_apply)
                .expect("retained apply entry")
                .clone();
            if let Err(error) = self.verify_operator_command(&entry.command) {
                self.last_apply_error = Some(error.into());
                break;
            }
            if let Err(err) = self.prepare_leases(&entry) {
                self.last_apply_error = Some(err);
                break;
            }
            match &entry.command {
                MeshCommand::SetPeerTrust { peer_ip, trusted } => {
                    let rule = PeerTrustRule {
                        peer_ip: *peer_ip,
                        trusted: *trusted,
                        ttl_secs: 0,
                    };
                    // R4 対策: カーネル eBPF ブリッジへの書き込み結果を先に確認し、失敗時は last_applied を進めずエラーを記録する
                    if let Err(err) = self.apply_peer_rule_internal(&rule, entry.index, 0) {
                        self.last_apply_error = Some(err);
                        break;
                    }
                }
                MeshCommand::SetPeerTrustBatch { .. }
                | MeshCommand::SetPeerTrustLeaseBatch { .. }
                | MeshCommand::AuthorizedPolicy(_) => {
                    let mut failed_err = None;
                    for (ordinal, rule) in entry
                        .command
                        .batch_rules()
                        .enumerate()
                        .skip(self.apply_rule_cursor)
                    {
                        if let Err(err) = self.apply_peer_rule_internal(rule, entry.index, ordinal)
                        {
                            failed_err = Some(err);
                            break;
                        }
                        self.apply_rule_cursor += 1;
                    }
                    if let Some(err) = failed_err {
                        self.last_apply_error = Some(err);
                        break;
                    }
                }
                MeshCommand::SetNodeLockdown {
                    target_node,
                    lockdown,
                } => {
                    if (target_node == &self.node_id || target_node == "*")
                        && (entry.index > self.recovery_through
                            || self
                                .recovery_latest_rules
                                .as_ref()
                                .is_none_or(|latest| latest.contains(&(entry.index, usize::MAX))))
                    {
                        let policy = NodeLsmPolicy::new(*lockdown, entry.index as u32);
                        if let Some(ref bridge) = self.ebpf_bridge {
                            if let Err(err) = bridge.sync_lsm_policy(policy) {
                                self.last_apply_error = Some(err.to_string());
                                break;
                            }
                        }
                        self.lsm_policy = policy;
                    }
                }
                MeshCommand::NoOp | MeshCommand::RetireExpired { .. } => {}
            }
            self.last_apply_error = None;
            self.apply_rule_cursor = 0;
            self.last_applied = next_apply;
            tracing::debug!(target: "veil_apply_timing", node = %self.node_id, index = next_apply, elapsed_us = apply_started.elapsed().as_micros(), "Committed entry application completed");
        }
        if self.last_applied == self.commit_index {
            self.recovery_latest_rules = None;
        }
    }

    /// 単調増加クロックを `delta_secs` 秒進め、期限切れの遮断・信頼ポリシーを自動失効（GC）させる (v0.5.0)
    pub fn advance_monotonic_clock(&mut self, delta_secs: u64) -> usize {
        assert!(
            self.policy_clock.is_none(),
            "Manual clock is simulation-only"
        );
        let delta_ns = delta_secs.saturating_mul(1_000_000_000);
        self.monotonic_now_ns = self.monotonic_now_ns.saturating_add(delta_ns);
        self.expire_stale_policies()
    }

    /// 現在の `monotonic_now_ns` を基準に TTL 期限切れのルールを回収し、カーネル eBPF マップからもエントリを除去する (v0.5.0)
    pub fn expire_stale_policies(&mut self) -> usize {
        if let Err(err) = self.refresh_policy_clock() {
            self.last_apply_error = Some(err);
            return 0;
        }
        let now = self.monotonic_now_ns;
        let expired_ips: Vec<u32> = self
            .peer_rule_deadlines_ns
            .iter()
            .filter_map(|(&ip, &deadline)| if now >= deadline { Some(ip) } else { None })
            .collect();

        let mut removed = 0;
        for ip in expired_ips {
            let default_pass = PeerPolicy::new(true, self.last_applied as u32);
            if let Some(ref bridge) = self.ebpf_bridge {
                if let Err(err) = bridge.sync_peer_policy(ip, default_pass) {
                    self.last_apply_error = Some(err.to_string());
                    continue;
                }
            }
            self.peer_rule_deadlines_ns.remove(&ip);
            self.ebpf_policy_map.remove(&ip);
            removed += 1;
        }
        removed
    }

    /// 緊急ローカルバイパス (`veil-ctl bypass-local on|off` 相当) を切り替える (v0.5.0)
    pub fn set_emergency_local_bypass(&mut self, enabled: bool) -> Result<(), String> {
        // Relieve traffic before a potentially slow fsync. A successful response
        // still requires both the kernel update and persistence. Disabling must
        // persist first, so a failed write never removes an active bypass.
        let previous = self.emergency_local_bypass;
        self.emergency_local_bypass = enabled;
        self.bypass_sync_pending = true;
        if enabled {
            let applied = self.sync_bypass();
            let durable = self.checkpoint().map_err(str::to_owned);
            if let Err(err) = &applied {
                self.last_apply_error = Some(err.clone());
            }
            applied.and(durable)
        } else {
            if let Err(err) = self.checkpoint() {
                self.emergency_local_bypass = previous;
                return Err(err.into());
            }
            self.sync_bypass()
                .inspect_err(|err| self.last_apply_error = Some(err.clone()))
        }
    }

    fn sync_bypass(&mut self) -> Result<(), String> {
        if let Some(bridge) = &self.ebpf_bridge {
            bridge
                .set_xdp_bypass(self.emergency_local_bypass)
                .map_err(|e| e.to_string())?;
        }
        self.bypass_sync_pending = false;
        Ok(())
    }

    /// Prometheus 互換のメトリクス文字列を出力する (v0.5.0 可観測性)
    pub fn render_prometheus_metrics(&self) -> String {
        let blocked_count = self
            .ebpf_policy_map
            .values()
            .filter(|p| p.trusted == 0)
            .count();
        let degraded = u8::from(
            self.last_apply_error.is_some()
                || self.persistence_error.is_some()
                || self.bypass_sync_pending
                || self.lease_clock_error.is_some(),
        );
        let bypass = u8::from(self.emergency_local_bypass);
        format!(
            "# HELP veil_consensus_term Current BFT consensus term\n\
             # TYPE veil_consensus_term gauge\n\
             veil_consensus_term{{node=\"{node}\"}} {term}\n\
             # HELP veil_consensus_commit_index Highest committed log index\n\
             # TYPE veil_consensus_commit_index gauge\n\
             veil_consensus_commit_index{{node=\"{node}\"}} {commit}\n\
             # HELP veil_consensus_last_applied Highest kernel-applied log index\n\
             # TYPE veil_consensus_last_applied gauge\n\
             veil_consensus_last_applied{{node=\"{node}\"}} {applied}\n\
             # HELP veil_ebpf_blocked_ips Active blocked IP count in PEER_POLICY_MAP\n\
             # TYPE veil_ebpf_blocked_ips gauge\n\
             veil_ebpf_blocked_ips{{node=\"{node}\"}} {blocked}\n\
             # HELP veil_rejected_byzantine_packets_total Total rejected Byzantine/malformed RPC packets\n\
             # TYPE veil_rejected_byzantine_packets_total counter\n\
             veil_rejected_byzantine_packets_total{{node=\"{node}\"}} {rejected}\n\
             # HELP veil_node_degraded Kernel eBPF synchronization degraded flag (1=degraded, 0=healthy)\n\
             # TYPE veil_node_degraded gauge\n\
             veil_node_degraded{{node=\"{node}\"}} {degraded}\n\
             # HELP veil_emergency_local_bypass Desired local XDP bypass flag (1=bypass, 0=enforcing; consult degraded)\n\
             # TYPE veil_emergency_local_bypass gauge\n\
             veil_emergency_local_bypass{{node=\"{node}\"}} {bypass}\n",
            node = self.node_id,
            term = self.current_term,
            commit = self.commit_index,
            applied = self.last_applied,
            blocked = blocked_count,
            rejected = self.rejected_byzantine_packets,
            degraded = degraded,
            bypass = bypass,
        )
    }

    pub fn check_xdp_packet(&self, src_ip: Ipv4Addr) -> XdpVerdict {
        if self.emergency_local_bypass {
            return XdpVerdict::Pass;
        }
        let ip_u32 = u32::from(src_ip);
        if let Some(&deadline) = self.peer_rule_deadlines_ns.get(&ip_u32) {
            if self.monotonic_now_ns >= deadline {
                return XdpVerdict::Pass;
            }
        }
        let policy = self.ebpf_policy_map.get(&ip_u32);
        let now = self
            .policy_clock
            .as_ref()
            .and_then(|c| c.now_ns().ok())
            .unwrap_or(self.monotonic_now_ns);
        evaluate_peer_policy_at(policy, now)
    }

    pub fn check_lsm_socket_connect(&self) -> i32 {
        evaluate_lsm_policy(Some(&self.lsm_policy))
    }

    /// 再起動時の二重投票・状態巻き戻りを防ぐ Crash-Recovery WAL スナップショットを `0600` + `sync_all` + SHA-256 検査付きで保存する
    pub fn save_wal(&self, path: &std::path::Path) -> std::io::Result<()> {
        let store = crate::infra::wal::FileWal::open(path)?.with_limit(self.max_wal_bytes);
        store.persist(&self.wal_bytes()?)
    }

    fn wal_bytes(&self) -> std::io::Result<Vec<u8>> {
        use ring::digest;
        let payload = WalPayload {
            version: if self.uses_two_phase() {
                9
            } else {
                WAL_MAGIC_VERSION
            },
            context: self.signing_context.clone(),
            boot_id: self.boot_id.clone(),
            lease_deadlines: self.lease_deadlines.clone(),
            emergency_local_bypass: self.emergency_local_bypass,
            node_id: self.node_id.clone(),
            current_term: self.current_term,
            voted_for: self.voted_for.clone(),
            log: self.log.clone(),
            commit_index: self.commit_index,
            commit_qcs: self.commit_qcs.clone(),
            voted_entry_hashes: self
                .voted_entry_hashes
                .iter()
                .map(|(&(t, i), &h)| (t, i, h))
                .collect(),
        };
        // v6 appends witnessed requests and progress certificates. The v4/v5
        // vote/log/certificate fields retain their ordering and values.
        let payload_bytes = if let Some(s) = &self.two_phase {
            bincode::serialize(&(
                &payload,
                &self.election.certificate,
                &self.progress.durable,
                &s.durable,
            ))
        } else {
            bincode::serialize(&(&payload, &self.election.certificate, &self.progress.durable))
        }
        .map_err(std::io::Error::other)?;
        let digest_val = digest::digest(&digest::SHA256, &payload_bytes);
        let mut sha256_checksum = [0u8; 32];
        sha256_checksum.copy_from_slice(digest_val.as_ref());

        let envelope = WalEnvelope {
            payload_bytes,
            sha256_checksum,
        };
        let size = bincode::serialized_size(&envelope).map_err(std::io::Error::other)?;
        if size > self.max_wal_bytes {
            return Err(std::io::Error::other(
                "WAL byte limit reached; preserve WAL and increase capacity deliberately",
            ));
        }
        bincode::serialize(&envelope).map_err(std::io::Error::other)
    }

    /// Crash-Recovery WAL から永続化状態を検証・復元し、確定済みエントリを eBPF ブリッジへ再同期する
    pub fn load_wal(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let result = self.load_wal_inner(path);
        if let Err(error) = &result {
            self.persistence_error = Some(error.to_string());
            self.role = Role::Follower;
        }
        result
    }
    fn decode_wal<T: serde::de::DeserializeOwned>(&self, bytes: &[u8]) -> bincode::Result<T> {
        use bincode::Options;
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(self.max_wal_bytes)
            .reject_trailing_bytes()
            .deserialize(bytes)
    }
    fn load_wal_inner(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        use ring::digest;
        if !self.log.is_empty() || self.persistence.is_some() {
            return Err(std::io::Error::other("WAL recovery requires a fresh state"));
        }
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Refusing symlink WAL file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o600 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Insecure WAL permissions {:04o}: expected 0600", mode),
                ));
            }
        }

        if meta.len() > self.max_wal_bytes {
            return Err(std::io::Error::other(
                "Existing WAL exceeds configured byte limit; preserve history",
            ));
        }
        use std::io::Read;
        let mut raw = Vec::new();
        std::fs::File::open(path)?
            .take(self.max_wal_bytes.saturating_add(1))
            .read_to_end(&mut raw)?;
        if raw.len() as u64 > self.max_wal_bytes {
            return Err(std::io::Error::other(
                "WAL grew beyond byte limit while reading",
            ));
        }
        let envelope: WalEnvelope = self
            .decode_wal(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let actual_digest = digest::digest(&digest::SHA256, &envelope.payload_bytes);
        if actual_digest.as_ref() != envelope.sha256_checksum {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WAL checksum mismatch (corrupted or tampered WAL)",
            ));
        }

        let version = envelope.payload_bytes.get(..4);
        if self.uses_two_phase() != (version == Some(&9u32.to_le_bytes())) {
            self.persistence_error = Some(
                "Protocol/WAL mismatch; preserve original history, no automatic migration".into(),
            );
            return Err(std::io::Error::other("Protocol/WAL mismatch: preserve legacy WAL and identity; explicit migration is required"));
        }
        let mut phase_durable = None;
        let (payload, certificate, progress): (
            WalPayload,
            Option<crate::domain::TermCertificate>,
            progress::ProgressDurable,
        ) = if version == Some(&9u32.to_le_bytes()) {
            let (payload, cert, progress, phase): (
                WalPayload,
                Option<crate::domain::TermCertificate>,
                progress::ProgressDurable,
                two_phase::Durable,
            ) = self
                .decode_wal(&envelope.payload_bytes)
                .map_err(std::io::Error::other)?;
            self.verify_phase_wal(&payload, &phase)
                .map_err(std::io::Error::other)?;
            phase_durable = Some(phase);
            (payload, cert, progress)
        } else if version == Some(&WAL_MAGIC_VERSION.to_le_bytes()) {
            self.decode_wal(&envelope.payload_bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
        } else if version == Some(&5u32.to_le_bytes()) {
            let (payload, cert) = self
                .decode_wal(&envelope.payload_bytes)
                .map_err(std::io::Error::other)?;
            (payload, cert, progress::ProgressDurable::default())
        } else if version == Some(&4u32.to_le_bytes()) {
            (
                self.decode_wal(&envelope.payload_bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
                None,
                progress::ProgressDurable::default(),
            )
        } else {
            return Err(std::io::Error::other(
                "Unsupported WAL version: preserve old WAL; explicit migration required",
            ));
        };
        if payload.node_id != self.node_id || payload.context != self.signing_context {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WAL node_id or signature context mismatch",
            ));
        }
        if let Some(cert) = &certificate {
            if cert.term() != payload.current_term {
                return Err(std::io::Error::other(
                    "WAL term evidence does not match durable term",
                ));
            }
            self.verify_term_certificate(cert)
                .map_err(std::io::Error::other)?;
        }

        self.verify_progress_wal(&progress, payload.current_term)
            .map_err(std::io::Error::other)?;
        for entry in &payload.log {
            if let Err(error) = self.verify_operator_command(&entry.command) {
                // Preserve the original WAL bytes/votes. Never convert old signed
                // history or let a caller continue voting with an empty state.
                self.persistence_error = Some(error.into());
                self.role = Role::Follower;
                return Err(std::io::Error::other(format!(
                    "WAL contains a forbidden policy at index {}; preserve WAL and stop this validator: {error}",
                    entry.index
                )));
            }
        }
        self.refresh_policy_clock().map_err(std::io::Error::other)?;
        self.emergency_local_bypass = payload.emergency_local_bypass;
        // Restore bypass before any rule, never temporarily reinstate a historical BAN.
        self.sync_bypass().map_err(std::io::Error::other)?;
        self.lease_deadlines = payload.lease_deadlines;
        if payload.boot_id != self.boot_id {
            self.lease_deadlines
                .values_mut()
                .for_each(|deadline| *deadline = 1);
        }
        if let Some(durable) = phase_durable {
            self.two_phase = Some(two_phase::State {
                durable,
                ..Default::default()
            });
        }
        self.election = election::ElectionState::default();
        self.progress = progress::ProgressState::default();
        self.progress.durable = progress;
        self.election.certificate = certificate;
        self.election_activity = 0;
        self.role = Role::Follower;
        self.leader_id = None;
        self.votes_received.clear();
        self.current_term = payload.current_term;
        self.voted_for = payload.voted_for;
        self.truncate_log(0);
        for entry in payload.log {
            self.push_log_entry(entry);
        }
        self.voted_entry_hashes = payload
            .voted_entry_hashes
            .into_iter()
            .map(|(t, i, h)| ((t, i), h))
            .collect();
        self.commit_qcs = payload.commit_qcs;
        self.commit_index = payload.commit_index.min(self.last_log_index());
        // A committed snapshot can precede lease preparation. Absence is not
        // permission to create a new lease after recovery; retain an expired tombstone.
        let mut latest = BTreeMap::new();
        let mut latest_lsm = None;
        for entry in self.log.iter().take_while(|e| e.index <= self.commit_index) {
            match &entry.command {
                MeshCommand::SetPeerTrust { peer_ip, .. } => {
                    latest.insert(*peer_ip, (entry.index, 0));
                }
                MeshCommand::SetPeerTrustBatch { .. }
                | MeshCommand::SetPeerTrustLeaseBatch { .. }
                | MeshCommand::AuthorizedPolicy(_) => {
                    for (ordinal, rule) in entry.command.batch_rules().enumerate() {
                        latest.insert(rule.peer_ip, (entry.index, ordinal));
                        if rule.ttl_secs > 0 {
                            self.lease_deadlines
                                .entry((entry.index, ordinal))
                                .or_insert(1);
                        }
                    }
                }
                MeshCommand::SetNodeLockdown { target_node, .. }
                    if target_node == &self.node_id || target_node == "*" =>
                {
                    latest_lsm = Some((entry.index, usize::MAX));
                }
                _ => {}
            }
        }
        let mut selected: BTreeSet<_> = latest.into_values().collect();
        if let Some(stable) = self
            .two_phase
            .as_ref()
            .and_then(|s| s.durable.stable.as_ref())
        {
            for r in stable.state.rules.values() {
                if !self.log.iter().take_while(|e|e.index<=self.commit_index).any(|e|e.command.batch_rules().any(|rule|rule.peer_ip == r.rule.peer_ip) || matches!(&e.command, MeshCommand::SetPeerTrust { peer_ip, .. } if *peer_ip == r.rule.peer_ip)) { selected.insert((r.index,r.ordinal)); }
            }
        }
        selected.extend(latest_lsm);
        self.recovery_latest_rules = Some(selected);
        self.recovery_through = self.commit_index;
        self.last_applied = 0;
        self.apply_rule_cursor = 0;
        self.apply_committed_entries();
        // Failed kernel application remains degraded and retryable; caller must
        // inspect last_applied before claiming enforcement is restored.
        Ok(())
    }
}

#[cfg(test)]
#[path = "regression_tests.rs"]
mod regression_tests;

#[cfg(test)]
#[path = "lease_tests.rs"]
mod lease_tests;

#[cfg(test)]
#[path = "deadline_tests.rs"]
mod deadline_tests;

#[cfg(test)]
#[path = "context_tests.rs"]
mod context_tests;

#[cfg(test)]
#[path = "membership_tests.rs"]
mod membership_tests;

#[cfg(test)]
#[path = "finite_ttl_tests.rs"]
mod finite_ttl_tests;

#[cfg(test)]
#[path = "election_tests.rs"]
mod election_tests;

#[cfg(test)]
#[path = "prefix_recovery_tests.rs"]
mod prefix_recovery_tests;

#[cfg(test)]
#[path = "two_phase_tests.rs"]
pub(crate) mod two_phase_tests;

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod checkpoint_tests;

#[cfg(test)]
#[path = "authorization_tests.rs"]
mod authorization_tests;
