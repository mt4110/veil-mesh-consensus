//! Stable logical state, authenticated separately from prepare/commit votes.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CHECKPOINT_INTERVAL: u64 = 32;
pub const MAX_SNAPSHOT_RULES: usize = 256;
pub const MAX_SNAPSHOT_BYTES: u64 = 32 * 1024;
pub const DEFAULT_MAX_WAL_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRule {
    pub index: u64,
    pub ordinal: usize,
    pub issued_at: u64,
    pub not_after: u64,
    pub rule: PeerTrustRule,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySnapshot {
    pub authorization_revision: u64,
    pub height: u64,
    pub hash: [u8; 32],
    pub retired_through_unix_ns: u64,
    // Trusted commands remove a BAN from the logical state. Expiry alone does
    // not remove its tombstone: peers must hash the same deterministic state.
    pub rules: BTreeMap<u32, SnapshotRule>,
    pub lockdown: BTreeMap<NodeId, (u64, bool)>,
}
impl PolicySnapshot {
    pub fn digest(&self) -> [u8; 32] {
        ring::digest::digest(
            &ring::digest::SHA256,
            &bincode::serialize(self).expect("snapshot"),
        )
        .as_ref()
        .try_into()
        .expect("SHA256")
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointVote {
    pub height: u64,
    pub hash: [u8; 32],
    pub state_hash: [u8; 32],
    pub voter: NodeId,
    pub signature: Vec<u8>,
}
impl CheckpointVote {
    pub fn payload(&self, context: &SigningContext) -> Vec<u8> {
        bincode::serialize(&(
            "VEIL_CHECKPOINT_V1",
            context,
            self.height,
            self.hash,
            self.state_hash,
            &self.voter,
        ))
        .expect("checkpoint payload")
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertifiedSnapshot {
    pub state: PolicySnapshot,
    pub commit: PhaseCertificate,
    pub votes: BTreeMap<NodeId, CheckpointVote>,
}
