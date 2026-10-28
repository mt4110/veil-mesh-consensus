//! Reuses the existing map ABI/object, loads only XDP and owns its link FD.
use super::{
    attach_guard::{AttachMode, Guard},
    Backend,
};
use anyhow::{ensure, Context, Result};
use aya::{
    maps::{Array, HashMap, IterableMap, MapData, MapError, PerCpuArray},
    programs::{links::FdLink, Xdp},
    Ebpf,
};
use std::{net::Ipv4Addr, sync::Mutex};
use veil_common::{
    PeerPolicy, ServiceScopeKey, MAP_PACKETS_SEEN, MAP_PEER_POLICY, MAP_SERVICE_SCOPE,
    MAP_XDP_CONTROL,
};

pub struct Kernel {
    peers: Mutex<HashMap<MapData, u32, PeerPolicy>>,
    // Separate lock: a slow policy syscall never owns the bypass map lock.
    control: Mutex<Array<MapData, u32>>,
    counters: Mutex<PerCpuArray<MapData, u64>>,
    _link: FdLink,
    _owner: Mutex<Ebpf>,
    pub actual_mode: &'static str,
    pub program_id: u32,
}
impl Kernel {
    pub fn attach(
        object: &str,
        interface: &str,
        destination: Ipv4Addr,
        port: u16,
        mode: AttachMode,
    ) -> Result<Self> {
        ensure!(
            !interface.is_empty() && !interface.as_bytes().contains(&0),
            "Invalid interface"
        );
        ensure!(
            port != 0
                && !destination.is_unspecified()
                && !destination.is_loopback()
                && !destination.is_multicast()
                && !destination.is_broadcast(),
            "Invalid destination/port"
        );
        ensure!(
            aya::util::KernelVersion::current()? >= aya::util::KernelVersion::new(5, 9, 0),
            "FD-owned XDP requires Linux >=5.9"
        );
        let guard =
            Guard::before(interface).context("NIC preservation preflight refused attachment")?;
        let mut bpf = Ebpf::load_file(object)?;
        let peers = HashMap::<_, u32, PeerPolicy>::try_from(
            bpf.take_map(MAP_PEER_POLICY).context("Missing peer map")?,
        )?;
        ensure!(
            peers.map().info()?.max_entries() == 4096,
            "Unexpected peer capacity"
        );
        ensure!(
            peers.keys().next().is_none(),
            "Fresh empty peer map required"
        );
        let mut scopes = HashMap::<_, ServiceScopeKey, u8>::try_from(
            bpf.take_map(MAP_SERVICE_SCOPE)
                .context("Missing scope map")?,
        )?;
        ensure!(scopes.keys().next().is_none(), "Fresh scope map required");
        scopes.insert(
            ServiceScopeKey {
                destination: u32::from(destination),
                protocol: 6,
                port,
            },
            1,
            0,
        )?;
        // Maps must live as long as the loaded program, including scope.
        // Keeping the scope fd is unnecessary: the loaded program owns its map reference.
        let mut control = Array::<_, u32>::try_from(
            bpf.take_map(MAP_XDP_CONTROL)
                .context("Missing control map")?,
        )?;
        control.set(0, 1, 0)?;
        let counters = PerCpuArray::<_, u64>::try_from(
            bpf.take_map(MAP_PACKETS_SEEN).context("Missing counters")?,
        )?;
        let xdp: &mut Xdp = bpf
            .program_mut("veil_xdp_filter")
            .context("Missing XDP")?
            .try_into()?;
        xdp.load()?;
        // Linux >=5.9 uses BPF_LINK_CREATE here. dev_xdp_attach rejects an
        // existing link/program atomically. UPDATE_IF_NOEXIST is a netlink
        // flag, invalid for this FD-owned path; never fall back to replacement.
        let program_id = xdp.info()?.id();
        let link_id = xdp
            .attach_to_if_index(guard.ifindex(), mode.flags())
            .context("Single FD-owned XDP attachment failed; no fallback or NIC restoration")?;
        // Conversion also proves this is an FD link. If conversion or postflight
        // fails, RAII closes only our link, including before READY is published.
        let link: FdLink = xdp.take_link(link_id)?.try_into()?;
        let actual_mode = guard.after(mode, program_id)
            .context("Post-attachment preservation check failed; releasing owned link, not changing NIC settings")?;
        Ok(Self {
            peers: Mutex::new(peers),
            control: Mutex::new(control),
            counters: Mutex::new(counters),
            _link: link,
            _owner: Mutex::new(bpf),
            actual_mode,
            program_id,
        })
    }
}
impl Backend for Kernel {
    fn insert(&self, ip: u32, policy: PeerPolicy) -> Result<()> {
        ensure!(
            policy.trusted == 0 && policy.expires_at_boot_ns > 0,
            "Finite BAN required"
        );
        self.peers
            .lock()
            .map_err(|_| anyhow::anyhow!("Peer lock poisoned"))?
            .insert(ip, policy, 0)?;
        Ok(())
    }
    fn remove(&self, ip: u32) -> Result<()> {
        match self
            .peers
            .lock()
            .map_err(|_| anyhow::anyhow!("Peer lock poisoned"))?
            .remove(&ip)
        {
            Ok(()) | Err(MapError::KeyNotFound) => Ok(()),
            Err(MapError::SyscallError(e)) if e.io_error.kind() == std::io::ErrorKind::NotFound => {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
    fn bypass(&self, enabled: bool) -> Result<()> {
        self.control
            .lock()
            .map_err(|_| anyhow::anyhow!("Control lock poisoned"))?
            .set(0, u32::from(!enabled), 0)?;
        Ok(())
    }
    fn counters(&self) -> Result<(u64, u64)> {
        let map = self
            .counters
            .lock()
            .map_err(|_| anyhow::anyhow!("Counter lock poisoned"))?;
        Ok((
            map.get(&0, 0)?.iter().copied().sum(),
            map.get(&1, 0)?.iter().copied().sum(),
        ))
    }
}

pub fn check_clock_namespace() -> Result<()> {
    match std::fs::read_to_string("/proc/self/timens_offsets") {
        Ok(text) => {
            let row = text
                .lines()
                .find(|l| l.split_whitespace().next() == Some("boottime"))
                .context("Missing boottime offset")?;
            ensure!(
                row.split_whitespace().skip(1).collect::<Vec<_>>() == ["0", "0"],
                "Nonzero boot offset unsupported"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    boot_now()?;
    Ok(())
}
pub fn boot_now() -> Result<u64> {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writable timespec and Linux suspend-aware clock; read only.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut t) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    u64::try_from(t.tv_sec)?
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(t.tv_nsec as u64))
        .context("Clock overflow")
}
