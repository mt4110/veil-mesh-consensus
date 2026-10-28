//! Read-only admission for a dedicated, simple NIC. Never SET features or retry
//! attachment. GET and attachment are not atomic against other administrators.
use anyhow::{bail, ensure, Context, Result};
use aya::programs::XdpFlags;
use clap::ValueEnum;
use std::{
    collections::BTreeSet,
    ffi::CString,
    mem,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum AttachMode {
    #[default]
    Default,
    Generic,
}
impl AttachMode {
    pub fn flags(self) -> XdpFlags {
        match self {
            Self::Default => XdpFlags::empty(),
            Self::Generic => XdpFlags::SKB_MODE,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Generic => "generic",
        }
    }
}

const MAX_FEATURES: usize = 256;
const NAME_LEN: usize = 32;
const ETH_SS_FEATURES: u32 = 4;
const SIOCETHTOOL: libc::c_ulong = 0x8946;
const GSSET_INFO: u32 = 0x37;
const GSTRINGS: u32 = 0x1b;
const GFEATURES: u32 = 0x3a;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Features {
    names: Vec<String>,
    wanted: Vec<u32>,
    active: Vec<u32>,
}
impl Features {
    fn decode(names: &[u8], blocks: &[u32], count: usize) -> Result<Self> {
        ensure!(
            (1..=MAX_FEATURES).contains(&count),
            "Feature count outside guard bound"
        );
        let words = count.div_ceil(32);
        ensure!(
            names.len() == count * NAME_LEN && blocks.len() == words * 4,
            "Incomplete feature reply"
        );
        let mut seen = BTreeSet::new();
        let names = names
            .chunks_exact(NAME_LEN)
            .map(|b| {
                let end = b
                    .iter()
                    .position(|v| *v == 0)
                    .context("Unterminated feature name")?;
                ensure!(
                    b[end..].iter().all(|v| *v == 0),
                    "Malformed feature name padding"
                );
                let name = std::str::from_utf8(&b[..end])?;
                // Empty entries are reserved feature bits, not a missing reply.
                ensure!(
                    name.is_empty() || seen.insert(name.to_owned()),
                    "Duplicate feature name"
                );
                Ok(name.to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            names,
            wanted: blocks.chunks_exact(4).map(|b| b[1]).collect(),
            active: blocks.chunks_exact(4).map(|b| b[2]).collect(),
        })
    }
    fn admit(&self) -> Result<()> {
        for required in ["rx-lro", "rx-gro-hw"] {
            let bit = self
                .names
                .iter()
                .position(|s| s == required)
                .with_context(|| format!("Missing {required} active/wanted information"))?;
            ensure!(
                (self.wanted[bit / 32] | self.active[bit / 32]) & (1 << (bit % 32)) == 0,
                "{required} must already be off in both active and wanted; no automatic NIC change"
            );
        }
        Ok(())
    }
}

fn socket(domain: i32, protocol: i32) -> Result<OwnedFd> {
    // SAFETY: successful socket returns one owned fd, immediately wrapped.
    let fd = unsafe { libc::socket(domain, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, protocol) };
    ensure!(fd >= 0, "Guard socket: {}", std::io::Error::last_os_error());
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn interface_index(name: &str) -> Result<u32> {
    ensure!(
        !name.is_empty() && name.len() < libc::IFNAMSIZ,
        "Invalid interface name length"
    );
    let name = CString::new(name)?;
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    ensure!(
        index > 0,
        "Cannot resolve interface: {}",
        std::io::Error::last_os_error()
    );
    Ok(index)
}
// Only these three read-only UAPI commands can reach ioctl. AF_UNIX works
// with SIOCETHTOOL in the existing RestrictAddressFamilies profile.
fn ethtool_get(fd: &OwnedFd, name: &str, data: &mut [u32]) -> Result<()> {
    ensure!(
        !data.is_empty() && [GSSET_INFO, GSTRINGS, GFEATURES].contains(&data[0]),
        "Read-only ethtool command required"
    );
    ensure!(
        !name.is_empty() && name.len() < libc::IFNAMSIZ && !name.as_bytes().contains(&0),
        "Invalid interface"
    );
    let mut req: libc::ifreq = unsafe { mem::zeroed() };
    for (dst, src) in req.ifr_name.iter_mut().zip(name.bytes()) {
        *dst = src as libc::c_char;
    }
    req.ifr_ifru.ifru_data = data.as_mut_ptr().cast();
    let rc = unsafe { libc::ioctl(fd.as_raw_fd(), SIOCETHTOOL, &mut req) };
    ensure!(
        rc == 0,
        "Read-only ethtool GET failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}
fn read_features(name: &str) -> Result<Features> {
    let fd = socket(libc::AF_UNIX, 0)?;
    // ethtool_sset_info: cmd,reserved,u64 mask, followed by one u32 count.
    // u32 storage has enough space, kernel copies bytes (no u64 dereference).
    let mut info = vec![0u32; 6];
    info[0] = GSSET_INFO;
    let mask = (1u64 << ETH_SS_FEATURES).to_ne_bytes();
    info[2] = u32::from_ne_bytes(mask[..4].try_into()?);
    info[3] = u32::from_ne_bytes(mask[4..].try_into()?);
    ethtool_get(&fd, name, &mut info)?;
    ensure!(
        info[0] == GSSET_INFO
            && info[1] == 0
            && info[2].to_ne_bytes() == mask[..4]
            && info[3].to_ne_bytes() == mask[4..],
        "Feature string set unavailable"
    );
    let count = info[4] as usize;
    ensure!(
        (1..=MAX_FEATURES).contains(&count),
        "Feature count outside guard bound"
    );
    let mut strings = vec![0u32; 3 + count * NAME_LEN / 4];
    strings[..3].copy_from_slice(&[GSTRINGS, ETH_SS_FEATURES, count as u32]);
    ethtool_get(&fd, name, &mut strings)?;
    ensure!(
        strings[..3] == [GSTRINGS, ETH_SS_FEATURES, count as u32],
        "Feature names changed during GET"
    );
    let names: Vec<u8> = strings[3..].iter().flat_map(|v| v.to_ne_bytes()).collect();
    let words = count.div_ceil(32);
    let mut bits = vec![0u32; 2 + words * 4];
    bits[..2].copy_from_slice(&[GFEATURES, words as u32]);
    ethtool_get(&fd, name, &mut bits)?;
    ensure!(
        bits[..2] == [GFEATURES, words as u32],
        "Feature block count changed during GET"
    );
    Features::decode(&names, &bits[2..], count)
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_ne_bytes(
        bytes
            .get(offset..offset + 4)
            .context("Truncated u32")?
            .try_into()?,
    ))
}
fn string(bytes: &[u8]) -> Result<&str> {
    ensure!(
        bytes.last() == Some(&0) && !bytes[..bytes.len() - 1].contains(&0),
        "Malformed netlink string"
    );
    Ok(std::str::from_utf8(&bytes[..bytes.len() - 1])?)
}
fn attributes(mut bytes: &[u8]) -> Result<Vec<(u16, &[u8])>> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    while !bytes.is_empty() {
        ensure!(
            bytes.len() >= 4 && out.len() < 256,
            "Attribute header/count outside bound"
        );
        let len = u16::from_ne_bytes(bytes[..2].try_into()?) as usize;
        let raw = u16::from_ne_bytes(bytes[2..4].try_into()?);
        let kind = raw & 0x3fff;
        ensure!(
            raw & 0x4000 == 0 && seen.insert(kind),
            "Network-order or duplicate attribute"
        );
        let aligned = (len + 3) & !3;
        ensure!(
            len >= 4 && aligned <= bytes.len(),
            "Invalid attribute length"
        );
        out.push((kind, &bytes[4..len]));
        bytes = &bytes[aligned..];
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
struct LinkState {
    index: u32,
    name: String,
    mtu: u32,
    kind: Option<String>,
    xdp_mode: u8,
    // aggregate, driver, skb, hardware
    programs: [u32; 4],
}
impl LinkState {
    fn decode(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() >= 16 && u16::from_ne_bytes(data[2..4].try_into()?) == 1,
            "Only Ethernet NICs admitted"
        );
        let index = u32_at(data, 4)?;
        ensure!(
            index > 0 && index <= i32::MAX as u32,
            "Invalid interface index"
        );
        let (mut name, mut mtu, mut kind, mut mode, mut ids) = (None, None, None, None, [0; 4]);
        let mut lower = None;
        for (ty, value) in attributes(&data[16..])? {
            match ty {
                3 => name = Some(string(value)?.to_owned()), // IFLA_IFNAME
                4 => {
                    ensure!(value.len() == 4, "Invalid MTU");
                    mtu = Some(u32_at(value, 0)?);
                }
                5 => {
                    ensure!(value.len() == 4, "Invalid lower link");
                    lower = Some(u32_at(value, 0)?);
                }
                10 => {
                    ensure!(
                        value.len() == 4 && u32_at(value, 0)? == 0,
                        "Master/stacked device unsupported"
                    );
                }
                18 => {
                    for (field, v) in attributes(value)? {
                        match field {
                            1 => kind = Some(string(v)?.to_owned()),
                            2 => {}
                            _ => bail!("Unknown/slave link topology"),
                        }
                    }
                }
                43 => {
                    // IFLA_XDP
                    for (field, v) in attributes(value)? {
                        match field {
                            2 => {
                                ensure!(v.len() == 1, "Invalid XDP mode");
                                mode = Some(v[0]);
                            }
                            4..=7 => {
                                ensure!(v.len() == 4, "Invalid XDP program ID");
                                ids[(field - 4) as usize] = u32_at(v, 0)?;
                            }
                            3 => {} // flags (informational)
                            _ => bail!("Unknown XDP state"),
                        }
                    }
                }
                _ => {}
            }
        }
        ensure!(
            kind.as_deref().is_none() || kind.as_deref() == Some("veth"),
            "Only plain Ethernet or isolated veth supported"
        );
        ensure!(
            kind.as_deref() == Some("veth") || lower.is_none() || lower == Some(index),
            "Lower/stacked device unsupported"
        );
        let mtu = mtu.context("Missing MTU")?;
        ensure!(mtu > 0, "Invalid MTU");
        Ok(Self {
            index,
            name: name.context("Missing interface name")?,
            mtu,
            kind,
            xdp_mode: mode.context("Missing XDP state")?,
            programs: ids,
        })
    }
    fn vacant(&self) -> Result<()> {
        ensure!(
            self.xdp_mode == 0 && self.programs == [0; 4],
            "Existing XDP; replacement forbidden"
        );
        Ok(())
    }
}

// One bounded rtnetlink transaction. No RTM_NEW/SET/DEL message is emitted.
fn route_get(ty: u16, index: u32, dump: bool) -> Result<Vec<Vec<u8>>> {
    route_get_at(ty, index, dump, 0)
}
fn route_get_at(ty: u16, index: u32, dump: bool, parent: u32) -> Result<Vec<Vec<u8>>> {
    ensure!(
        matches!((ty, dump), (18, false) | (38, true) | (46, true))
            && (ty == 46 || parent == 0)
            && parent & 0xffff == 0,
        "Only bounded link/qdisc/filter GET permitted"
    );
    let fd = socket(libc::AF_NETLINK, libc::NETLINK_ROUTE)?;
    let mut local: libc::sockaddr_nl = unsafe { mem::zeroed() };
    local.nl_family = libc::AF_NETLINK as u16;
    ensure!(
        unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&local as *const libc::sockaddr_nl).cast(),
                mem::size_of_val(&local) as _,
            )
        } == 0,
        "Netlink bind: {}",
        std::io::Error::last_os_error()
    );
    let timeout = libc::timeval {
        tv_sec: 1,
        tv_usec: 0,
    };
    ensure!(
        unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&timeout as *const libc::timeval).cast(),
                mem::size_of_val(&timeout) as _,
            )
        } == 0,
        "Netlink receive timeout"
    );
    let body_len = if dump { 20 } else { 16 }; // tcmsg / ifinfomsg
    let mut request = vec![0u8; 16 + body_len];
    let len = request.len() as u32;
    request[..4].copy_from_slice(&len.to_ne_bytes());
    request[4..6].copy_from_slice(&ty.to_ne_bytes());
    request[6..8].copy_from_slice(&(if dump { 0x301u16 } else { 1 }).to_ne_bytes());
    request[8..12].copy_from_slice(&1u32.to_ne_bytes());
    request[20..24].copy_from_slice(&index.to_ne_bytes());
    if ty == 46 {
        request[28..32].copy_from_slice(&parent.to_ne_bytes());
    }
    let mut kernel: libc::sockaddr_nl = unsafe { mem::zeroed() };
    kernel.nl_family = libc::AF_NETLINK as u16;
    ensure!(
        unsafe {
            libc::sendto(
                fd.as_raw_fd(),
                request.as_ptr().cast(),
                request.len(),
                0,
                (&kernel as *const libc::sockaddr_nl).cast(),
                mem::size_of_val(&kernel) as _,
            )
        } == request.len() as isize,
        "Netlink GET send: {}",
        std::io::Error::last_os_error()
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut messages = Vec::new();
    let mut buffer = vec![0u8; 65536];
    for _ in 0..16 {
        ensure!(Instant::now() < deadline, "Netlink GET deadline");
        let mut sender: libc::sockaddr_nl = unsafe { mem::zeroed() };
        let mut sender_len = mem::size_of_val(&sender) as libc::socklen_t;
        let n = unsafe {
            libc::recvfrom(
                fd.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::MSG_TRUNC,
                (&mut sender as *mut libc::sockaddr_nl).cast(),
                &mut sender_len,
            )
        };
        ensure!(
            n > 0 && n as usize <= buffer.len(),
            "Netlink GET truncated/failed: {}",
            std::io::Error::last_os_error()
        );
        ensure!(
            sender_len as usize == mem::size_of_val(&sender)
                && sender.nl_pid == 0
                && sender.nl_groups == 0,
            "Non-kernel netlink sender"
        );
        let mut data = &buffer[..n as usize];
        while !data.is_empty() {
            ensure!(data.len() >= 16, "Truncated netlink header");
            let len = u32_at(data, 0)? as usize;
            ensure!(
                len >= 16 && ((len + 3) & !3) <= data.len() && u32_at(data, 8)? == 1,
                "Invalid netlink length/sequence"
            );
            let msg_type = u16::from_ne_bytes(data[4..6].try_into()?);
            let flags = u16::from_ne_bytes(data[6..8].try_into()?);
            ensure!(flags & 0x10 == 0, "Interrupted netlink dump");
            if msg_type == 2 {
                bail!("Netlink GET error: {}", u32_at(&data[16..len], 0)? as i32);
            }
            if msg_type == 3 {
                ensure!(
                    dump && (len == 16 || (len >= 20 && u32_at(data, 16)? == 0)),
                    "Failed netlink dump"
                );
                return Ok(messages);
            }
            ensure!(
                msg_type
                    == match ty {
                        18 => 16,
                        38 => 36,
                        46 => 44,
                        _ => unreachable!(),
                    },
                "Unexpected netlink message"
            );
            ensure!(messages.len() < 256, "Netlink message count bound");
            messages.push(data[16..len].to_vec());
            data = &data[(len + 3) & !3..];
            if !dump {
                ensure!(data.is_empty(), "Unexpected extra link reply");
                return Ok(messages);
            }
        }
    }
    bail!("Netlink GET response count bound")
}
fn read_link(name: &str, index: u32) -> Result<LinkState> {
    ensure!(
        interface_index(name)? == index,
        "Interface changed before GET"
    );
    let rows = route_get(18, index, false)?;
    ensure!(rows.len() == 1, "Missing link reply");
    let state = LinkState::decode(&rows[0])?;
    ensure!(
        state.index == index && state.name == name,
        "Interface identity changed"
    );
    Ok(state)
}
fn read_qdiscs(index: u32) -> Result<Vec<Vec<u8>>> {
    let selected = decode_qdiscs(route_get(38, index, true)?, index)?;
    // fq_codel, although commonly a default qdisc, supports classifiers.
    // Query every selectable qdisc, all chains/priorities; any reply refuses.
    let mut handles = BTreeSet::new();
    for row in &selected {
        handles.insert(u32_at(row, 8)?);
    }
    for handle in handles {
        ensure!(
            route_get_at(46, index, true, handle)?.is_empty(),
            "Existing TC filter; shared policy unsupported"
        );
    }
    Ok(selected)
}
fn decode_qdiscs(rows: Vec<Vec<u8>>, index: u32) -> Result<Vec<Vec<u8>>> {
    let mut selected = Vec::new();
    for row in rows {
        ensure!(row.len() >= 20, "Incomplete qdisc reply");
        if u32_at(&row, 4)? != index {
            continue;
        }
        ensure!(
            selected.len() < 16 && u32_at(&row, 8)? & 0xffff == 0,
            "Qdisc topology outside dedicated guard bound"
        );
        let attrs = attributes(&row[20..])?;
        let kind = attrs
            .iter()
            .find(|(ty, _)| *ty == 1)
            .context("Missing qdisc kind")?
            .1;
        ensure!(
            ["noqueue", "mq", "fq_codel", "pfifo_fast", "fq"].contains(&string(kind)?),
            "Existing TC hook or unsupported qdisc"
        );
        // Store structural identity only: traffic statistics change normally.
        let mut identity = row[..16].to_vec();
        identity.extend_from_slice(kind);
        selected.push(identity);
    }
    selected.sort();
    Ok(selected)
}

pub struct Guard {
    before: LinkState,
    features: Features,
    qdiscs: Vec<Vec<u8>>,
}
impl Guard {
    pub fn before(name: &str) -> Result<Self> {
        let index = interface_index(name)?;
        let before = read_link(name, index)?;
        before.vacant()?;
        let qdiscs = read_qdiscs(index)?;
        let features =
            read_features(name).context("Cannot establish active/wanted offload state")?;
        features.admit()?;
        // Recheck topology and identity after the separate feature GET calls.
        ensure!(
            read_link(name, index)? == before,
            "Interface changed during preflight"
        );
        Ok(Self {
            before,
            features,
            qdiscs,
        })
    }
    pub fn ifindex(&self) -> u32 {
        self.before.index
    }
    pub fn after(&self, mode: AttachMode, program: u32) -> Result<&'static str> {
        let after = read_link(&self.before.name, self.before.index)?;
        self.validate_after(
            &after,
            &read_features(&self.before.name)?,
            &read_qdiscs(self.before.index)?,
            mode,
            program,
        )
    }
    fn validate_after(
        &self,
        after: &LinkState,
        features: &Features,
        qdiscs: &[Vec<u8>],
        mode: AttachMode,
        program: u32,
    ) -> Result<&'static str> {
        ensure!(
            after.index == self.before.index
                && after.name == self.before.name
                && after.mtu == self.before.mtu
                && after.kind == self.before.kind,
            "NIC identity/topology/MTU changed"
        );
        features.admit()?;
        ensure!(
            *features == self.features && qdiscs == self.qdiscs,
            "NIC active/wanted features or TC changed; refuse READY, release owned link only"
        );
        ensure!(
            program != 0 && after.programs[0] == program && after.programs[3] == 0,
            "XDP owner/program mismatch"
        );
        match (mode, after.xdp_mode, after.programs[1], after.programs[2]) {
            (_, 2, 0, id) if id == program => Ok("generic"),
            (AttachMode::Default, 1, id, 0) if id == program => Ok("driver"),
            _ => bail!("Unexpected XDP mode/program; no fallback"),
        }
    }
}

#[cfg(test)]
mod tests;
