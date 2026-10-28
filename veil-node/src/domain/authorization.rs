//! Independent operator authorization. Validators hold only the public key.
use super::*;
use serde::{Deserialize, Serialize};
use std::{net::Ipv4Addr, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ServiceScope {
    pub destination: u32,
    pub protocol: u8,
    pub port: u16,
}
impl FromStr for ServiceScope {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = s.split('/').collect();
        if parts.len() != 3 {
            return Err("Scope must be IPv4/tcp/PORT or IPv4/udp/PORT");
        }
        let ip: Ipv4Addr = parts[0].parse().map_err(|_| "Scope requires exact IPv4")?;
        let protocol = match parts[1] {
            "tcp" => 6,
            "udp" => 17,
            _ => return Err("Scope requires tcp or udp"),
        };
        let port = parts[2].parse().map_err(|_| "Invalid scope port")?;
        if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || port == 0 {
            return Err("Invalid service scope");
        }
        Ok(Self {
            destination: u32::from(ip),
            protocol,
            port,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    pub public_key: [u8; 32],
    pub scopes: Vec<ServiceScope>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedPolicy {
    /// Exactly one greater than the last committed operator revision (CAS).
    pub revision: u64,
    pub issued_at_unix_ns: u64,
    /// Last time at which a new vote may authorize this operation, including unban.
    pub valid_until_unix_ns: u64,
    pub rule: DeadlinePeerTrustRule,
    pub signature: Vec<u8>,
}
impl ApprovedPolicy {
    pub fn payload(&self, context: &SigningContext) -> Vec<u8> {
        bincode::serialize(&(
            "VEIL_OPERATOR_POLICY_V1",
            context,
            self.revision,
            self.issued_at_unix_ns,
            self.valid_until_unix_ns,
            &self.rule,
        ))
        .expect("approval payload")
    }
}
pub fn parse_public_key(s: &str) -> Result<[u8; 32], &'static str> {
    if s.len() != 64 {
        return Err("Public key must be 64 hex characters");
    }
    let mut key = [0; 32];
    for (i, pair) in s.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        key[i] = ((pair[0] as char).to_digit(16).ok_or("Invalid key hex")? * 16
            + (pair[1] as char).to_digit(16).ok_or("Invalid key hex")?) as u8;
    }
    if key == [0; 32] {
        return Err("Zero approval key");
    }
    Ok(key)
}
