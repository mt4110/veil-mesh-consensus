use super::*;
use crate::domain::{
    phase_vote_payload, NewView, Phase, PhaseBlock, PhaseCertificate, PhaseMessage, ViewReport,
    MAX_PHASE_COMMAND_BYTES, MAX_PHASE_MEMBERS,
};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Durable {
    pub prepared: Option<PhaseCertificate>,
    pub commits: BTreeMap<u64, PhaseCertificate>,
    pub votes: BTreeMap<(Phase, u64, u64), [u8; 32]>,
    pub report: Option<ViewReport>,
    pub active: Option<NewView>,
    pub stable: Option<crate::domain::CertifiedSnapshot>,
    pub checkpoint_vote: Option<crate::domain::CheckpointVote>,
    pub request_receipts: BTreeMap<[u8; 32], (u64, [u8; 32])>,
}
#[derive(Default)]
pub(super) struct State {
    pub durable: Durable,
    pub(super) reports: BTreeMap<NodeId, ViewReport>,
    pub(super) votes: BTreeMap<Phase, BTreeMap<NodeId, Vec<u8>>>,
    pub(super) vote_target: Option<(u64, u64, [u8; 32])>,
    pub(super) initialized_view: Option<u64>,
    pub checkpoint_votes: BTreeMap<NodeId, crate::domain::CheckpointVote>,
    pub snapshot_applied: bool,
}
impl ConsensusState {
    /// Only a leader's verified prepare/commit progress can trigger an immediate
    /// replication followup. This is a volatile observation, not new evidence.
    pub(crate) fn phase_replication_position(&self) -> Option<(u64, u64, u64, u64)> {
        let s = self.two_phase.as_ref()?;
        if self.role != Role::Leader || self.persistence_error.is_some() {
            return None;
        }
        let prepared = s.durable.prepared.as_ref();
        Some((
            self.current_term,
            self.commit_index,
            prepared.map_or(0, |c| c.view),
            prepared.map_or(0, |c| c.block.entry.index),
        ))
    }

    /// Test-only labels for an in-flight snapshot, including when the state
    /// mutex is busy. Never print commands, keys, signatures, or snapshot bytes.
    #[cfg(test)]
    pub(crate) fn phase_snapshot_diagnostics(bytes: &[u8]) -> bincode::Result<String> {
        use bincode::Options;
        let envelope: WalEnvelope = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(bytes.len() as u64)
            .reject_trailing_bytes()
            .deserialize(bytes)?;
        let (p, _, _, s): (
            WalPayload,
            Option<crate::domain::TermCertificate>,
            progress::ProgressDurable,
            Durable,
        ) = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(envelope.payload_bytes.len() as u64)
            .reject_trailing_bytes()
            .deserialize(&envelope.payload_bytes)?;
        let vote = |phase| {
            s.votes
                .keys()
                .filter(|(ph, _, _)| *ph == phase)
                .map(|(_, term, index)| (*term, *index))
                .max()
        };
        Ok(format!(
            "term={} log={} commit={} prepare_vote={:?} commit_vote={:?} prepared={:?}",
            p.current_term,
            p.log.last().map_or(0, |e| e.index),
            p.commit_index,
            vote(Phase::Prepare),
            vote(Phase::Commit),
            s.prepared.as_ref().map(|c| (c.view, c.block.entry.index)),
        ))
    }

    #[cfg(test)]
    pub(crate) fn phase_diagnostics(&self) -> String {
        let s = self.two_phase.as_ref().expect("two phase fixture");
        format!("role={:?} term={} leader={:?} voted_for={:?} log={} commit={} applied={} reports={:?} prepare_voters={:?} commit_voters={:?} prepared={:?} persistence={:?}",
            self.role, self.current_term, self.leader_id, self.voted_for,
            self.last_log_index(), self.commit_index, self.last_applied,
            s.reports.keys().collect::<Vec<_>>(),
            s.votes.get(&Phase::Prepare).map(|v| v.keys().collect::<Vec<_>>()),
            s.votes.get(&Phase::Commit).map(|v| v.keys().collect::<Vec<_>>()),
            s.durable.prepared.as_ref().map(|c| (c.view, c.block.entry.index)),
            self.persistence_error)
    }
    /// New contexts only. No legacy vote/QC/WAL is reinterpreted as a phase vote.
    pub fn enable_two_phase(&mut self) -> Result<(), &'static str> {
        if self.current_term != 0
            || !self.log.is_empty()
            || self.persistence.is_some()
            || self.two_phase.is_some()
            || !self.voted_entry_hashes.is_empty()
        {
            return Err(
                "Two-phase activation requires a fresh state, never an in-place history reset",
            );
        }
        if self.cluster_size() > MAX_PHASE_MEMBERS {
            return Err("Two-phase pilot supports at most 7 fixed voters");
        }
        self.signing_context.protocol_version = 6;
        self.two_phase = Some(State::default());
        Ok(())
    }
    pub fn uses_two_phase(&self) -> bool {
        self.two_phase.is_some()
    }
    pub(super) fn phase_rpc(&self, message: PhaseMessage) -> ConsensusRpc {
        ConsensusRpc::TwoPhase {
            term: self.current_term.max(1),
            sender_id: self.node_id.clone(),
            message: Box::new(message),
        }
    }
    fn validate_phase_block(&self, b: &PhaseBlock, view: u64) -> Result<(), &'static str> {
        if b.entry.index == 0
            || b.entry.term == 0
            || b.entry.term > view
            || view >= MAX_SAFE_TERM - 1
            || bincode::serialized_size(&b.entry.command).map_err(|_| "Command encoding")?
                > MAX_PHASE_COMMAND_BYTES
        {
            return Err("Invalid phase block bounds");
        }
        self.verify_operator_command(&b.entry.command)
    }
    pub(super) fn verify_phase_certificate(
        &self,
        c: &PhaseCertificate,
    ) -> Result<(), &'static str> {
        self.validate_phase_block(&c.block, c.view)?;
        if c.votes.len() < self.quorum_size() || c.votes.len() > self.cluster_size() {
            return Err("Phase certificate quorum");
        }
        let p = phase_vote_payload(
            &self.signing_context,
            c.phase,
            c.view,
            c.block.entry.index,
            &c.block.hash(),
        );
        for (id, sig) in &c.votes {
            let key = self.peer_public_keys.get(id).ok_or("Unknown phase voter")?;
            if !NodeCryptoIdentity::verify_signature(key, &p, sig) {
                return Err("Invalid phase signature");
            }
        }
        Ok(())
    }
    fn verify_view_report(&self, r: &ViewReport) -> Result<(), &'static str> {
        if r.view == 0
            || r.view >= MAX_SAFE_TERM - 1
            || !self.peer_public_keys.contains_key(&r.candidate)
        {
            return Err("Invalid report view/candidate");
        }
        for (cert, phase) in [(&r.prepared, Phase::Prepare), (&r.committed, Phase::Commit)] {
            if let Some(c) = cert {
                self.verify_phase_certificate(c)?;
                if c.phase != phase || c.view >= r.view {
                    return Err("Report must contain earlier-view evidence of the correct phase");
                }
            }
        }
        let key = self
            .peer_public_keys
            .get(&r.voter)
            .ok_or("Unknown report voter")?;
        if !NodeCryptoIdentity::verify_signature(
            key,
            &r.payload(&self.signing_context),
            &r.signature,
        ) {
            return Err("Invalid view report signature");
        }
        Ok(())
    }
    pub(super) fn select_new_view(&self, v: &NewView) -> Result<Option<PhaseBlock>, &'static str> {
        if v.reports.len() < self.quorum_size() || v.reports.len() > self.cluster_size() {
            return Err("New-view quorum");
        }
        let mut selected: Option<&PhaseCertificate> = None;
        for (id, r) in &v.reports {
            self.verify_view_report(r)?;
            if id != &r.voter || r.view != v.view || r.candidate != v.leader {
                return Err("New-view report binding");
            }
            for c in r.prepared.iter().chain(r.committed.iter()) {
                if let Some(old) = selected {
                    let a = (c.block.entry.index, c.view);
                    let b = (old.block.entry.index, old.view);
                    if a == b && c.block != old.block {
                        return Err("Contradictory certificates");
                    }
                    if a > b {
                        selected = Some(c);
                    }
                } else {
                    selected = Some(c);
                }
            }
        }
        Ok(selected.map(|c| c.block.clone()))
    }
    fn make_view_report(&mut self, candidate: &str) -> Result<ViewReport, &'static str> {
        if self.leader_excluded(candidate)
            || self.voted_for.as_deref().is_some_and(|id| id != candidate)
        {
            return Err("View already promised to another candidate");
        }
        if let Some(report) = &self.two_phase.as_ref().unwrap().durable.report {
            if report.view == self.current_term {
                if report.candidate != candidate {
                    return Err("Frozen report belongs to another candidate");
                }
                return Ok(report.clone());
            }
        }
        let d = &self.two_phase.as_ref().unwrap().durable;
        let mut r = ViewReport {
            view: self.current_term,
            candidate: candidate.into(),
            voter: self.node_id.clone(),
            prepared: d.prepared.clone(),
            committed: d.commits.values().next_back().cloned(),
            signature: vec![],
        };
        if r.prepared
            .iter()
            .chain(r.committed.iter())
            .any(|c| c.view >= r.view)
        {
            return Err("Cannot report after voting in this view");
        }
        self.voted_for = Some(candidate.into());
        self.role = Role::Follower;
        self.leader_id = None;
        r.signature = self
            .identity
            .sign_progress_payload(&r.payload(&self.signing_context));
        self.two_phase.as_mut().unwrap().durable.report = Some(r.clone());
        // current_term is the durable old-view signing fence.
        self.checkpoint()?;
        self.election_activity = self.election_activity.wrapping_add(1);
        Ok(r)
    }
    pub(super) fn start_phase_election(&mut self) -> Result<ConsensusRpc, &'static str> {
        let id = self.node_id.clone();
        let r = self.make_view_report(&id)?;
        self.role = Role::Candidate;
        let s = self.two_phase.as_mut().unwrap();
        s.reports.clear();
        s.reports.insert(id, r);
        self.finish_phase_election()?;
        Ok(self.phase_rpc(PhaseMessage::ViewRequest))
    }
    fn finish_phase_election(&mut self) -> Result<(), &'static str> {
        let reports = &self.two_phase.as_ref().unwrap().reports;
        if reports.len() < self.quorum_size() {
            return Ok(());
        }
        let view = NewView {
            view: self.current_term,
            leader: self.node_id.clone(),
            reports: reports.clone(),
        };
        self.select_new_view(&view)?;
        self.two_phase.as_mut().unwrap().durable.active = Some(view);
        self.become_leader();
        // Re-send commit certificates, one bounded entry at a time, to every peer.
        for next in self.next_index.values_mut() {
            *next = 1;
        }
        self.checkpoint()
    }
    fn with_phase_view(
        &mut self,
        v: &NewView,
        sender: &str,
        work: impl FnOnce(&mut Self, Option<PhaseBlock>) -> Result<Option<ConsensusRpc>, &'static str>,
    ) -> Result<Option<ConsensusRpc>, &'static str> {
        if v.view < self.current_term || v.leader != sender || self.leader_excluded(sender) {
            return Err("Stale or unauthorized view");
        }
        let selected = self.select_new_view(v)?;
        if let Some(b) = &selected {
            if b.entry.index >= self.log_base()
                && b.entry.index <= self.commit_index
                && self.entry_hash_at(b.entry.index) != Some(b.hash())
            {
                return Err("New-view conflicts with committed prefix");
            }
        }
        if v.view > self.current_term {
            self.step_down(v.view);
            self.election.certificate = None;
        }
        if sender != self.node_id {
            self.role = Role::Follower;
        }
        self.leader_id = Some(sender.into());
        self.two_phase.as_mut().unwrap().durable.active = Some(v.clone());
        // No response escapes between activating the view and voting. The
        // phase_vote checkpoint flushes both in one snapshot before signing.
        // Still persist the admitted view if work rejects the proposal or
        // returns a receipt without voting; a failed save always fails closed.
        let response = work(self, selected);
        self.checkpoint()?;
        response
    }
    fn check_phase_proposal(
        &mut self,
        block: &PhaseBlock,
        selected: Option<&PhaseBlock>,
        view: u64,
    ) -> Result<(), &'static str> {
        self.validate_phase_block(block, view)?;
        if block.entry.index > self.log_base() + crate::domain::CHECKPOINT_INTERVAL {
            return Err("Stable checkpoint required before advancing the log window");
        }
        if let MeshCommand::RetireExpired { through_unix_ns } = block.entry.command {
            let time = self
                .lease_time()
                .map_err(|_| "Clock bound required for retirement vote")?;
            if through_unix_ns == 0
                || through_unix_ns > time.unix_ns.saturating_sub(time.uncertainty_ns)
            {
                return Err("Retirement cutoff is not proven past by the local clock bound");
            }
        }
        self.snapshot_with_command(&block.entry)?;
        if block.entry.index != self.commit_index + 1
            || block.parent != self.entry_hash_at(self.commit_index).unwrap_or([0; 32])
        {
            return Err("Phase proposal needs its committed parent; catch up first");
        }
        if let Some(s) = selected {
            if block.entry.index < s.entry.index
                || (block.entry.index == s.entry.index && block != s)
                || (block.entry.index > s.entry.index
                    && s.entry.index >= self.log_base()
                    && self.entry_hash_at(s.entry.index) != Some(s.hash()))
            {
                return Err("Proposal violates new-view selection");
            }
        }
        Ok(())
    }
    fn phase_vote(
        &mut self,
        phase: Phase,
        block: &PhaseBlock,
    ) -> Result<ConsensusRpc, &'static str> {
        let key = (phase, self.current_term, block.entry.index);
        let hash = block.hash();
        let s = self.two_phase.as_mut().unwrap();
        if s.durable.votes.get(&key).is_some_and(|old| *old != hash) {
            return Err("Conflicting vote in the same phase/view/height");
        }
        s.durable.votes.insert(key, hash);
        self.truncate_log(self.commit_index as usize);
        self.push_log_entry(block.entry.clone());
        self.checkpoint()?;
        let signature = self.identity.sign_progress_payload(&phase_vote_payload(
            &self.signing_context,
            phase,
            self.current_term,
            block.entry.index,
            &hash,
        ));
        Ok(self.phase_rpc(PhaseMessage::Vote {
            phase,
            index: block.entry.index,
            hash,
            signature,
        }))
    }
    pub(super) fn propose_phase(&mut self, entry: LogEntry) -> Result<LogEntry, &'static str> {
        let view = self
            .two_phase
            .as_ref()
            .unwrap()
            .durable
            .active
            .clone()
            .ok_or("Leader lacks new-view proof")?;
        if view.view != self.current_term || self.last_log_index() != self.commit_index {
            return Err("One pending height at a time; wait for commit or view change");
        }
        let selected = self.select_new_view(&view)?;
        let b = PhaseBlock {
            entry: entry.clone(),
            parent: self.entry_hash_at(self.commit_index).unwrap_or([0; 32]),
        };
        self.check_phase_proposal(&b, selected.as_ref(), self.current_term)?;
        let vote = self.phase_vote(Phase::Prepare, &b)?;
        self.collect_own_phase_vote(vote)?;
        Ok(entry)
    }
    fn collect_own_phase_vote(&mut self, vote: ConsensusRpc) -> Result<(), &'static str> {
        let ConsensusRpc::TwoPhase { message, .. } = vote else {
            unreachable!()
        };
        let PhaseMessage::Vote {
            phase,
            index,
            hash,
            signature,
        } = *message
        else {
            unreachable!()
        };
        self.collect_phase_vote(&self.node_id.clone(), phase, index, hash, signature)
    }
    fn collect_phase_vote(
        &mut self,
        sender: &str,
        phase: Phase,
        index: u64,
        hash: [u8; 32],
        sig: Vec<u8>,
    ) -> Result<(), &'static str> {
        if self.role != Role::Leader
            || index != self.commit_index + 1
            || self.entry_hash_at(index) != Some(hash)
        {
            return Err("Vote does not match pending proposal");
        }
        let p = phase_vote_payload(
            &self.signing_context,
            phase,
            self.current_term,
            index,
            &hash,
        );
        if !self
            .peer_public_keys
            .get(sender)
            .is_some_and(|key| NodeCryptoIdentity::verify_signature(key, &p, &sig))
        {
            return Err("Invalid phase vote");
        }
        let b = PhaseBlock {
            entry: self
                .entry_at(index)
                .ok_or("Missing retained entry")?
                .clone(),
            parent: self.entry_hash_at(index - 1).unwrap_or([0; 32]),
        };
        if phase == Phase::Commit
            && !self
                .two_phase
                .as_ref()
                .unwrap()
                .durable
                .prepared
                .as_ref()
                .is_some_and(|c| c.view == self.current_term && c.block == b)
        {
            return Err("Commit vote without local prepare proof");
        }
        let quorum = self.quorum_size();
        let s = self.two_phase.as_mut().unwrap();
        // A verified late prepare vote adds no safety evidence after this
        // block's prepare quorum and our commit fence are already durable.
        // Keep the original certificate stable instead of rewriting it for
        // every larger quorum (and forcing another sync on every follower).
        if phase == Phase::Prepare
            && s.durable
                .prepared
                .as_ref()
                .is_some_and(|c| c.view == self.current_term && c.block == b)
            && s.durable
                .votes
                .get(&(Phase::Commit, self.current_term, index))
                == Some(&hash)
        {
            return Ok(());
        }
        let target = (self.current_term, index, hash);
        if s.vote_target != Some(target) {
            s.votes.clear();
            s.vote_target = Some(target);
        }
        let votes = s.votes.entry(phase).or_default();
        votes.insert(sender.into(), sig);
        if votes.len() < quorum {
            return Ok(());
        }
        let cert = PhaseCertificate {
            phase,
            view: self.current_term,
            block: b.clone(),
            votes: votes.clone(),
        };
        if phase == Phase::Prepare {
            s.durable.prepared = Some(cert);
            let vote = self.phase_vote(Phase::Commit, &b)?; // prepared proof is flushed with the commit vote fence
            self.collect_own_phase_vote(vote)
        } else {
            self.install_phase_commit(cert)
        }
    }
    pub(super) fn install_phase_commit(
        &mut self,
        cert: PhaseCertificate,
    ) -> Result<(), &'static str> {
        self.verify_phase_certificate(&cert)?;
        if cert.phase != Phase::Commit {
            return Err("Prepare is not commit");
        }
        let index = cert.block.entry.index;
        if index <= self.commit_index {
            if self.entry_hash_at(index) != Some(cert.block.hash()) {
                return Err("Conflicting committed prefix");
            }
            self.apply_committed_entries();
            return self.checkpoint();
        }
        if index > self.log_base() + crate::domain::CHECKPOINT_INTERVAL {
            return Err("Checkpoint required before further commits");
        }
        if index != self.commit_index + 1
            || cert.block.parent != self.entry_hash_at(self.commit_index).unwrap_or([0; 32])
        {
            return Err("Commit needs sequential certified catch-up");
        }
        self.snapshot_with_command(&cert.block.entry)?;
        if cert.view > self.current_term {
            self.step_down(cert.view);
            self.election.certificate = None;
        }
        self.truncate_log(self.commit_index as usize);
        self.push_log_entry(cert.block.entry.clone());
        self.two_phase
            .as_mut()
            .unwrap()
            .durable
            .commits
            .insert(index, cert);
        self.commit_index = index;
        self.checkpoint()?;
        self.apply_committed_entries();
        self.checkpoint()
    }
    pub(super) fn handle_phase_rpc(
        &mut self,
        term: u64,
        sender: NodeId,
        message: PhaseMessage,
    ) -> Result<Option<ConsensusRpc>, &'static str> {
        if self.two_phase.is_none() {
            return Err("Two-phase RPC on legacy state");
        }
        match message {
            PhaseMessage::CheckpointVote(vote) => self.on_checkpoint_vote(vote).map(Some),
            PhaseMessage::Snapshot(snapshot) => {
                self.install_snapshot(*snapshot)?;
                Ok(Some(self.phase_rpc(PhaseMessage::Receipt {
                    index: self.commit_index,
                    hash: self.entry_hash_at(self.commit_index).unwrap(),
                })))
            }
            PhaseMessage::ViewRequest => {
                if term != self.current_term {
                    return Err("View request requires admitted term");
                }
                let report = self.make_view_report(&sender)?;
                Ok(Some(
                    self.phase_rpc(PhaseMessage::ViewReport(Box::new(report))),
                ))
            }
            PhaseMessage::ViewReport(r) => {
                if !matches!(self.role, Role::Candidate | Role::Leader)
                    || term != self.current_term
                    || r.view != term
                    || r.candidate != self.node_id
                    || r.voter != sender
                {
                    return Err("Unexpected report");
                }
                self.verify_view_report(&r)?;
                if self.role == Role::Leader {
                    return Ok(None);
                }
                self.two_phase
                    .as_mut()
                    .unwrap()
                    .reports
                    .entry(sender)
                    .or_insert(*r);
                self.finish_phase_election()?;
                Ok(None)
            }
            PhaseMessage::Proposal { view, block } => {
                if term != view.view {
                    return Err("Proposal view mismatch");
                }
                self.with_phase_view(&view, &sender, |st, selected| {
                    if block.entry.index <= st.commit_index {
                        return Ok(Some(st.phase_rpc(PhaseMessage::Receipt {
                            index: st.commit_index,
                            hash: st.entry_hash_at(st.commit_index).unwrap(),
                        })));
                    }
                    st.check_phase_proposal(&block, selected.as_ref(), term)?;
                    // A quorum-prepared value must remain completable after its
                    // admission window closes; its BAN expiry is never renewed.
                    if selected.as_ref() != Some(&block) {
                        st.check_operator_vote_time(&block.entry.command)?;
                    }
                    st.phase_vote(Phase::Prepare, &block).map(Some)
                })
            }
            PhaseMessage::Prepared { view, certificate } => {
                self.verify_phase_certificate(&certificate)?;
                if term != view.view
                    || certificate.view != term
                    || certificate.phase != Phase::Prepare
                {
                    return Err("Wrong prepare phase/view");
                }
                self.with_phase_view(&view, &sender, |st, selected| {
                    if certificate.block.entry.index <= st.commit_index {
                        return Ok(Some(st.phase_rpc(PhaseMessage::Receipt {
                            index: st.commit_index,
                            hash: st.entry_hash_at(st.commit_index).unwrap(),
                        })));
                    }
                    st.check_phase_proposal(&certificate.block, selected.as_ref(), term)?;
                    let b = certificate.block.clone();
                    st.two_phase.as_mut().unwrap().durable.prepared = Some(certificate);
                    st.phase_vote(Phase::Commit, &b).map(Some)
                })
            }
            PhaseMessage::Vote {
                phase,
                index,
                hash,
                signature,
            } => {
                if term != self.current_term {
                    return Err("Stale phase vote");
                }
                if index <= self.commit_index && self.entry_hash_at(index) == Some(hash) {
                    return Ok(None);
                }
                self.collect_phase_vote(&sender, phase, index, hash, signature)?;
                Ok(None)
            }
            PhaseMessage::Commit(c) => {
                self.install_phase_commit(c)?;
                Ok(Some(self.phase_rpc(PhaseMessage::Receipt {
                    index: self.commit_index,
                    hash: self.entry_hash_at(self.commit_index).unwrap(),
                })))
            }
            PhaseMessage::Sync { index } if index <= self.log_base() => Ok(self.snapshot_rpc()),
            PhaseMessage::Sync { index } => Ok(self
                .two_phase
                .as_ref()
                .unwrap()
                .durable
                .commits
                .get(&index)
                .map(|c| self.phase_rpc(PhaseMessage::Commit(c.clone())))),
            PhaseMessage::Receipt { index, hash } => {
                if self.role == Role::Leader
                    && index <= self.commit_index
                    && self.entry_hash_at(index) == Some(hash)
                {
                    self.next_index.insert(sender.clone(), index + 1);
                    self.match_index.insert(sender, index);
                }
                Ok(None)
            }
        }
    }
    pub(super) fn phase_append(&self, peer: &str) -> Option<ConsensusRpc> {
        if self.role != Role::Leader {
            return None;
        }
        let s = self.two_phase.as_ref()?;
        let next = *self.next_index.get(peer).unwrap_or(&1);
        let message = if next <= self.log_base() {
            return self.snapshot_rpc();
        } else if let Some(c) = s.durable.commits.get(&next) {
            PhaseMessage::Commit(c.clone())
        } else if self.last_log_index() > self.commit_index {
            let view = s.durable.active.clone()?;
            let block = PhaseBlock {
                entry: self.log.last()?.clone(),
                parent: self.entry_hash_at(self.commit_index).unwrap_or([0; 32]),
            };
            if let Some(c) = s
                .durable
                .prepared
                .as_ref()
                .filter(|c| c.view == self.current_term && c.block == block)
            {
                PhaseMessage::Prepared {
                    view,
                    certificate: c.clone(),
                }
            } else {
                PhaseMessage::Proposal { view, block }
            }
        } else {
            // A commit certificate proves a value, not the current sender's
            // leadership. Recovered followers also need the new-view proof to
            // discover whom to challenge for fresh liveness responses.
            PhaseMessage::Proposal {
                view: s.durable.active.clone()?,
                block: s.durable.commits.values().next_back()?.block.clone(),
            }
        };
        let rpc = self.phase_rpc(message);
        self.rpc_fits_wire(&rpc).then_some(rpc)
    }
    /// Recover the selected value unchanged, then initialize this leader with a NoOp.
    /// Missing parents are fetched with commit proofs, never from unproven log hints.
    pub fn drive_two_phase(&mut self) -> Result<Option<SignedConsensusRpc>, &'static str> {
        if self.two_phase.is_none() {
            return Ok(None);
        }
        self.checkpoint()?;
        // Retrying an already certified application is independent of new votes.
        // A follower with no new traffic must also recover from backend failure.
        self.apply_committed_entries();
        self.checkpoint()?;
        if self.role == Role::Candidate
            && !self.leader_excluded(&self.node_id)
            && self.voted_for.as_deref() == Some(self.node_id.as_str())
            && self
                .two_phase
                .as_ref()
                .unwrap()
                .durable
                .report
                .as_ref()
                .is_some_and(|r| r.view == self.current_term && r.candidate == self.node_id)
        {
            // A delivered request can lose its reply after the peer has durably
            // promised this candidate. Retry that same view; never reset the
            // report set, advance the term, or replace the frozen vote/report.
            return self
                .sign_rpc_durable(self.phase_rpc(PhaseMessage::ViewRequest))
                .map(Some);
        }
        if self.role != Role::Leader {
            return Ok(None);
        }
        let v = self
            .two_phase
            .as_ref()
            .unwrap()
            .durable
            .active
            .clone()
            .ok_or("Missing active view")?;
        let selected = self.select_new_view(&v)?;
        // A bare approval that expired before preparing cannot collect new
        // votes. Abdicate rather than rewrite its same-view vote fence. A new
        // view either carries any hidden prepare QC or safely replaces it.
        if let Some(entry) = self.log.last().filter(|e| e.index > self.commit_index) {
            if let MeshCommand::AuthorizedPolicy(a) = &entry.command {
                let deadline = a.valid_until_unix_ns;
                let prepared = self
                    .two_phase
                    .as_ref()
                    .unwrap()
                    .durable
                    .prepared
                    .as_ref()
                    .is_some_and(|c| c.block.entry == *entry);
                if !prepared
                    && !selected.as_ref().is_some_and(|b| b.entry == *entry)
                    && self
                        .lease_time()
                        .ok()
                        .and_then(|t| t.upper_unix_ns().ok())
                        .is_some_and(|now| now >= deadline)
                {
                    self.role = Role::Follower;
                    self.leader_id = None;
                    return Ok(None);
                }
            }
        }
        if let Some(b) = selected.filter(|b| b.entry.index > self.commit_index) {
            if b.entry.index > self.commit_index + 1 {
                return self
                    .sign_rpc_durable(self.phase_rpc(PhaseMessage::Sync {
                        index: self.commit_index + 1,
                    }))
                    .map(Some);
            }
            // Discard only an unprepared minority proposal selected against by the new-view proof.
            if self.last_log_index() == self.commit_index
                || self.entry_hash_at(b.entry.index) != Some(b.hash())
            {
                self.truncate_log(self.commit_index as usize);
            }
            if self.two_phase.as_ref().unwrap().vote_target
                != Some((self.current_term, b.entry.index, b.hash()))
            {
                self.check_phase_proposal(&b, Some(&b), self.current_term)?;
                let vote = self.phase_vote(Phase::Prepare, &b)?;
                self.collect_own_phase_vote(vote)?;
            }
        } else if self.two_phase.as_ref().unwrap().initialized_view != Some(self.current_term) {
            if self.commit_index >= self.log_base() + crate::domain::CHECKPOINT_INTERVAL {
                return Ok(None);
            }
            // A new-view quorum without a prepared value authorizes replacing a
            // bare, uncommitted proposal, but never deleting its vote history.
            if !self
                .two_phase
                .as_ref()
                .unwrap()
                .durable
                .votes
                .contains_key(&(Phase::Prepare, self.current_term, self.commit_index + 1))
            {
                self.truncate_log(self.commit_index as usize);
                self.propose(MeshCommand::NoOp)?;
            }
        }
        self.two_phase.as_mut().unwrap().initialized_view = Some(self.current_term);
        self.retire_expired_if_idle()?;
        Ok(None)
    }
    pub(super) fn verify_phase_wal(
        &self,
        payload: &WalPayload,
        d: &Durable,
    ) -> Result<(), &'static str> {
        let base = d.stable.as_ref().map_or(0, |s| s.state.height);
        if let Some(stable) = &d.stable {
            self.verify_snapshot(stable)?;
        }
        if payload.context.protocol_version != 6
            || payload.commit_index > base + payload.log.len() as u64
            || base + payload.log.len() as u64 > payload.commit_index + 1
            || payload.commit_index < base
            || payload.log.len() as u64 > crate::domain::CHECKPOINT_INTERVAL
            || !payload.commit_qcs.is_empty()
            || !payload.voted_entry_hashes.is_empty()
        {
            return Err("Invalid two-phase WAL shape or legacy evidence");
        }
        let mut revision = d
            .stable
            .as_ref()
            .map_or(0, |s| s.state.authorization_revision);
        let mut parent = d.stable.as_ref().map_or([0; 32], |s| s.state.hash);
        for (i, entry) in payload.log.iter().enumerate() {
            if entry.index != base + i as u64 + 1 {
                return Err("WAL index gap");
            }
            let b = PhaseBlock {
                entry: entry.clone(),
                parent,
            };
            self.validate_phase_block(&b, payload.current_term)?;
            if let MeshCommand::AuthorizedPolicy(a) = &entry.command {
                if revision.checked_add(1) != Some(a.revision) {
                    return Err("WAL operator revision gap/replay");
                }
                revision = a.revision;
            }
            if entry.index <= payload.commit_index {
                let c = d
                    .commits
                    .get(&entry.index)
                    .ok_or("WAL missing commit proof")?;
                self.verify_phase_certificate(c)?;
                if c.phase != Phase::Commit || c.block != b || c.view > payload.current_term {
                    return Err("WAL commit mismatch");
                }
            }
            parent = b.hash();
        }
        if d.commits.len() as u64 != payload.commit_index - base + u64::from(base != 0) {
            return Err("WAL extra commit evidence");
        }
        self.verify_checkpoint_wal(payload, d)?;
        if let Some(c) = &d.prepared {
            self.verify_phase_certificate(c)?;
            if c.phase != Phase::Prepare
                || c.view > payload.current_term
                || c.block.entry.index > payload.commit_index + 1
            {
                return Err("Invalid WAL prepare");
            }
        }
        if let Some(r) = &d.report {
            self.verify_view_report(r)?;
            if r.voter != self.node_id || r.view > payload.current_term {
                return Err("Invalid WAL report");
            }
        }
        if let Some(v) = &d.active {
            self.select_new_view(v)?;
            if v.view > payload.current_term {
                return Err("Invalid WAL active view");
            }
        }
        for (_, view, index) in d.votes.keys() {
            if *view == 0
                || *view > payload.current_term
                || *index <= base
                || *index > payload.commit_index + 1
            {
                return Err("Invalid WAL vote fence");
            }
        }
        Ok(())
    }
}
