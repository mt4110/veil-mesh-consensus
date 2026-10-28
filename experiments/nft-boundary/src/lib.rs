//! Offline boundary model. No sockets, processes, capabilities or nft execution.
//! Kernel evidence and peer credentials below are *model inputs*, not OS proof.
#![forbid(unsafe_code)]

use std::net::Ipv4Addr;

pub const CAPACITY: usize = 256;
pub const SWEEP_BUDGET: usize = 16;
pub const MAX_TTL_SECS: u32 = 86_400;
pub const CHANGE_INTERVAL_NS: u64 = 200_000_000;
pub const FRAME_BYTES: usize = 40;
pub const TABLE: &str = "veil_trial";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Frame,
    Version,
    Operation,
    Reserved,
    Config,
    Peer,
    Namespace,
    Owner,
    Instance,
    Revision,
    RevisionExhausted,
    Clock,
    Overflow,
    InvalidIp,
    InvalidTtl,
    Capacity,
    RateLimit,
    Busy,
    NotReady,
    TablePresent,
    Receipt,
    Unknown,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Ready,
    Pending,
    Quarantined,
    Closing,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Ban,
    Unban,
}

/// The real adapter must obtain these through SO_PEERCRED and pinned OS objects.
/// Values supplied by a request are never acceptable as credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedContext {
    pub uid: u32,
    pub namespace: u64,
    pub owner_socket: u64,
}

#[derive(Debug, Clone)]
pub struct Scope {
    destination: Ipv4Addr,
    port: u16,
    context: ObservedContext,
    protected: [Option<Ipv4Addr>; CAPACITY],
}

fn valid_ip(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && ip.octets()[0] != 0
        && ip.octets()[0] < 240
}

impl Scope {
    pub fn new(
        destination: Ipv4Addr,
        port: u16,
        context: ObservedContext,
        protected: &[Ipv4Addr],
    ) -> Result<Self, Error> {
        if !valid_ip(destination)
            || port == 0
            || context.uid == 0
            || context.namespace == 0
            || context.owner_socket == 0
            || protected.len() > CAPACITY
        {
            return Err(Error::Config);
        }
        let mut stored = [None; CAPACITY];
        for (slot, ip) in stored.iter_mut().zip(protected) {
            *slot = Some(*ip);
        }
        Ok(Self {
            destination,
            port,
            context,
            protected: stored,
        })
    }
    fn authorize(&self, seen: ObservedContext) -> Result<(), Error> {
        if seen.uid != self.context.uid {
            return Err(Error::Peer);
        }
        if seen.namespace != self.context.namespace {
            return Err(Error::Namespace);
        }
        if seen.owner_socket != self.context.owner_socket {
            return Err(Error::Owner);
        }
        Ok(())
    }
    fn target(&self, ip: Ipv4Addr) -> Result<(), Error> {
        if !valid_ip(ip) || ip == self.destination || self.protected.contains(&Some(ip)) {
            Err(Error::InvalidIp)
        } else {
            Ok(())
        }
    }
    pub fn destination(&self) -> Ipv4Addr {
        self.destination
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn protocol(&self) -> u8 {
        6
    }
    pub fn context(&self) -> ObservedContext {
        self.context
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub operation: Operation,
    pub instance: u128,
    pub revision: u64,
    pub ip: Ipv4Addr,
    pub ttl_secs: u32,
}

impl Request {
    pub fn decode(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() != FRAME_BYTES || &frame[..4] != b"VNF1" {
            return Err(Error::Frame);
        }
        if frame[4] != 1 {
            return Err(Error::Version);
        }
        if frame[6..8] != [0, 0] {
            return Err(Error::Reserved);
        }
        let operation = match frame[5] {
            1 => Operation::Ban,
            2 => Operation::Unban,
            _ => return Err(Error::Operation),
        };
        let request = Self {
            operation,
            instance: u128::from_be_bytes(frame[8..24].try_into().map_err(|_| Error::Frame)?),
            revision: u64::from_be_bytes(frame[24..32].try_into().map_err(|_| Error::Frame)?),
            ip: Ipv4Addr::new(frame[32], frame[33], frame[34], frame[35]),
            ttl_secs: u32::from_be_bytes(frame[36..40].try_into().map_err(|_| Error::Frame)?),
        };
        request.validate_ttl()?;
        Ok(request)
    }
    fn validate_ttl(&self) -> Result<(), Error> {
        match self.operation {
            Operation::Ban if self.ttl_secs == 0 || self.ttl_secs > MAX_TTL_SECS => {
                Err(Error::InvalidTtl)
            }
            Operation::Unban if self.ttl_secs != 0 => Err(Error::InvalidTtl),
            _ => Ok(()),
        }
    }
    pub fn encode(self) -> [u8; FRAME_BYTES] {
        let mut frame = [0; FRAME_BYTES];
        frame[..4].copy_from_slice(b"VNF1");
        frame[4] = 1;
        frame[5] = match self.operation {
            Operation::Ban => 1,
            Operation::Unban => 2,
        };
        frame[8..24].copy_from_slice(&self.instance.to_be_bytes());
        frame[24..32].copy_from_slice(&self.revision.to_be_bytes());
        frame[32..36].copy_from_slice(&self.ip.octets());
        frame[36..40].copy_from_slice(&self.ttl_secs.to_be_bytes());
        frame
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub ip: Ipv4Addr,
    pub deadline_boot_ns: u64,
    pub confirmed: bool,
}

/// Only this typed operation can be emitted. No caller-selected table, chain,
/// namespace, command text, path, policy, scope or executable is represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    operation: Operation,
    context: ObservedContext,
    instance: u128,
    revision: u64,
    destination: Ipv4Addr,
    port: u16,
    ip: Ipv4Addr,
    deadline: u64,
}
impl Transaction {
    pub fn instance(&self) -> u128 {
        self.instance
    }
    pub fn operation(&self) -> Operation {
        self.operation
    }
    pub fn context(&self) -> ObservedContext {
        self.context
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn destination(&self) -> Ipv4Addr {
        self.destination
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn ip(&self) -> Ipv4Addr {
        self.ip
    }
    pub fn deadline(&self) -> u64 {
        self.deadline
    }
    pub fn table(&self) -> &'static str {
        TABLE
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    /// Model of independent readback after the matching atomic commit/ACK.
    /// The real adapter must validate kernel sender, sequence and live lease.
    Confirmed(Transaction),
    DefinitiveRejection,
    Unknown,
}

#[derive(Debug, Clone)]
struct Pending {
    transaction: Transaction,
    previous: Option<Entry>,
    index: Option<usize>,
}

#[derive(Debug)]
pub struct ModelBroker {
    scope: Scope,
    instance: u128,
    revision: u64,
    phase: Phase,
    entries: [Option<Entry>; CAPACITY],
    pending: Option<Pending>,
    cursor: usize,
    last_now: u64,
    last_change: Option<u64>,
}

impl ModelBroker {
    /// Offline only: table_absent is a *simulated observation*, not an owner
    /// marker or proof that the live kernel has been checked.
    pub fn start(scope: Scope, instance: u128, table_absent: bool) -> Result<Self, Error> {
        if instance == 0 {
            return Err(Error::Instance);
        }
        if !table_absent {
            return Err(Error::TablePresent);
        }
        Ok(Self {
            scope,
            instance,
            revision: 0,
            phase: Phase::Ready,
            entries: [None; CAPACITY],
            pending: None,
            cursor: 0,
            last_now: 0,
            last_change: None,
        })
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn occupied(&self) -> usize {
        self.entries.iter().flatten().count()
    }
    pub fn entries(&self) -> &[Option<Entry>; CAPACITY] {
        &self.entries
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn instance(&self) -> u128 {
        self.instance
    }

    fn ready(&self) -> Result<(), Error> {
        match self.phase {
            Phase::Ready => Ok(()),
            Phase::Pending => Err(Error::Busy),
            _ => Err(Error::NotReady),
        }
    }
    fn check(&self, seen: ObservedContext, r: Request, now: u64) -> Result<(), Error> {
        self.scope.authorize(seen)?;
        self.ready()?;
        if r.instance != self.instance {
            return Err(Error::Instance);
        }
        if r.revision != self.revision {
            return Err(Error::Revision);
        }
        if now < self.last_now {
            return Err(Error::Clock);
        }
        self.scope.target(r.ip)?;
        r.validate_ttl()?;
        if r.operation == Operation::Ban {
            now.checked_add(u64::from(r.ttl_secs) * 1_000_000_000)
                .ok_or(Error::Overflow)?;
            if !self
                .entries
                .iter()
                .any(|s| s.is_none() || s.is_some_and(|e| e.ip == r.ip))
            {
                return Err(Error::Capacity);
            }
        }
        Ok(())
    }
    /// No reservation, revision change, timer start or kernel operation.
    pub fn preview(
        &self,
        seen: ObservedContext,
        operation: Operation,
        ip: Ipv4Addr,
        ttl_secs: u32,
        now: u64,
    ) -> Result<[u8; FRAME_BYTES], Error> {
        let r = Request {
            operation,
            instance: self.instance,
            revision: self.revision,
            ip,
            ttl_secs,
        };
        self.check(seen, r, now)?;
        Ok(r.encode())
    }
    pub fn prepare(
        &mut self,
        seen: ObservedContext,
        frame: &[u8],
        now: u64,
    ) -> Result<Transaction, Error> {
        // Authorization is checked before parsing attacker-controlled bytes.
        self.scope.authorize(seen)?;
        let r = Request::decode(frame)?;
        self.check(seen, r, now)?;
        if self
            .last_change
            .is_some_and(|t| now.saturating_sub(t) < CHANGE_INTERVAL_NS)
        {
            return Err(Error::RateLimit);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)?;
        let index = self
            .entries
            .iter()
            .position(|s| s.is_some_and(|e| e.ip == r.ip));
        let index = if r.operation == Operation::Ban {
            Some(
                index
                    .or_else(|| self.entries.iter().position(Option::is_none))
                    .ok_or(Error::Capacity)?,
            )
        } else {
            index
        };
        let deadline = if r.operation == Operation::Ban {
            now.checked_add(u64::from(r.ttl_secs) * 1_000_000_000)
                .ok_or(Error::Overflow)?
        } else {
            0
        };
        let transaction = Transaction {
            operation: r.operation,
            context: self.scope.context,
            instance: self.instance,
            revision,
            destination: self.scope.destination,
            port: self.scope.port,
            ip: r.ip,
            deadline,
        };
        let previous = index.and_then(|i| self.entries[i]);
        if let Some(i) = index {
            // Track both the old possibly-longer lease and an unknown new lease.
            let horizon = if r.operation == Operation::Ban {
                deadline.max(previous.map_or(0, |e| e.deadline_boot_ns))
            } else {
                previous.map_or(0, |e| e.deadline_boot_ns)
            };
            self.entries[i] = Some(Entry {
                ip: r.ip,
                deadline_boot_ns: horizon,
                confirmed: false,
            });
        }
        self.revision = revision;
        self.last_now = now;
        self.last_change = Some(now);
        self.pending = Some(Pending {
            transaction: transaction.clone(),
            previous,
            index,
        });
        self.phase = Phase::Pending;
        Ok(transaction)
    }
    pub fn finish(&mut self, completion: Completion) -> Result<(), Error> {
        if self.phase != Phase::Pending {
            return Err(Error::NotReady);
        }
        let pending = self.pending.as_ref().ok_or(Error::Receipt)?;
        match completion {
            Completion::Confirmed(ref observed) if observed == &pending.transaction => {
                if let Some(i) = pending.index {
                    self.entries[i] = match observed.operation {
                        Operation::Ban => Some(Entry {
                            ip: observed.ip,
                            deadline_boot_ns: observed.deadline,
                            confirmed: true,
                        }),
                        Operation::Unban => None,
                    };
                }
                self.phase = Phase::Ready;
                self.pending = None;
                Ok(())
            }
            Completion::DefinitiveRejection => {
                if let Some(i) = pending.index {
                    self.entries[i] = pending.previous;
                }
                // Revision remains consumed, even when the transaction was rejected.
                self.phase = Phase::Ready;
                self.pending = None;
                Err(Error::Rejected)
            }
            Completion::Unknown => {
                self.phase = Phase::Quarantined;
                Err(Error::Unknown)
            }
            Completion::Confirmed(_) => {
                self.phase = Phase::Quarantined;
                Err(Error::Receipt)
            }
        }
    }
    /// Userspace bookkeeping only. An absolute-deadline *reference kernel*
    /// already passes expired entries. This is not nft TTL validation.
    pub fn sweep(&mut self, now: u64) -> Result<usize, Error> {
        self.ready()?;
        if now < self.last_now {
            return Err(Error::Clock);
        }
        self.last_now = now;
        for _ in 0..SWEEP_BUDGET {
            let i = self.cursor;
            self.cursor = (i + 1) % CAPACITY;
            if self.entries[i].is_some_and(|e| now >= e.deadline_boot_ns) {
                self.entries[i] = None;
            }
        }
        Ok(SWEEP_BUDGET)
    }
    /// Separate supervisor/owner-socket lifecycle event, not an ordinary CLI
    /// disconnect. It never emits flush-ruleset or deletes an unknown table.
    pub fn close_owner(&mut self, seen: ObservedContext) -> Result<(), Error> {
        self.scope.authorize(seen)?;
        if self.phase == Phase::Stopped {
            return Err(Error::NotReady);
        }
        self.phase = Phase::Closing;
        Ok(())
    }
    pub fn observe_absent_after_close(
        &mut self,
        seen: ObservedContext,
        absent: bool,
    ) -> Result<(), Error> {
        self.scope.authorize(seen)?;
        if self.phase != Phase::Closing {
            return Err(Error::NotReady);
        }
        if !absent {
            return Err(Error::TablePresent);
        }
        self.entries = [None; CAPACITY];
        self.pending = None;
        self.phase = Phase::Stopped;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DeploymentEvidence {
    pub isolated_namespace: bool,
    pub os_peer_credentials_verified: bool,
    pub capabilities_confined: bool,
    pub owner_without_persist_verified: bool,
    pub absolute_boottime_deadline_verified: bool,
    pub recovery_and_packet_tests_verified: bool,
}
impl DeploymentEvidence {
    pub fn missing(self) -> Vec<&'static str> {
        let mut result = Vec::new();
        for (ok, reason) in [
            (self.isolated_namespace, "namespace-isolation"),
            (self.os_peer_credentials_verified, "OS-peer-credentials"),
            (self.capabilities_confined, "privilege-confinement"),
            (
                self.owner_without_persist_verified,
                "owner-socket-lifecycle",
            ),
            (
                self.absolute_boottime_deadline_verified,
                "absolute-CLOCK_BOOTTIME-deadline",
            ),
            (
                self.recovery_and_packet_tests_verified,
                "kernel-packets-and-recovery",
            ),
        ] {
            if !ok {
                result.push(reason);
            }
        }
        result
    }
}

/// Pure calculations used to reproduce a naive relative-timeout conversion.
/// No measured OS latency or kernel clock is implied by these inputs.
pub fn relative_deadline_at_kernel_init(
    kernel_init_boot_ns: u64,
    relative_ns: u64,
) -> Result<u64, Error> {
    kernel_init_boot_ns
        .checked_add(relative_ns)
        .ok_or(Error::Overflow)
}

#[cfg(test)]
mod private_tests {
    use super::*;
    #[test]
    fn exhausted_revision_never_wraps() {
        let context = ObservedContext {
            uid: 1001,
            namespace: 12,
            owner_socket: 42,
        };
        let scope = Scope::new(Ipv4Addr::new(10, 99, 0, 66), 18443, context, &[]).unwrap();
        let mut broker = ModelBroker::start(scope, 1, true).unwrap();
        broker.revision = u64::MAX;
        let frame = broker
            .preview(context, Operation::Ban, Ipv4Addr::new(10, 99, 0, 1), 30, 0)
            .unwrap();
        assert_eq!(
            broker.prepare(context, &frame, 0),
            Err(Error::RevisionExhausted)
        );
        assert_eq!(broker.phase(), Phase::Ready);
        assert_eq!(broker.occupied(), 0);
    }
}
