use crate::domain::entities::{LogEntry, MeshCommand};
use std::collections::BTreeSet;
use std::net::Ipv4Addr;

/// `u64::MAX` による Term 飽和（インクリメント不能・DoS）を防ぐための安全な Term 上限値
pub const MAX_SAFE_TERM: u64 = u64::MAX / 2;

/// 1回の `AppendEntries` RPC に含められる最大ログエントリ数（メモリ枯渇 DoS 防止）
pub const MAX_LOG_ENTRIES_PER_RPC: usize = 1024;

/// 1回の `SetPeerTrustBatch` コマンドに含められる最大ルール数（50ms マイクロバッチ上限）
pub const MAX_BATCH_RULES_PER_COMMAND: usize = 128;

/// 許容される最大 TTL 秒数（24時間 = 86,400秒）
pub const MAX_POLICY_TTL_SECS: u32 = 86_400;

/// QUIC ストリーム上で許容する最大メッセージサイズ（64 KiB）
pub const MAX_WIRE_MESSAGE_BYTES: usize = 64 * 1024;

/// ドメイン層の安全方針（Guardrail Policy）：
/// 誤検知や悪意ある提案によって「自分自身（ループバック）や保護対象 IP（ゲートウェイ・管理用 IP・クラスタ内ノード）」を
/// 誤って `XDP_DROP` で遮断してしまう（自己窒息 / 自殺攻撃）ことを防ぐドメインルール。
#[derive(Debug, Clone)]
pub struct SecurityPolicyGuard {
    /// 遮断禁止（保護対象）の IPv4 アドレス集合 (`u32` host-order)
    pub protected_ips: BTreeSet<u32>,
}

impl Default for SecurityPolicyGuard {
    fn default() -> Self {
        let mut protected_ips = BTreeSet::new();
        // ループバック (127.0.0.1) および未指定 (0.0.0.0) は常に遮断禁止
        protected_ips.insert(u32::from(Ipv4Addr::LOCALHOST));
        protected_ips.insert(u32::from(Ipv4Addr::UNSPECIFIED));
        Self { protected_ips }
    }
}

impl SecurityPolicyGuard {
    pub fn add_protected_ip(&mut self, ip: Ipv4Addr) {
        self.protected_ips.insert(u32::from(ip));
    }

    /// 提案されたコマンドがドメインの安全方針に違反していないか検証する
    pub fn validate_command(&self, cmd: &MeshCommand) -> Result<(), &'static str> {
        match cmd {
            MeshCommand::SetPeerTrust {
                peer_ip,
                trusted: false,
            } if self.protected_ips.contains(peer_ip) => {
                return Err("Policy Violation: Cannot block a protected/loopback IP address");
            }
            MeshCommand::SetPeerTrustBatch { rules } => {
                if rules.is_empty() || rules.len() > MAX_BATCH_RULES_PER_COMMAND {
                    return Err("Policy Violation: Batch rules count out of bounds (1..=128)");
                }
                for rule in rules {
                    if !rule.trusted && self.protected_ips.contains(&rule.peer_ip) {
                        return Err(
                            "Policy Violation: Batch contains a protected/loopback IP address",
                        );
                    }
                    if rule.ttl_secs > MAX_POLICY_TTL_SECS {
                        return Err("Policy Violation: Rule TTL exceeds MAX_POLICY_TTL_SECS");
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// `AppendEntries` で受信したエントリ列の境界値（サイズ、連番 Index >= 1、単調非減少 Term 範囲）と方針を検証する
    pub fn validate_entries(
        &self,
        prev_log_index: u64,
        prev_log_term: u64,
        rpc_term: u64,
        entries: &[LogEntry],
    ) -> Result<(), &'static str> {
        if entries.len() > MAX_LOG_ENTRIES_PER_RPC {
            return Err("Entries exceed MAX_LOG_ENTRIES_PER_RPC");
        }
        let mut min_term = prev_log_term;
        for (offset, entry) in entries.iter().enumerate() {
            let expected_idx = match prev_log_index.checked_add((offset as u64) + 1) {
                Some(idx) if idx >= 1 => idx,
                _ => return Err("Entry index overflow or zero index"),
            };
            if entry.index != expected_idx
                || entry.term == 0
                || entry.term < min_term
                || entry.term > rpc_term
                || entry.term >= MAX_SAFE_TERM
            {
                return Err("Entry index discontinuity or invalid term bounds");
            }
            min_term = entry.term;
            self.validate_command(&entry.command)?;
        }
        Ok(())
    }
}
