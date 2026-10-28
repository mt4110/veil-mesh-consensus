use super::*;
use crate::domain::{term_support_payload, TermCertificate};
use bincode::Options;
use ring::rand::{SecureRandom, SystemRandom};
use std::time::{Duration, Instant};

const CONTACT_WINDOW: Duration = Duration::from_millis(1800);
const PROBE_WINDOW: Duration = Duration::from_millis(800);

#[derive(Default)]
pub(super) struct ElectionState {
    pub(super) certificate: Option<TermCertificate>,
    pub(super) prepared_certificate: Option<TermCertificate>,
    pending_term: Option<u64>,
    // One response per configured voter bounds attacker-controlled storage.
    support: BTreeMap<NodeId, (u64, Vec<u8>)>,
    pub(super) probe: Option<(NodeId, u64, [u8; 32], Instant)>,
    pub(super) last_fresh_contact: Option<Instant>,
}

impl ConsensusState {
    pub(super) fn verify_term_certificate(
        &self,
        cert: &TermCertificate,
    ) -> Result<(), &'static str> {
        let term = cert.term();
        if term == 0 || term >= MAX_SAFE_TERM - 1 {
            return Err("Invalid term certificate bounds");
        }
        match cert {
            TermCertificate::Timeout(tc) => self.verify_timeout_certificate(tc),
            TermCertificate::Progress(pc) => self.verify_progress_certificate(pc),
            TermCertificate::PreVote { votes, .. } => {
                if votes.len() < self.quorum_size() || votes.len() > self.cluster_size() {
                    return Err("Term certificate lacks a fixed membership quorum");
                }
                let payload = term_support_payload(&self.signing_context, term);
                for (id, sig) in votes {
                    let pk = self.peer_public_keys.get(id).ok_or("Unknown term voter")?;
                    if !NodeCryptoIdentity::verify_signature(pk, &payload, sig) {
                        return Err("Invalid term support signature");
                    }
                }
                Ok(())
            }
        }
    }

    pub(super) fn attach_term_certificate(&self, rpc: ConsensusRpc) -> ConsensusRpc {
        let phase_request = matches!(&rpc, ConsensusRpc::TwoPhase { message, .. } if matches!(message.as_ref(), crate::domain::PhaseMessage::ViewRequest));
        if phase_request
            || matches!(
                rpc,
                ConsensusRpc::RequestVote { .. }
                    | ConsensusRpc::RequestVoteResponse { .. }
                    | ConsensusRpc::AppendEntries { .. }
                    | ConsensusRpc::AppendEntriesResponse { .. }
            )
        {
            if let Some(cert) = &self.election.certificate {
                if cert.term() == rpc.term() {
                    return ConsensusRpc::TermCertified {
                        term: rpc.term(),
                        sender_id: rpc.sender_id().into(),
                        certificate: cert.clone(),
                        rpc_bytes: bincode::serialize(&rpc).expect("certified RPC"),
                    };
                }
            }
        }
        rpc
    }

    pub(super) fn admit_rpc_term(
        &mut self,
        rpc: ConsensusRpc,
    ) -> Result<ConsensusRpc, &'static str> {
        let rpc = if let ConsensusRpc::TermCertified {
            term,
            sender_id,
            certificate,
            rpc_bytes,
        } = rpc
        {
            let rpc: ConsensusRpc = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(MAX_WIRE_MESSAGE_BYTES as u64)
                .reject_trailing_bytes()
                .deserialize(&rpc_bytes)
                .map_err(|_| "Invalid certified RPC encoding")?;
            // No nesting; the inner sender/term are covered by the outer RPC signature.
            let phase_request = matches!(&rpc, ConsensusRpc::TwoPhase { message, .. } if matches!(message.as_ref(), crate::domain::PhaseMessage::ViewRequest));
            if self.uses_two_phase() && !phase_request {
                return Err("Legacy certified RPC forbidden in two-phase mode");
            }
            if (!phase_request
                && !matches!(
                    rpc,
                    ConsensusRpc::RequestVote { .. }
                        | ConsensusRpc::RequestVoteResponse { .. }
                        | ConsensusRpc::AppendEntries { .. }
                        | ConsensusRpc::AppendEntriesResponse { .. }
                ))
                || certificate.term() != rpc.term()
                || term != rpc.term()
                || sender_id != rpc.sender_id()
            {
                return Err("Invalid certified RPC shape");
            }
            self.verify_term_certificate(&certificate)?;
            if let TermCertificate::Progress(pc) = &certificate {
                self.apply_progress_certificate(pc.clone());
            }
            if certificate.term() > self.current_term {
                self.step_down(certificate.term());
            }
            if certificate.term() == self.current_term {
                self.election.certificate = Some(certificate);
            }
            rpc
        } else {
            rpc
        };
        if rpc.term() > self.current_term {
            match &rpc {
                ConsensusRpc::TwoPhase {
                    term: 1, message, ..
                } if matches!(message.as_ref(), crate::domain::PhaseMessage::ViewRequest) => {
                    self.step_down(1);
                }
                ConsensusRpc::TwoPhase { message, .. }
                    if !matches!(message.as_ref(), crate::domain::PhaseMessage::ViewRequest) => {}
                // Genesis is the only implicit term admission. Later terms need a quorum.
                ConsensusRpc::RequestVote { term: 1, .. }
                | ConsensusRpc::AppendEntries { term: 1, .. } => {}
                ConsensusRpc::PreVote { .. }
                | ConsensusRpc::PreVoteResponse { .. }
                | ConsensusRpc::ProgressGossip { .. } => {}
                _ => return Err("Higher term requires quorum evidence"),
            }
        }
        Ok(rpc)
    }

    pub(super) fn can_start_election(&self) -> bool {
        !self.leader_excluded(&self.node_id)
            && self.current_term < MAX_SAFE_TERM - 2
            && (self.current_term == 0
                || self.cluster_size() == 1
                || self
                    .election
                    .prepared_certificate
                    .as_ref()
                    .is_some_and(|c| c.term() == self.current_term + 1))
    }

    /// Local timeout starts a non-mutating, non-exclusive pre-vote. No term or
    /// durable election vote changes until a quorum supports the next term.
    pub fn start_pre_vote(&mut self) -> Result<SignedConsensusRpc, &'static str> {
        self.checkpoint()?;
        if self.current_term >= MAX_SAFE_TERM - 2 {
            return Err("Election term exhausted");
        }
        let term = self.current_term + 1;
        self.election.pending_term = Some(term);
        self.election.support.clear();
        self.election.prepared_certificate = None;
        let sig = self.identity.sign_term_support(&self.signing_context, term);
        self.election
            .support
            .insert(self.node_id.clone(), (term, sig));
        self.sign_rpc_durable(ConsensusRpc::PreVote {
            term,
            candidate_id: self.node_id.clone(),
            last_log_index: self.last_log_index(),
            last_log_term: self.last_log_term(),
        })
    }

    pub(super) fn on_pre_vote(
        &self,
        term: u64,
        candidate: &str,
        last_index: u64,
        last_term: u64,
    ) -> Option<ConsensusRpc> {
        if self.leader_excluded(candidate) {
            return None;
        }
        // Relay durable evidence to a lagging requester. A single relay is
        // sufficient only because it carries independently verified quorum signatures.
        if !self.uses_two_phase()
            && term <= self.current_term
            && self.election.certificate.is_some()
        {
            return Some(ConsensusRpc::RequestVoteResponse {
                term: self.current_term,
                voter_id: self.node_id.clone(),
                vote_granted: false,
            });
        }
        // Legacy v4 recovery may lack evidence. Its current-term observations
        // need a quorum; one unproven high-term peer can never advance a receiver.
        let supported = if term <= self.current_term {
            self.current_term
        } else if term == self.current_term + 1
            && term < MAX_SAFE_TERM - 1
            && self.role != Role::Leader
            && self
                .election
                .last_fresh_contact
                .is_none_or(|t| t.elapsed() >= CONTACT_WINDOW)
            && (self.uses_two_phase()
                || (last_term, last_index) >= (self.last_log_term(), self.last_log_index()))
        {
            term
        } else {
            return None;
        };
        Some(ConsensusRpc::PreVoteResponse {
            term: supported,
            voter_id: self.node_id.clone(),
            support_signature: self
                .identity
                .sign_term_support(&self.signing_context, supported),
        })
    }

    pub(super) fn on_pre_vote_response(&mut self, term: u64, voter: NodeId, sig: Vec<u8>) {
        let Some(target) = self.election.pending_term else {
            return;
        };
        if term < target {
            return;
        }
        let Some(pk) = self.peer_public_keys.get(&voter) else {
            return;
        };
        if !NodeCryptoIdentity::verify_signature(
            pk,
            &term_support_payload(&self.signing_context, term),
            &sig,
        ) {
            return;
        }
        self.election.support.insert(voter, (term, sig));
        let votes: BTreeMap<_, _> = self
            .election
            .support
            .iter()
            .filter(|(_, (t, _))| *t == term)
            .map(|(id, (_, sig))| (id.clone(), sig.clone()))
            .collect();
        if votes.len() < self.quorum_size() {
            return;
        }
        let cert = TermCertificate::PreVote { term, votes };
        if self.verify_term_certificate(&cert).is_err() {
            return;
        }
        if term > target {
            // Catch up only with quorum evidence; never jump on one peer's hint.
            self.step_down(term);
            self.election.pending_term = None;
            self.election.certificate = Some(cert);
        } else {
            self.election.prepared_certificate = Some(cert);
        }
    }

    /// Called by the driver after pre-vote responses, once per local attempt.
    pub fn finish_pre_vote(&mut self) -> Result<Option<SignedConsensusRpc>, &'static str> {
        if self.election.pending_term != Some(self.current_term + 1) {
            return Ok(None);
        }
        let supported = self.cluster_size() == 1
            || self
                .election
                .prepared_certificate
                .as_ref()
                .is_some_and(|c| c.term() == self.current_term + 1);
        if !supported || self.leader_excluded(&self.node_id) {
            return Ok(None);
        }
        self.election.pending_term = None;
        self.start_signed_election().map(Some)
    }

    /// Follower-generated randomness, bounded lifetime and one-time consumption
    /// make stored heartbeat/ack signatures useless for extending a new timer.
    pub fn make_liveness_probe(
        &mut self,
    ) -> Result<Option<(NodeId, SignedConsensusRpc)>, &'static str> {
        if self.role == Role::Leader || self.current_term == 0 {
            return Ok(None);
        }
        let Some(leader) = self.leader_id.clone() else {
            return Ok(None);
        };
        let mut nonce = [0; 32];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| "Liveness random source failed")?;
        self.election.probe = Some((leader.clone(), self.current_term, nonce, Instant::now()));
        let rpc = ConsensusRpc::LivenessProbe {
            term: self.current_term,
            sender_id: self.node_id.clone(),
            nonce,
        };
        Ok(Some((leader, self.sign_rpc_durable(rpc)?)))
    }

    pub(super) fn on_liveness_ack(&mut self, term: u64, sender: NodeId, nonce: [u8; 32]) {
        let Some((expected, expected_term, expected_nonce, sent)) = &self.election.probe else {
            return;
        };
        if self.role == Role::Leader
            || self.current_term != term
            || *expected_term != term
            || sender != *expected
            || self.leader_id.as_ref() != Some(&sender)
            || self.leader_excluded(&sender)
            || nonce != *expected_nonce
            || sent.elapsed() >= PROBE_WINDOW
        {
            return;
        }
        self.election.probe = None;
        self.election.last_fresh_contact = Some(Instant::now());
        self.election_activity = self.election_activity.wrapping_add(1);
    }
}
