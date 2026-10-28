//! A single outstanding height, phase-separated votes, and signed new-view evidence.
use super::{LogEntry, NodeId, SigningContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_PHASE_COMMAND_BYTES: u64 = 1024;
pub const MAX_PHASE_MEMBERS: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    Prepare,
    Commit,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseBlock {
    // Birth term is immutable, including during re-proposal in a later view.
    pub entry: LogEntry,
    pub parent: [u8; 32],
}
impl PhaseBlock {
    pub fn hash(&self) -> [u8; 32] {
        self.entry.digest_with_parent(&self.parent)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseCertificate {
    pub phase: Phase,
    pub view: u64,
    pub block: PhaseBlock,
    pub votes: BTreeMap<NodeId, Vec<u8>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewReport {
    pub view: u64,
    pub candidate: NodeId,
    pub voter: NodeId,
    pub prepared: Option<PhaseCertificate>,
    pub committed: Option<PhaseCertificate>,
    pub signature: Vec<u8>,
}
impl ViewReport {
    pub fn payload(&self, context: &SigningContext) -> Vec<u8> {
        bincode::serialize(&(
            "VEIL_VIEW_REPORT_V1",
            context,
            self.view,
            &self.candidate,
            &self.voter,
            &self.prepared,
            &self.committed,
        ))
        .expect("report payload")
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewView {
    pub view: u64,
    pub leader: NodeId,
    pub reports: BTreeMap<NodeId, ViewReport>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhaseMessage {
    ViewRequest,
    CheckpointVote(super::CheckpointVote),
    Snapshot(Box<super::CertifiedSnapshot>),
    ViewReport(Box<ViewReport>),
    Proposal {
        view: NewView,
        block: PhaseBlock,
    },
    Prepared {
        view: NewView,
        certificate: PhaseCertificate,
    },
    Vote {
        phase: Phase,
        index: u64,
        hash: [u8; 32],
        signature: Vec<u8>,
    },
    Commit(PhaseCertificate),
    Sync {
        index: u64,
    },
    Receipt {
        index: u64,
        hash: [u8; 32],
    },
}
pub fn phase_vote_payload(
    context: &SigningContext,
    phase: Phase,
    view: u64,
    index: u64,
    hash: &[u8; 32],
) -> Vec<u8> {
    bincode::serialize(&("VEIL_PHASE_V1", context, phase, view, index, hash))
        .expect("phase payload")
}
