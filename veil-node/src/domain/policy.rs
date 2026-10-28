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
    pub approval: Option<super::ApprovalPolicy>,
    pub protected_ips: BTreeSet<u32>,
}

impl Default for SecurityPolicyGuard {
    fn default() -> Self {
        let mut protected_ips = BTreeSet::new();
        // ループバック (127.0.0.1) および未指定 (0.0.0.0) は常に遮断禁止
        protected_ips.insert(u32::from(Ipv4Addr::LOCALHOST));
        protected_ips.insert(u32::from(Ipv4Addr::UNSPECIFIED));
        Self {
            protected_ips,
            approval: None,
        }
    }
}

impl SecurityPolicyGuard {
    pub fn add_protected_ip(&mut self, ip: Ipv4Addr) {
        self.protected_ips.insert(u32::from(ip));
    }

    /// 提案されたコマンドがドメインの安全方針に違反していないか検証する
    pub fn validate_command(&self, cmd: &MeshCommand) -> Result<(), &'static str> {
        if self.approval.is_some()
            && !matches!(
                cmd,
                MeshCommand::AuthorizedPolicy(_)
                    | MeshCommand::NoOp
                    | MeshCommand::RetireExpired { .. }
            )
        {
            return Err("Independent operator approval required; raw policy/LSM commands disabled");
        }
        if let MeshCommand::AuthorizedPolicy(a) = cmd {
            if self.approval.is_none() {
                return Err("Approval profile not configured");
            }
            let mut shape_guard = self.clone();
            shape_guard.approval = None;
            shape_guard.validate_log_command(&MeshCommand::SetPeerTrustLeaseBatch {
                issued_at_unix_ns: a.issued_at_unix_ns,
                rules: vec![a.rule.clone()],
            })?;
            if a.revision == 0
                || a.signature.len() != 64
                || a.valid_until_unix_ns <= a.issued_at_unix_ns
                || a.valid_until_unix_ns - a.issued_at_unix_ns > 300_000_000_000
                || (a.rule.rule.trusted
                    && (a.rule.rule.ttl_secs != 0 || a.rule.not_after_unix_ns != 0))
                || (!a.rule.rule.trusted && a.valid_until_unix_ns > a.rule.not_after_unix_ns)
            {
                return Err("Invalid operator approval bounds");
            }
        }
        match cmd {
            MeshCommand::SetPeerTrust {
                peer_ip,
                trusted: false,
            } if self.protected_ips.contains(peer_ip) => {
                return Err("Policy Violation: Cannot block a protected/loopback IP address");
            }
            MeshCommand::SetPeerTrust { trusted: false, .. } => {
                return Err("Policy Violation: BAN requires a finite TTL and signed deadline");
            }
            MeshCommand::SetPeerTrustBatch { .. } | MeshCommand::SetPeerTrustLeaseBatch { .. } => {
                let count = cmd.batch_rules().count();
                if count == 0 || count > MAX_BATCH_RULES_PER_COMMAND {
                    return Err("Policy Violation: Batch rules count out of bounds (1..=128)");
                }
                for rule in cmd.batch_rules() {
                    let ip = Ipv4Addr::from(rule.peer_ip);
                    if !rule.trusted
                        && (ip.is_loopback()
                            || ip.is_unspecified()
                            || ip.is_multicast()
                            || ip.is_broadcast())
                    {
                        return Err("Policy Violation: Invalid or loopback BAN target");
                    }
                    if !rule.trusted && rule.ttl_secs == 0 {
                        return Err("Policy Violation: BAN TTL must be positive");
                    }
                    if !rule.trusted && self.protected_ips.contains(&rule.peer_ip) {
                        return Err(
                            "Policy Violation: Batch contains a protected/loopback IP address",
                        );
                    }
                    if rule.ttl_secs > MAX_POLICY_TTL_SECS {
                        return Err("Policy Violation: Rule TTL exceeds MAX_POLICY_TTL_SECS");
                    }
                }
                if let MeshCommand::SetPeerTrustLeaseBatch {
                    issued_at_unix_ns,
                    rules,
                } = cmd
                {
                    if *issued_at_unix_ns == 0 {
                        return Err("Policy Violation: Missing issuance timestamp");
                    }
                    for item in rules {
                        let expected = if item.rule.ttl_secs == 0 {
                            Some(0)
                        } else {
                            issued_at_unix_ns
                                .checked_add(u64::from(item.rule.ttl_secs) * 1_000_000_000)
                        };
                        if expected != Some(item.not_after_unix_ns) {
                            return Err("Policy Violation: Invalid absolute lease window");
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Replicated/persisted BANs must already carry their signed absolute expiry.
    /// Relative TTL is a local proposal input only; unban commands need no expiry.
    pub fn validate_log_command(&self, cmd: &MeshCommand) -> Result<(), &'static str> {
        self.validate_command(cmd)?;
        if matches!(cmd, MeshCommand::SetPeerTrustBatch { rules }
            if rules.iter().any(|rule| !rule.trusted))
        {
            return Err("Policy Violation: Legacy BAN has no signed absolute deadline");
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
            self.validate_log_command(&entry.command)?;
        }
        Ok(())
    }
}
