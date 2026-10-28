//! Authenticated state replacement with a durable low watermark.
use super::*;
use crate::domain::{
    CertifiedSnapshot, CheckpointVote, Phase, PhaseMessage, PolicySnapshot, SnapshotRule,
    CHECKPOINT_INTERVAL, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_RULES,
};

impl ConsensusState {
    /// Committed retirement only; a pending cleanup proposal is not a fence.
    pub fn retired_through_unix_ns(&self) -> u64 {
        let base = self
            .two_phase
            .as_ref()
            .and_then(|s| s.durable.stable.as_ref())
            .map_or(0, |s| s.state.retired_through_unix_ns);
        self.log
            .iter()
            .take_while(|e| e.index <= self.commit_index)
            .filter_map(|e| match e.command {
                MeshCommand::RetireExpired { through_unix_ns } => Some(through_unix_ns),
                _ => None,
            })
            .fold(base, u64::max)
    }

    pub fn log_base(&self) -> u64 {
        self.two_phase
            .as_ref()
            .and_then(|s| s.durable.stable.as_ref())
            .map_or(0, |s| s.state.height)
    }
    pub(super) fn base_hash(&self) -> [u8; 32] {
        self.two_phase
            .as_ref()
            .and_then(|s| s.durable.stable.as_ref())
            .map_or([0; 32], |s| s.state.hash)
    }
    pub(super) fn entry_at(&self, index: u64) -> Option<&LogEntry> {
        index
            .checked_sub(self.log_base() + 1)
            .and_then(|i| self.log.get(i as usize))
    }
    fn fold_snapshot(
        &self,
        state: &mut PolicySnapshot,
        entry: &LogEntry,
    ) -> Result<(), &'static str> {
        self.verify_operator_command(&entry.command)?;
        if let MeshCommand::AuthorizedPolicy(a) = &entry.command {
            if state.authorization_revision.checked_add(1) != Some(a.revision) {
                return Err(
                    "Operator revision conflict/replay; obtain current revision before approving",
                );
            }
            state.authorization_revision = a.revision;
        }
        match &entry.command {
            MeshCommand::RetireExpired { through_unix_ns } => {
                state.retired_through_unix_ns = state.retired_through_unix_ns.max(*through_unix_ns);
                state
                    .rules
                    .retain(|_, r| r.not_after > state.retired_through_unix_ns);
            }
            MeshCommand::SetNodeLockdown {
                target_node,
                lockdown,
            } => {
                if target_node != "*" && !self.peer_public_keys.contains_key(target_node) {
                    return Err("Snapshot requires a fixed-member lockdown target");
                }
                for id in self
                    .peer_public_keys
                    .keys()
                    .filter(|id| target_node == "*" || *id == target_node)
                {
                    state.lockdown.insert(id.clone(), (entry.index, *lockdown));
                }
            }
            _ => {
                for (ordinal, rule) in entry.command.batch_rules().enumerate() {
                    if rule.trusted {
                        state.rules.remove(&rule.peer_ip);
                    } else {
                        let Some((issued_at_unix_ns, rules)) = entry.command.lease_parts() else {
                            return Err("Snapshot BAN requires signed absolute expiry");
                        };
                        if rules[ordinal].not_after_unix_ns <= state.retired_through_unix_ns {
                            continue;
                        }
                        state.rules.insert(
                            rule.peer_ip,
                            SnapshotRule {
                                index: entry.index,
                                ordinal,
                                issued_at: issued_at_unix_ns,
                                not_after: rules[ordinal].not_after_unix_ns,
                                rule: rule.clone(),
                            },
                        );
                    }
                }
            }
        }
        // The one-rule legacy unban has no batch iterator.
        if let MeshCommand::SetPeerTrust {
            peer_ip,
            trusted: true,
        } = entry.command
        {
            state.rules.remove(&peer_ip);
        }
        if state.rules.len() > MAX_SNAPSHOT_RULES {
            return Err(
                "Snapshot policy capacity reached; unban obsolete IPs before adding new ones",
            );
        }
        if bincode::serialized_size(state).map_err(|_| "Snapshot encoding")? > MAX_SNAPSHOT_BYTES {
            return Err("Snapshot exceeds bounded transfer size");
        }
        Ok(())
    }
    fn image_through(&self, height: u64) -> Result<PolicySnapshot, &'static str> {
        let mut image = self
            .two_phase
            .as_ref()
            .and_then(|s| s.durable.stable.as_ref())
            .map(|s| s.state.clone())
            .unwrap_or_default();
        if height < image.height || height > self.commit_index {
            return Err("Snapshot outside retained committed range");
        }
        for entry in self.log.iter().take_while(|e| e.index <= height) {
            self.fold_snapshot(&mut image, entry)?;
        }
        image.height = height;
        image.hash = self
            .entry_hash_at(height)
            .ok_or("Missing checkpoint prefix hash")?;
        Ok(image)
    }
    pub(super) fn snapshot_with_command(&self, entry: &LogEntry) -> Result<(), &'static str> {
        let mut image = if self.commit_index == 0 {
            PolicySnapshot::default()
        } else {
            self.image_through(self.commit_index)?
        };
        self.fold_snapshot(&mut image, entry)
    }
    fn verify_checkpoint_vote(&self, v: &CheckpointVote) -> Result<(), &'static str> {
        if v.height == 0 || !v.height.is_multiple_of(CHECKPOINT_INTERVAL) {
            return Err("Invalid checkpoint height");
        }
        let key = self
            .peer_public_keys
            .get(&v.voter)
            .ok_or("Unknown checkpoint voter")?;
        if !NodeCryptoIdentity::verify_signature(
            key,
            &v.payload(&self.signing_context),
            &v.signature,
        ) {
            return Err("Invalid checkpoint signature/context");
        }
        Ok(())
    }
    pub(super) fn verify_snapshot(&self, s: &CertifiedSnapshot) -> Result<(), &'static str> {
        self.verify_phase_certificate(&s.commit)?;
        if s.commit.phase != Phase::Commit
            || s.state.height != s.commit.block.entry.index
            || s.state.hash != s.commit.block.hash()
            || s.votes.len() < self.quorum_size()
            || s.votes.len() > self.cluster_size()
            || s.state.rules.len() > MAX_SNAPSHOT_RULES
            || s.state.lockdown.len() > self.cluster_size()
            || bincode::serialized_size(&s.state).map_err(|_| "Snapshot encoding")?
                > MAX_SNAPSHOT_BYTES
        {
            return Err("Invalid checkpoint certificate or bounds");
        }
        let digest = s.state.digest();
        for (id, v) in &s.votes {
            self.verify_checkpoint_vote(v)?;
            if id != &v.voter
                || v.height != s.state.height
                || v.hash != s.state.hash
                || v.state_hash != digest
            {
                return Err("Checkpoint vote mismatch");
            }
        }
        for (ip, r) in &s.state.rules {
            if *ip != r.rule.peer_ip
                || r.rule.trusted
                || r.not_after <= s.state.retired_through_unix_ns
                || r.index == 0
                || r.index > s.state.height
                || r.ordinal >= 1024
            {
                return Err("Invalid snapshot rule position");
            }
            let mut shape_guard = self.policy_guard.clone();
            shape_guard.approval = None;
            shape_guard.validate_log_command(&MeshCommand::SetPeerTrustLeaseBatch {
                issued_at_unix_ns: r.issued_at,
                rules: vec![crate::domain::DeadlinePeerTrustRule {
                    rule: r.rule.clone(),
                    not_after_unix_ns: r.not_after,
                }],
            })?;
        }
        for (id, (index, _)) in &s.state.lockdown {
            if !self.peer_public_keys.contains_key(id) || *index == 0 || *index > s.state.height {
                return Err("Invalid snapshot lockdown");
            }
        }
        Ok(())
    }
    pub(super) fn snapshot_rpc(&self) -> Option<ConsensusRpc> {
        self.two_phase
            .as_ref()?
            .durable
            .stable
            .as_ref()
            .map(|s| self.phase_rpc(PhaseMessage::Snapshot(Box::new(s.clone()))))
    }
    /// Each member gossips at the boundary; checkpoint progress never depends on
    /// a leader forwarding the votes. The pending image is reconstructed from WAL.
    pub fn drive_stable_checkpoint(&mut self) -> Result<Option<SignedConsensusRpc>, &'static str> {
        if self.two_phase.is_none() {
            return Ok(None);
        }
        self.checkpoint()?;
        if self.commit_index != self.log_base() + CHECKPOINT_INTERVAL
            || self.last_applied != self.commit_index
        {
            return Ok(None);
        }
        let image = self.image_through(self.commit_index)?;
        let mut vote = CheckpointVote {
            height: image.height,
            hash: image.hash,
            state_hash: image.digest(),
            voter: self.node_id.clone(),
            signature: vec![],
        };
        if let Some(saved) = &self.two_phase.as_ref().unwrap().durable.checkpoint_vote {
            if (saved.height, saved.hash, saved.state_hash)
                != (vote.height, vote.hash, vote.state_hash)
            {
                return Err("Checkpoint signing fence mismatch");
            }
            vote = saved.clone();
        } else {
            vote.signature = self
                .identity
                .sign_progress_payload(&vote.payload(&self.signing_context));
            self.two_phase.as_mut().unwrap().durable.checkpoint_vote = Some(vote.clone());
            self.checkpoint()?;
        }
        self.two_phase
            .as_mut()
            .unwrap()
            .checkpoint_votes
            .insert(self.node_id.clone(), vote.clone());
        self.try_stabilize()?;
        self.sign_rpc_durable(self.phase_rpc(PhaseMessage::CheckpointVote(vote)))
            .map(Some)
    }
    pub(super) fn on_checkpoint_vote(
        &mut self,
        vote: CheckpointVote,
    ) -> Result<ConsensusRpc, &'static str> {
        self.verify_checkpoint_vote(&vote)?;
        if vote.height <= self.log_base() {
            return self.snapshot_rpc().ok_or("No checkpoint available");
        }
        if vote.height != self.commit_index
            || self.commit_index != self.log_base() + CHECKPOINT_INTERVAL
        {
            return Err("Checkpoint not locally reached");
        }
        let image = self.image_through(vote.height)?;
        if vote.hash != image.hash || vote.state_hash != image.digest() {
            return Err("Checkpoint differs from committed state");
        }
        self.two_phase
            .as_mut()
            .unwrap()
            .checkpoint_votes
            .insert(vote.voter.clone(), vote);
        // Local signature must itself be durable before it can be returned.
        self.drive_stable_checkpoint()?;
        if let Some(rpc) = self
            .snapshot_rpc()
            .filter(|_| self.log_base() == image.height)
        {
            return Ok(rpc);
        }
        let own = self
            .two_phase
            .as_ref()
            .unwrap()
            .durable
            .checkpoint_vote
            .clone()
            .ok_or("Checkpoint application pending")?;
        Ok(self.phase_rpc(PhaseMessage::CheckpointVote(own)))
    }
    fn try_stabilize(&mut self) -> Result<(), &'static str> {
        let phase = self.two_phase.as_ref().unwrap();
        if phase.checkpoint_votes.len() < self.quorum_size() {
            return Ok(());
        }
        let s = CertifiedSnapshot {
            state: self.image_through(self.commit_index)?,
            commit: phase.durable.commits[&self.commit_index].clone(),
            votes: phase.checkpoint_votes.clone(),
        };
        self.install_snapshot(s)
    }
    pub(super) fn install_snapshot(&mut self, s: CertifiedSnapshot) -> Result<(), &'static str> {
        self.verify_snapshot(&s)?;
        let height = s.state.height;
        if height < self.commit_index || height <= self.log_base() {
            return Ok(());
        }
        if height == self.commit_index && self.image_through(height)? != s.state {
            return Err("Checkpoint conflicts with local committed state");
        }
        let old_commit = self.commit_index;
        let old_base = self.log_base();
        let keep_suffix = height == old_commit;
        let sample = self.lease_time().ok();
        let mut deadlines = BTreeMap::new();
        for r in s.state.rules.values() {
            let key = (r.index, r.ordinal);
            let deadline = self.lease_deadlines.get(&key).copied().unwrap_or_else(|| {
                if r.index <= old_commit {
                    1
                } else {
                    sample
                        .and_then(|t| t.boot_deadline(r.issued_at, r.not_after).ok())
                        .unwrap_or(1)
                }
            });
            deadlines.insert(key, deadline);
        }
        deadlines.extend(
            self.lease_deadlines
                .iter()
                .filter(|((i, _), _)| *i > height)
                .map(|(k, v)| (*k, *v)),
        );
        self.remember_compacted_requests(height);
        if s.commit.view > self.current_term {
            self.step_down(s.commit.view);
            self.election.certificate = None;
        }
        if keep_suffix {
            let remove = (height - old_base) as usize;
            self.log.drain(..remove);
            self.chain_hashes.drain(..remove);
        } else {
            self.log.clear();
            self.chain_hashes.clear();
        }
        let phase = self.two_phase.as_mut().unwrap();
        phase.durable.commits.retain(|i, _| *i > height);
        phase.durable.commits.insert(height, s.commit.clone());
        phase.durable.votes.retain(|(_, _, i), _| *i > height);
        if phase
            .durable
            .prepared
            .as_ref()
            .is_some_and(|c| c.block.entry.index <= height)
        {
            phase.durable.prepared = None;
        }
        phase.durable.stable = Some(s);
        phase.durable.checkpoint_vote = None;
        phase.checkpoint_votes.clear();
        phase.snapshot_applied = self.last_applied >= height;
        if phase.snapshot_applied {
            let rules = &phase.durable.stable.as_ref().unwrap().state.rules;
            // Expiry may have failed to remove a kernel entry. Keep its retry
            // record until expire_stale_policies confirms backend removal.
            self.ebpf_policy_map.retain(|ip, _| {
                rules.contains_key(ip) || self.peer_rule_deadlines_ns.contains_key(ip)
            });
        }
        self.commit_index = height;
        self.lease_deadlines = deadlines;
        // The new low watermark and proof replace the old WAL atomically before
        // any response or enforcement. No separate file deletion / rename pair.
        self.checkpoint()?;
        self.apply_committed_entries();
        self.checkpoint()
    }
    pub(super) fn apply_snapshot_if_needed(&mut self) -> Result<(), String> {
        let Some(phase) = &self.two_phase else {
            return Ok(());
        };
        if phase.snapshot_applied {
            return Ok(());
        }
        let Some(stable) = phase.durable.stable.clone() else {
            return Ok(());
        };
        self.checkpoint().map_err(str::to_owned)?;
        for ip in self
            .ebpf_policy_map
            .keys()
            .filter(|ip| !stable.state.rules.contains_key(ip))
            .copied()
            .collect::<Vec<_>>()
        {
            let pass = PeerPolicy::new(true, stable.state.height as u32);
            if let Some(bridge) = &self.ebpf_bridge {
                bridge
                    .sync_peer_policy(ip, pass)
                    .map_err(|e| e.to_string())?;
            }
            self.ebpf_policy_map.remove(&ip);
            self.peer_rule_deadlines_ns.remove(&ip);
        }
        for r in stable.state.rules.values() {
            self.apply_peer_rule_internal(&r.rule, r.index, r.ordinal)?;
        }
        if let Some((index, lockdown)) = stable.state.lockdown.get(&self.node_id).filter(|_| !self.log.iter().take_while(|e|e.index<=self.recovery_through).any(|e|matches!(&e.command, MeshCommand::SetNodeLockdown { target_node, .. } if target_node == &self.node_id || target_node == "*"))) {
            let p = NodeLsmPolicy::new(*lockdown, *index as u32);
            if let Some(bridge) = &self.ebpf_bridge {
                bridge.sync_lsm_policy(p).map_err(|e| e.to_string())?;
            }
            self.lsm_policy = p;
        }
        self.last_applied = stable.state.height;
        self.two_phase.as_mut().unwrap().snapshot_applied = true;
        Ok(())
    }
    pub(super) fn retire_expired_if_idle(&mut self) -> Result<(), &'static str> {
        if self.role != Role::Leader
            || self.last_log_index() != self.commit_index
            || self.commit_index == 0
            || self.commit_index >= self.log_base() + CHECKPOINT_INTERVAL
        {
            return Ok(());
        }
        let Ok(time) = self.lease_time() else {
            return Ok(());
        };
        let cutoff = time.unix_ns.saturating_sub(time.uncertainty_ns);
        let image = self.image_through(self.commit_index)?;
        if image.rules.values().any(|r| r.not_after <= cutoff) {
            self.propose(MeshCommand::RetireExpired {
                through_unix_ns: cutoff,
            })?;
        }
        Ok(())
    }
    pub(super) fn verify_checkpoint_wal(
        &self,
        payload: &WalPayload,
        d: &two_phase::Durable,
    ) -> Result<(), &'static str> {
        if let Some(stable) = &d.stable {
            if d.commits.get(&stable.state.height) != Some(&stable.commit)
                || stable.commit.view > payload.current_term
            {
                return Err("Checkpoint WAL anchor mismatch");
            }
            for r in stable.state.rules.values() {
                if !payload.lease_deadlines.contains_key(&(r.index, r.ordinal)) {
                    return Err("Checkpoint WAL missing local deadline/tombstone");
                }
            }
        }
        if let Some(v) = &d.checkpoint_vote {
            self.verify_checkpoint_vote(v)?;
            let base = d.stable.as_ref().map_or(0, |s| s.state.height);
            if v.voter != self.node_id
                || v.height != base + CHECKPOINT_INTERVAL
                || v.height != payload.commit_index
            {
                return Err("Invalid durable checkpoint vote");
            }
            let mut image = d
                .stable
                .as_ref()
                .map(|s| s.state.clone())
                .unwrap_or_default();
            for e in payload.log.iter().take_while(|e| e.index <= v.height) {
                self.fold_snapshot(&mut image, e)?;
            }
            image.height = v.height;
            image.hash = d
                .commits
                .get(&v.height)
                .ok_or("Missing checkpoint commit")?
                .block
                .hash();
            if image.hash != v.hash || image.digest() != v.state_hash {
                return Err("WAL checkpoint vote/image mismatch");
            }
        }
        if d.request_receipts.len() > self.cluster_size() * 2 {
            return Err("Unbounded compacted receipts");
        }
        Ok(())
    }
}
