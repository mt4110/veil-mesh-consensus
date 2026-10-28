use crate::domain::SignedConsensusRpc;
use serde::{Deserialize, Serialize};

/// 【Interface Adapter】: QUIC ストリーム上でやり取りするメッセージフレーム（すべての Consensus RPC は Ed25519 署名付き `SignedConsensusRpc` を必須とする）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireMessage {
    Ping {
        from_node: String,
        epoch: u32,
    },
    Ack {
        from_node: String,
        epoch: u32,
        trusted: bool,
    },
    Consensus(Box<SignedConsensusRpc>),
}
