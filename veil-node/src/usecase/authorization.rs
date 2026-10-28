use super::*;
impl ConsensusState {
    pub fn approval_policy(&self) -> Option<&crate::domain::ApprovalPolicy> {
        self.policy_guard.approval.as_ref()
    }
    pub(super) fn verify_operator_command(
        &self,
        command: &MeshCommand,
    ) -> Result<(), &'static str> {
        self.policy_guard.validate_log_command(command)?;
        if let MeshCommand::AuthorizedPolicy(a) = command {
            let policy = self
                .policy_guard
                .approval
                .as_ref()
                .ok_or("Missing approval profile")?;
            if !NodeCryptoIdentity::verify_signature(
                &policy.public_key,
                &a.payload(&self.signing_context),
                &a.signature,
            ) {
                return Err("Invalid independent operator signature/context");
            }
        }
        Ok(())
    }
    pub(super) fn check_operator_vote_time(
        &mut self,
        command: &MeshCommand,
    ) -> Result<(), &'static str> {
        if let MeshCommand::AuthorizedPolicy(a) = command {
            let t = self
                .lease_time()
                .map_err(|_| "Clock bound required for operator approval")?;
            // The entire observation interval must be inside the approval window.
            if a.issued_at_unix_ns > t.unix_ns.saturating_sub(t.uncertainty_ns)
                || t.upper_unix_ns().map_err(|_| "Clock bound overflow")? >= a.valid_until_unix_ns
            {
                return Err(
                    "Operator approval is expired, future-dated, or inside clock uncertainty",
                );
            }
        }
        Ok(())
    }
    pub fn authorization_revision(&self) -> u64 {
        let base = self
            .two_phase
            .as_ref()
            .and_then(|s| s.durable.stable.as_ref())
            .map_or(0, |s| s.state.authorization_revision);
        self.log
            .iter()
            .take_while(|e| e.index <= self.commit_index)
            .filter_map(|e| match &e.command {
                MeshCommand::AuthorizedPolicy(a) => Some(a.revision),
                _ => None,
            })
            .last()
            .unwrap_or(base)
    }
}
