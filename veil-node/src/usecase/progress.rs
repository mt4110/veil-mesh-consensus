//! Request-specific progress. A quorum timeout admits a new term, never a log
//! value. Durable election votes, prefix locks and commit QCs remain authoritative.
use super::*;
use crate::domain::{
    progress_payload, BanRequest, ProgressCertificate, ProgressVote, TermCertificate,
};
use std::time::{Duration, Instant};

const PROGRESS_GRACE: Duration = Duration::from_secs(5);

#[derive(Default, Serialize, Deserialize)]
pub(super) struct ProgressDurable {
    // One outstanding request per origin; terminal receipts may be replaced.
    requests: BTreeMap<NodeId, BanRequest>,
    // One latest exclusion per leader; proofs also carry requests across restart.
    exclusions: BTreeMap<NodeId, ProgressCertificate>,
}
#[derive(Default)]
pub(super) struct ProgressState {
    pub(super) durable: ProgressDurable,
    watches: BTreeMap<[u8; 32], (u64, NodeId, Instant)>,
    votes: BTreeMap<[u8; 32], BTreeMap<NodeId, ProgressVote>>,
    cursor: usize,
}

impl ConsensusState {
    #[cfg(test)]
    pub(crate) fn certified_handoff_for(&self, id: &[u8; 32], term: u64, leader: &str) -> bool {
        self.progress
            .durable
            .exclusions
            .get(leader)
            .is_some_and(|cert| {
                cert.term == term
                    && cert.leader == leader
                    && cert.request.id(&self.signing_context) == *id
                    && self.verify_progress_certificate(cert).is_ok()
            })
    }

    fn verify_ban_request(&self, request: &BanRequest) -> Result<(), &'static str> {
        self.verify_operator_command(&request.command)?;
        if !matches!(&request.command, MeshCommand::AuthorizedPolicy(_))
            && !matches!(&request.command, MeshCommand::SetPeerTrustLeaseBatch { rules, .. }
            if rules.len() == 1 && !rules[0].rule.trusted && rules[0].rule.ttl_secs > 0)
        {
            return Err("Progress requests require one finite signed-deadline BAN");
        }
        let key = self
            .peer_public_keys
            .get(&request.origin)
            .ok_or("Unknown request origin")?;
        if !NodeCryptoIdentity::verify_signature(
            key,
            &request.payload(&self.signing_context),
            &request.signature,
        ) {
            return Err("Invalid BAN request signature/context");
        }
        Ok(())
    }

    fn request_live(&self, request: &BanRequest) -> bool {
        if matches!(&request.command, MeshCommand::AuthorizedPolicy(a) if a.revision <= self.authorization_revision())
            && !self.request_committed(request)
        {
            return false;
        }
        let Some((issued_at_unix_ns, rules)) = request.command.lease_parts() else {
            return false;
        };
        let sample = match &self.policy_clock {
            Some(clock) => clock.lease_time().ok(),
            None => SIMULATION_WALL_EPOCH_NS
                .checked_add(self.monotonic_now_ns)
                .map(|unix_ns| LeaseTime {
                    boot_ns: self.monotonic_now_ns,
                    unix_ns,
                    uncertainty_ns: 0,
                    max_slow_rate_ppm: 0,
                }),
        };
        sample
            .and_then(|s| {
                s.boot_deadline(
                    issued_at_unix_ns,
                    match &request.command {
                        MeshCommand::AuthorizedPolicy(a) => a.valid_until_unix_ns,
                        _ => rules[0].not_after_unix_ns,
                    },
                )
                .ok()
            })
            .is_some_and(|d| d != 1)
    }

    pub(super) fn remember_compacted_requests(&mut self, height: u64) {
        let requests: Vec<_> = self
            .progress
            .durable
            .requests
            .values()
            .chain(
                self.progress
                    .durable
                    .exclusions
                    .values()
                    .map(|c| &c.request),
            )
            .cloned()
            .collect();
        let ids: BTreeSet<_> = requests
            .iter()
            .map(|r| r.id(&self.signing_context))
            .collect();
        let mut receipts = self
            .two_phase
            .as_ref()
            .unwrap()
            .durable
            .request_receipts
            .clone();
        receipts.retain(|id, _| ids.contains(id));
        for r in requests {
            if let Some(e) = self
                .log
                .iter()
                .find(|e| e.index <= height && e.command == r.command)
            {
                receipts.insert(
                    r.id(&self.signing_context),
                    (e.index, self.entry_hash_at(e.index).unwrap()),
                );
            }
        }
        self.two_phase.as_mut().unwrap().durable.request_receipts = receipts;
    }
    fn request_committed(&self, request: &BanRequest) -> bool {
        if self.two_phase.as_ref().is_some_and(|s| {
            s.durable
                .request_receipts
                .contains_key(&request.id(&self.signing_context))
        }) {
            return true;
        }
        self.log
            .iter()
            .take_while(|e| e.index <= self.commit_index)
            .any(|e| e.command == request.command)
    }
    fn request_pending(&self, request: &BanRequest) -> bool {
        !self.request_committed(request) && self.request_live(request)
    }
    fn pending_requests(&self) -> BTreeMap<[u8; 32], BanRequest> {
        self.progress
            .durable
            .requests
            .values()
            .chain(
                self.progress
                    .durable
                    .exclusions
                    .values()
                    .map(|c| &c.request),
            )
            .filter(|r| self.request_pending(r))
            .map(|r| (r.id(&self.signing_context), r.clone()))
            .collect()
    }

    pub(super) fn leader_excluded(&self, leader: &str) -> bool {
        self.progress
            .durable
            .exclusions
            .get(leader)
            .is_some_and(|c| {
                // A lost clock bound must not release a persisted exclusion.
                !matches!(&c.request.command, MeshCommand::AuthorizedPolicy(a) if a.revision <= self.authorization_revision())
                    && !self.request_committed(&c.request)
                    && (self.request_live(&c.request)
                        || self
                            .policy_clock
                            .as_ref()
                            .is_some_and(|clock| clock.lease_time().is_err()))
            })
    }

    /// Durable receipt on any member. TTL starts here, never on a later leader.
    pub fn accept_ban_request(&mut self, ip: Ipv4Addr, ttl: u32) -> Result<[u8; 32], &'static str> {
        self.checkpoint()?;
        let command = MeshCommand::set_trust_with_ttl(ip, false, ttl);
        self.policy_guard.validate_command(&command)?;
        let command = self.stamp_deadlines(command)?;
        self.accept_policy_request(command)
    }

    pub fn accept_policy_request(
        &mut self,
        command: MeshCommand,
    ) -> Result<[u8; 32], &'static str> {
        self.checkpoint()?;
        self.verify_operator_command(&command)?;
        self.check_operator_vote_time(&command)?;
        if let MeshCommand::AuthorizedPolicy(a) = &command {
            if self.authorization_revision().checked_add(1) != Some(a.revision) {
                return Err("Approval revision is stale or not next");
            }
        }
        let mut request = BanRequest {
            origin: self.node_id.clone(),
            command,
            signature: vec![],
        };
        request.signature = self
            .identity
            .sign_progress_payload(&request.payload(&self.signing_context));
        self.verify_ban_request(&request)?;
        if !self.request_live(&request) {
            return Err("Request expired or clock unavailable");
        }
        let id = request.id(&self.signing_context);
        self.store_request(request)?;
        self.checkpoint()?;
        Ok(id)
    }

    fn store_request(&mut self, request: BanRequest) -> Result<(), &'static str> {
        self.verify_ban_request(&request)?;
        if !self.request_pending(&request) {
            return Ok(());
        }
        if let Some(old) = self.progress.durable.requests.get(&request.origin) {
            if old != &request && self.request_pending(old) {
                return Err("Request origin already has a pending BAN");
            }
        }
        self.progress
            .durable
            .requests
            .insert(request.origin.clone(), request);
        Ok(())
    }

    pub fn ban_request_status(&self, id: &[u8; 32]) -> String {
        let request = self
            .progress
            .durable
            .requests
            .values()
            .chain(
                self.progress
                    .durable
                    .exclusions
                    .values()
                    .map(|c| &c.request),
            )
            .find(|r| r.id(&self.signing_context) == *id);
        let Some(request) = request else {
            return "OK\nrequest_phase=unknown\n".into();
        };
        let index = self
            .log
            .iter()
            .find(|e| e.command == request.command)
            .map(|e| e.index)
            .or_else(|| {
                self.two_phase
                    .as_ref()
                    .and_then(|s| s.durable.request_receipts.get(id).map(|(index, _)| *index))
            });
        let phase = if !self.request_live(request) {
            "expired"
        } else if index.is_some_and(|i| self.last_applied >= i) {
            "applied"
        } else if self.request_committed(request) {
            "committed"
        } else if index.is_some() {
            "logged"
        } else {
            "queued"
        };
        let (_, rules) = request.command.lease_parts().expect("validated request");
        format!(
            "OK\nrequest_phase={phase}\nentry_index={}\nnot_after_unix_ns={}\n",
            index.unwrap_or(0),
            rules[0].not_after_unix_ns
        )
    }

    pub(super) fn verify_progress_certificate(
        &self,
        cert: &ProgressCertificate,
    ) -> Result<(), &'static str> {
        self.verify_ban_request(&cert.request)?;
        if cert.term == 0
            || cert.term >= MAX_SAFE_TERM - 2
            || !self.peer_public_keys.contains_key(&cert.leader)
            || cert.votes.len() < self.quorum_size()
            || cert.votes.len() > self.cluster_size()
        {
            return Err("Invalid progress certificate bounds/quorum");
        }
        let payload = progress_payload(
            &self.signing_context,
            cert.term,
            &cert.leader,
            &cert.request.id(&self.signing_context),
        );
        for (id, sig) in &cert.votes {
            let key = self
                .peer_public_keys
                .get(id)
                .ok_or("Unknown progress voter")?;
            if !NodeCryptoIdentity::verify_signature(key, &payload, sig) {
                return Err("Invalid progress signature/context");
            }
        }
        Ok(())
    }

    pub(super) fn remember_progress_certificate(&mut self, cert: ProgressCertificate) {
        // Called only after full certificate verification. Historical proofs may
        // advance a lagging term but never renew an expired request.
        let replace = self
            .progress
            .durable
            .exclusions
            .get(&cert.leader)
            .is_none_or(|old| {
                cert.term > old.term
                    && (!self.request_pending(&old.request) || self.request_pending(&cert.request))
            });
        if replace {
            self.progress
                .durable
                .exclusions
                .insert(cert.leader.clone(), cert);
        }
    }
    pub(super) fn apply_progress_certificate(&mut self, cert: ProgressCertificate) {
        self.remember_progress_certificate(cert.clone());
        if cert.term + 1 > self.current_term {
            self.step_down(cert.term + 1);
            self.election.certificate = Some(TermCertificate::Progress(cert));
        }
        if self
            .leader_id
            .as_deref()
            .is_some_and(|id| self.leader_excluded(id))
        {
            self.role = Role::Follower;
            self.leader_id = None;
            self.election.probe = None;
            self.election.last_fresh_contact = None;
        }
    }

    pub(super) fn receive_progress(
        &mut self,
        request: Option<BanRequest>,
        vote: Option<ProgressVote>,
        certificate: Option<ProgressCertificate>,
    ) -> Result<(), &'static str> {
        // Validate all detached evidence before mutation. Outer term is just a hint.
        if let Some(c) = &certificate {
            self.verify_progress_certificate(c)?;
        }
        if let Some(r) = &request {
            self.verify_ban_request(r)?;
        }
        if let Some(v) = &vote {
            let r = request.as_ref().ok_or("Progress vote missing request")?;
            let key = self
                .peer_public_keys
                .get(&v.voter)
                .ok_or("Unknown progress voter")?;
            if !self.peer_public_keys.contains_key(&v.leader)
                || v.term == 0
                || v.term >= MAX_SAFE_TERM - 2
                || !NodeCryptoIdentity::verify_signature(
                    key,
                    &progress_payload(
                        &self.signing_context,
                        v.term,
                        &v.leader,
                        &r.id(&self.signing_context),
                    ),
                    &v.signature,
                )
            {
                return Err("Invalid progress vote");
            }
        }
        if let Some(c) = certificate {
            self.apply_progress_certificate(c);
        }
        if let Some(r) = request {
            // A full local queue must not block verified term evidence. Only a
            // stored request can allocate vote storage; one slot per origin.
            if self.store_request(r.clone()).is_ok() && self.request_pending(&r) {
                if let Some(v) = vote {
                    if v.term == self.current_term && self.leader_id.as_ref() == Some(&v.leader) {
                        let id = r.id(&self.signing_context);
                        let votes = self.progress.votes.entry(id).or_default();
                        votes.retain(|_, old| old.term == v.term && old.leader == v.leader);
                        votes.insert(v.voter.clone(), v.clone());
                        if votes.len() >= self.quorum_size() {
                            let cert = ProgressCertificate {
                                term: v.term,
                                leader: v.leader,
                                request: r,
                                votes: self.progress.votes[&id]
                                    .iter()
                                    .map(|(id, v)| (id.clone(), v.signature.clone()))
                                    .collect(),
                            };
                            self.verify_progress_certificate(&cert)?;
                            self.apply_progress_certificate(cert);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Driver tick, independent of liveness and unrelated commit progress.
    pub fn drive_ban_progress(&mut self) -> Result<Option<SignedConsensusRpc>, &'static str> {
        self.checkpoint()?;
        #[cfg(test)]
        if self.censor_ban_requests && self.role == Role::Leader {
            return Ok(None);
        }
        let pending = self.pending_requests();
        self.progress
            .watches
            .retain(|id, _| pending.contains_key(id));
        self.progress.votes.retain(|id, _| pending.contains_key(id));
        for id in pending.keys() {
            if let Some(leader) = &self.leader_id {
                let w = self.progress.watches.entry(*id).or_insert((
                    self.current_term,
                    leader.clone(),
                    Instant::now(),
                ));
                if w.0 != self.current_term || &w.1 != leader {
                    *w = (self.current_term, leader.clone(), Instant::now());
                }
            }
        }
        if self.role == Role::Leader && !self.leader_excluded(&self.node_id) {
            for request in pending.values() {
                if !self.log.iter().any(|e| e.command == request.command) {
                    if let Err(error) = self.propose(request.command.clone()) {
                        if self.persistence_error.is_some() {
                            return Err(error);
                        }
                        tracing::warn!(%error, "Witnessed BAN could not be proposed; retaining original deadline");
                    }
                    break;
                }
            }
        }
        if self.current_term == 0 {
            return Ok(None);
        }
        let cursor = self.progress.cursor;
        self.progress.cursor = cursor.wrapping_add(1);
        let request = if pending.is_empty() {
            None
        } else {
            pending.values().nth(cursor % pending.len()).cloned()
        };
        let vote = request.as_ref().and_then(|r| {
            let id = r.id(&self.signing_context);
            let (term, leader, since) = self.progress.watches.get(&id)?;
            if *term != self.current_term
                || self.leader_id.as_ref() != Some(leader)
                || since.elapsed() < PROGRESS_GRACE
                || !self.request_pending(r)
            {
                return None;
            }
            Some(ProgressVote {
                term: *term,
                leader: leader.clone(),
                voter: self.node_id.clone(),
                signature: self.identity.sign_progress_payload(&progress_payload(
                    &self.signing_context,
                    *term,
                    leader,
                    &id,
                )),
            })
        });
        if vote.is_some() {
            self.receive_progress(request.clone(), vote.clone(), None)?;
        }
        let certs = &self.progress.durable.exclusions;
        let certificate = if certs.is_empty() {
            None
        } else {
            certs.values().nth(cursor % certs.len()).cloned()
        };
        if request.is_none() && certificate.is_none() {
            return Ok(None);
        }
        self.sign_rpc_durable(ConsensusRpc::ProgressGossip {
            term: self.current_term,
            sender_id: self.node_id.clone(),
            request,
            vote,
            certificate,
        })
        .map(Some)
    }

    pub(super) fn verify_progress_wal(
        &self,
        data: &ProgressDurable,
        current_term: u64,
    ) -> Result<(), &'static str> {
        if data.requests.len() > self.cluster_size() || data.exclusions.len() > self.cluster_size()
        {
            return Err("Progress WAL exceeds fixed membership bounds");
        }
        for (origin, request) in &data.requests {
            if origin != &request.origin {
                return Err("Progress WAL origin mismatch");
            }
            self.verify_ban_request(request)?;
        }
        for (leader, cert) in &data.exclusions {
            if cert.term >= current_term {
                return Err("Progress WAL certificate exceeds durable term");
            }
            if leader != &cert.leader {
                return Err("Progress WAL leader mismatch");
            }
            self.verify_progress_certificate(cert)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
