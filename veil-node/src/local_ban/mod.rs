//! Bounded, volatile mitigation. No signing keys, consensus, files or recovery log.
use std::{collections::BTreeSet, net::Ipv4Addr, sync::Arc};
use veil_common::PeerPolicy;

pub const CAPACITY: usize = 256;
pub const SWEEP_BUDGET: usize = 16;
pub const EVENT_CAPACITY: usize = 64;
pub const MAX_TTL_SECS: u32 = 86_400;
pub const STEP_NS: u64 = 200_000_000;

#[cfg(target_os = "linux")]
mod attach_guard;
#[cfg(target_os = "linux")]
mod kernel;
#[cfg(target_os = "linux")]
pub mod runtime;
#[cfg(target_os = "linux")]
mod service;
#[cfg(target_os = "linux")]
mod sockets;

/// Implementations must be bounded maps owned exclusively by this instance.
pub trait Backend: Send + Sync {
    fn insert(&self, ip: u32, policy: PeerPolicy) -> anyhow::Result<()>;
    fn remove(&self, ip: u32) -> anyhow::Result<()>;
    fn bypass(&self, enabled: bool) -> anyhow::Result<()>;
    fn counters(&self) -> anyhow::Result<(u64, u64)>;
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    ip: u32,
    deadline: u64,
    confirmed: bool,
}

/// A bounded userspace mirror, not proof of packet enforcement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BanEntry {
    pub ip: Ipv4Addr,
    pub expires_at_boot_ns: u64,
    pub confirmed: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub ip: u32,
    pub operation: u8,
    pub boot_ns: u64,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Sweep {
    pub examined: usize,
    pub attempted: usize,
    pub removed: usize,
}

pub struct Engine {
    backend: Arc<dyn Backend>,
    protected: BTreeSet<u32>,
    slots: [Option<Slot>; CAPACITY],
    events: [Option<Event>; EVENT_CAPACITY],
    event_cursor: usize,
    event_count: usize,
    cursor: usize,
    revision: u64,
    last_mutation: Option<u64>,
    last_now: u64,
    pub steps: u64,
    pub max_examined: usize,
    pub max_attempted: usize,
    pub last_sweep: Sweep,
}

impl Engine {
    pub fn new(backend: Arc<dyn Backend>, protected: &[Ipv4Addr]) -> Result<Self, &'static str> {
        if protected.len() > 256 {
            return Err("TOO_MANY_PROTECTED_IPS");
        }
        Ok(Self {
            backend,
            protected: protected.iter().copied().map(u32::from).collect(),
            slots: [None; CAPACITY],
            events: [None; EVENT_CAPACITY],
            event_cursor: 0,
            event_count: 0,
            cursor: 0,
            revision: 0,
            last_mutation: None,
            last_now: 0,
            steps: 0,
            max_examined: 0,
            max_attempted: 0,
            last_sweep: Sweep::default(),
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn occupied(&self) -> usize {
        self.slots.iter().flatten().count()
    }
    pub fn active(&self, now: u64) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|s| now < s.deadline)
            .count()
    }
    pub fn pending(&self) -> usize {
        self.slots.iter().flatten().filter(|s| !s.confirmed).count()
    }
    pub fn event_count(&self) -> usize {
        self.event_count
    }
    pub fn verdict(&self, ip: u32, now: u64) -> &'static str {
        match self.slots.iter().flatten().find(|s| s.ip == ip) {
            Some(s) if now < s.deadline && !s.confirmed => "Unknown",
            Some(s) if now < s.deadline => "Drop",
            _ => "Pass",
        }
    }
    pub fn deadline(&self, ip: u32) -> Option<u64> {
        self.slots
            .iter()
            .flatten()
            .find(|s| s.ip == ip)
            .map(|s| s.deadline)
    }
    pub fn entries(&self) -> [Option<BanEntry>; CAPACITY] {
        self.slots.map(|s| {
            s.map(|s| BanEntry {
                ip: Ipv4Addr::from(s.ip),
                expires_at_boot_ns: s.deadline,
                confirmed: s.confirmed,
            })
        })
    }
    /// Read-only admission checks. Applying still revalidates time, capacity,
    /// instance and revision; a preview neither reserves nor renews a BAN.
    pub fn preview_ban(&self, ip: u32, ttl: u32, now: u64) -> Result<(), &'static str> {
        self.preview_unban(ip, now)?;
        if ttl == 0 || ttl > MAX_TTL_SECS {
            return Err("INVALID_TTL");
        }
        now.checked_add(u64::from(ttl) * 1_000_000_000)
            .ok_or("DEADLINE_OVERFLOW")?;
        if !self
            .slots
            .iter()
            .any(|s| s.is_none() || s.is_some_and(|s| s.ip == ip))
        {
            return Err("CAPACITY");
        }
        Ok(())
    }
    pub fn preview_unban(&self, ip: u32, now: u64) -> Result<(), &'static str> {
        if now < self.last_now {
            return Err("CLOCK_REVERSED");
        }
        self.target(ip)
    }
    fn time(&mut self, now: u64) -> Result<(), &'static str> {
        if now < self.last_now {
            return Err("CLOCK_REVERSED");
        }
        self.last_now = now;
        Ok(())
    }
    fn target(&self, ip: u32) -> Result<(), &'static str> {
        let ip_addr = Ipv4Addr::from(ip);
        if ip_addr.is_loopback()
            || ip_addr.is_unspecified()
            || ip_addr.is_multicast()
            || ip_addr.is_broadcast()
            || self.protected.contains(&ip)
        {
            return Err("PROTECTED_OR_INVALID_IP");
        }
        Ok(())
    }
    fn reserve_revision(&mut self, expected: u64, now: u64) -> Result<(), &'static str> {
        if expected != self.revision {
            return Err("REVISION_CONFLICT");
        }
        if self
            .last_mutation
            .is_some_and(|t| now.saturating_sub(t) < STEP_NS)
        {
            return Err("RATE_LIMIT");
        }
        self.revision = self.revision.checked_add(1).ok_or("REVISION_EXHAUSTED")?;
        self.last_mutation = Some(now);
        Ok(())
    }
    fn event(&mut self, ip: u32, operation: u8, now: u64) {
        self.events[self.event_cursor] = Some(Event {
            ip,
            operation,
            boot_ns: now,
        });
        self.event_cursor = (self.event_cursor + 1) % EVENT_CAPACITY;
        self.event_count = (self.event_count + 1).min(EVENT_CAPACITY);
    }
    pub fn ban(&mut self, ip: u32, ttl: u32, expected: u64, now: u64) -> Result<(), &'static str> {
        self.time(now)?;
        self.preview_ban(ip, ttl, now)?;
        let deadline = now
            .checked_add(u64::from(ttl) * 1_000_000_000)
            .ok_or("DEADLINE_OVERFLOW")?;
        let index = self
            .slots
            .iter()
            .position(|s| s.is_some_and(|s| s.ip == ip))
            .or_else(|| self.slots.iter().position(Option::is_none))
            .ok_or("CAPACITY")?;
        self.reserve_revision(expected, now)?;
        let previous_deadline = self.slots[index].map(|s| s.deadline).unwrap_or(0);
        // Reserve before the syscall: a failed/unknown insert is still tracked.
        self.slots[index] = Some(Slot {
            ip,
            deadline,
            confirmed: false,
        });
        let mut policy = PeerPolicy::new(false, 0);
        policy.expires_at_boot_ns = deadline;
        if self.backend.insert(ip, policy).is_err() {
            // A failed replacement must not forget the older, possibly longer
            // kernel lease or claim Pass while that lease could still apply.
            self.slots[index].as_mut().unwrap().deadline = deadline.max(previous_deadline);
            return Err("MAP_INSERT_FAILED");
        }
        self.slots[index].as_mut().unwrap().confirmed = true;
        self.event(ip, b'B', now);
        Ok(())
    }
    pub fn unban(&mut self, ip: u32, expected: u64, now: u64) -> Result<(), &'static str> {
        self.time(now)?;
        self.target(ip)?;
        self.reserve_revision(expected, now)?;
        self.backend.remove(ip).map_err(|_| "MAP_DELETE_FAILED")?;
        if let Some(index) = self
            .slots
            .iter()
            .position(|s| s.is_some_and(|s| s.ip == ip))
        {
            self.slots[index] = None;
        }
        self.event(ip, b'R', now);
        Ok(())
    }
    pub fn sweep(&mut self, now: u64) -> Result<Sweep, &'static str> {
        self.time(now)?;
        let mut stats = Sweep::default();
        for _ in 0..SWEEP_BUDGET {
            let index = self.cursor;
            self.cursor = (self.cursor + 1) % CAPACITY;
            stats.examined += 1;
            if let Some(slot) = self.slots[index] {
                if now >= slot.deadline {
                    stats.attempted += 1;
                    if self.backend.remove(slot.ip).is_ok() {
                        self.slots[index] = None;
                        stats.removed += 1;
                        self.event(slot.ip, b'E', now);
                    }
                }
            }
        }
        self.steps = self.steps.saturating_add(1);
        self.max_examined = self.max_examined.max(stats.examined);
        self.max_attempted = self.max_attempted.max(stats.attempted);
        self.last_sweep = stats;
        Ok(stats)
    }
}

#[cfg(test)]
mod tests;
