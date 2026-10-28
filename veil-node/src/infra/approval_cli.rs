//! Offline approval files. No node private key, daemon or listener is needed.
use crate::{adapter::NodeCryptoIdentity, domain::*};
use std::{
    io::{Read, Write},
    path::Path,
};
pub fn read(path: &Path) -> anyhow::Result<ApprovedPolicy> {
    use bincode::Options;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(1025)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 1024, "Approval file exceeds 1024 bytes");
    Ok(bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(1024)
        .reject_trailing_bytes()
        .deserialize(&bytes)?)
}
pub fn write(path: &Path, approval: &ApprovedPolicy) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(&bincode::serialize(approval)?)?;
    f.sync_all()?;
    std::fs::File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}
pub fn sign(
    config: &FixedConfiguration,
    key: &Path,
    ip: std::net::Ipv4Addr,
    trusted: bool,
    ttl: u32,
    previous_revision: u64,
) -> anyhow::Result<ApprovedPolicy> {
    let identity = NodeCryptoIdentity::load_existing_file("operator", key)?;
    let guard = config.policy_guard();
    let profile = guard
        .approval
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing approval profile"))?;
    anyhow::ensure!(
        identity.public_key_bytes == profile.public_key,
        "Approval key does not match configuration"
    );
    // Backdate by two seconds to tolerate the default verified clock bound.
    // The absolute expiration includes this allowance; it never extends TTL.
    let now = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    )?;
    let issued = now
        .checked_sub(2_000_000_000)
        .ok_or_else(|| anyhow::anyhow!("Clock underflow"))?;
    let deadline = if trusted {
        0
    } else {
        issued
            .checked_add(u64::from(ttl) * 1_000_000_000)
            .ok_or_else(|| anyhow::anyhow!("Deadline overflow"))?
    };
    anyhow::ensure!(
        trusted || deadline > now,
        "TTL must exceed the two-second signing clock allowance"
    );
    let mut a = ApprovedPolicy {
        revision: previous_revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Revision exhausted"))?,
        issued_at_unix_ns: issued,
        valid_until_unix_ns: if trusted {
            issued + 300_000_000_000
        } else {
            deadline.min(issued + 300_000_000_000)
        },
        rule: DeadlinePeerTrustRule {
            rule: PeerTrustRule::new(ip, trusted, if trusted { 0 } else { ttl }),
            not_after_unix_ns: deadline,
        },
        signature: vec![],
    };
    a.signature = identity.sign_progress_payload(&a.payload(config.context()));
    guard
        .validate_log_command(&MeshCommand::AuthorizedPolicy(Box::new(a.clone())))
        .map_err(anyhow::Error::msg)?;
    Ok(a)
}
