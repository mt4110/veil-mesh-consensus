//! Canonical fixed voter configuration. Local paths and bind addresses are not cluster data.
use super::{SecurityPolicyGuard, SigningContext, MAX_POLICY_TTL_SECS};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::FromStr,
};

pub const MAX_MEMBERS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Member {
    pub node_id: String,
    pub address: SocketAddr,
    pub public_key: [u8; 32],
}

pub fn valid_node_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

impl FromStr for Member {
    type Err = &'static str;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (id, rest) = input
            .split_once('@')
            .ok_or("Member must be ID@IP:PORT=HEX_PUBLIC_KEY")?;
        let (address, key) = rest
            .split_once('=')
            .ok_or("Member must include a public key")?;
        if key.len() != 64 {
            return Err("Ed25519 public key must have exactly 64 hex characters");
        }
        let mut public_key = [0u8; 32];
        for (i, pair) in key.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let hi = (pair[0] as char)
                .to_digit(16)
                .ok_or("Invalid public key hex")?;
            let lo = (pair[1] as char)
                .to_digit(16)
                .ok_or("Invalid public key hex")?;
            public_key[i] = (hi * 16 + lo) as u8;
        }
        Ok(Self {
            node_id: id.into(),
            address: address
                .parse()
                .map_err(|_| "Member address must be an IP socket address")?,
            public_key,
        })
    }
}

#[derive(Clone, Debug)]
pub struct FixedConfiguration {
    context: SigningContext,
    members: Vec<Member>,
    protected_ips: BTreeSet<u32>,
    max_clock_error_ms: u64,
    approval: Option<super::ApprovalPolicy>,
}

impl FixedConfiguration {
    pub fn new(
        cluster_id: impl Into<String>,
        config_epoch: u64,
        mut members: Vec<Member>,
        protected_ips: Vec<Ipv4Addr>,
        max_clock_error_ms: u64,
    ) -> Result<Self, &'static str> {
        let mut context = SigningContext::new(cluster_id, config_epoch)?;
        if members.is_empty() || members.len() > MAX_MEMBERS {
            return Err("Fixed membership requires 1..64 voters");
        }
        if protected_ips.len() > 256 {
            return Err("Too many configured protected IPv4 addresses");
        }
        if max_clock_error_ms == 0 || max_clock_error_ms.checked_mul(1_000_000).is_none() {
            return Err("Invalid clock error budget");
        }
        let mut ids = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        let mut protected = SecurityPolicyGuard::default().protected_ips;
        protected.extend(protected_ips.into_iter().map(u32::from));
        for member in &mut members {
            if !valid_node_id(&member.node_id) || !ids.insert(member.node_id.clone()) {
                return Err("Invalid or duplicate member ID");
            }
            if member.public_key == [0; 32] || !keys.insert(member.public_key) {
                return Err("Zero or duplicate member public key");
            }
            // IPv4-mapped IPv6 and IPv4 are the same endpoint and policy identity.
            if let SocketAddr::V6(v6) = member.address {
                if v6.scope_id() != 0 || v6.flowinfo() != 0 {
                    return Err("Scoped IPv6 member endpoints are unsupported");
                }
                if let Some(v4) = v6.ip().to_ipv4_mapped() {
                    member.address = SocketAddr::new(IpAddr::V4(v4), v6.port());
                }
            }
            let ip = member.address.ip();
            if member.address.port() == 0
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip == IpAddr::V4(Ipv4Addr::BROADCAST)
                || !addresses.insert(member.address)
            {
                return Err("Invalid or duplicate member endpoint");
            }
            if let IpAddr::V4(ip) = ip {
                protected.insert(u32::from(ip));
            }
        }
        members.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        // Order-independent input, deterministic bytes. Profile includes voter role,
        // quorum algorithm and policy semantics; changes require a new schema/protocol.
        let payload = bincode::serialize(&(
            "VEIL_FIXED_CONFIG_V1",
            context.protocol_version,
            &context.cluster_id,
            context.config_epoch,
            "all-voters:floor(2N/3)+1",
            &members,
            &protected,
            MAX_POLICY_TTL_SECS,
            max_clock_error_ms,
            1_000u32,
        ))
        .expect("Fixed configuration serialization");
        context
            .config_digest
            .copy_from_slice(ring::digest::digest(&ring::digest::SHA256, &payload).as_ref());
        Ok(Self {
            context,
            members,
            protected_ips: protected,
            max_clock_error_ms,
            approval: None,
        })
    }

    pub fn with_approval(
        mut self,
        mut approval: super::ApprovalPolicy,
    ) -> Result<Self, &'static str> {
        if approval.public_key == [0; 32]
            || self
                .members
                .iter()
                .any(|m| m.public_key == approval.public_key)
            || approval.scopes.is_empty()
            || approval.scopes.len() > 16
        {
            return Err("Require independent approval key and 1..16 explicit scopes");
        }
        approval.scopes.sort();
        approval.scopes.dedup();
        for s in &approval.scopes {
            let ip = Ipv4Addr::from(s.destination);
            if ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || s.port == 0
                || !matches!(s.protocol, 6 | 17)
            {
                return Err("Invalid scope");
            }
        }
        self.context.protocol_version = 6;
        let bytes = bincode::serialize(&("VEIL_APPROVED_SCOPE_V1", &self.context, &approval))
            .expect("profile");
        self.context
            .config_digest
            .copy_from_slice(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
        self.approval = Some(approval);
        Ok(self)
    }
    pub fn context(&self) -> &SigningContext {
        &self.context
    }
    pub fn members(&self) -> &[Member] {
        &self.members
    }
    pub fn max_clock_error_ms(&self) -> u64 {
        self.max_clock_error_ms
    }
    pub fn policy_guard(&self) -> SecurityPolicyGuard {
        SecurityPolicyGuard {
            protected_ips: self.protected_ips.clone(),
            approval: self.approval.clone(),
        }
    }
    pub fn public_keys(&self) -> BTreeMap<String, Vec<u8>> {
        self.members
            .iter()
            .map(|m| (m.node_id.clone(), m.public_key.to_vec()))
            .collect()
    }
    pub fn peer_addrs(&self, self_id: &str) -> BTreeMap<String, SocketAddr> {
        self.members
            .iter()
            .filter(|m| m.node_id != self_id)
            .map(|m| (m.node_id.clone(), m.address))
            .collect()
    }
    pub fn validate_local(&self, self_id: &str, public_key: &[u8]) -> Result<(), &'static str> {
        let member = self
            .members
            .iter()
            .find(|m| m.node_id == self_id)
            .ok_or("Local node is absent from fixed membership")?;
        if member.public_key.as_slice() != public_key {
            return Err("Local signing key does not match fixed membership");
        }
        Ok(())
    }
}
