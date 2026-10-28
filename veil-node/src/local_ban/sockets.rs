//! Linux-local runtime ownership, separate from BAN state. All names are fixed.
//! Only a recorded inode kept alive by its anchor can be reclaimed after death.
use anyhow::{ensure, Context, Result};
use ring::digest;
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::{AsRawFd, FromRawFd},
    },
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::net::UnixListener;

pub const NORMAL: &str = "control.sock";
pub const EMERGENCY: &str = "emergency.sock";
pub const PROOF: &str = "socket-owner";
const NEXT: &str = ".socket-owner.next";
const ANCHORS: [&str; 2] = [".control.owned.sock", ".emergency.owned.sock"];
const NAMES: [&str; 2] = [NORMAL, EMERGENCY];
const MAGIC: &[u8; 8] = b"VLBOWN01";
const PROOF_LEN: usize = 72;

pub fn validate_dir(dir: &Path) -> Result<()> {
    validate_directory(
        &std::fs::symlink_metadata(dir)
            .context("Create a private existing control directory (mode 0700)")?,
    )
}
fn uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}
fn validate_directory(m: &Metadata) -> Result<()> {
    ensure!(
        m.is_dir() && m.uid() == uid() && m.mode() & 0o777 == 0o700,
        "Control directory must be owned by this user with mode 0700"
    );
    Ok(())
}
struct Lease {
    directory: File,
}
impl Lease {
    fn open(dir: &Path) -> Result<Arc<Self>> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir)?;
        validate_directory(&directory.metadata()?)?;
        Ok(Arc::new(Self { directory }))
    }
    fn acquire(dir: &Path) -> Result<Arc<Self>> {
        let lease = Self::open(dir)?;
        // SAFETY: live directory descriptor; nonblocking advisory lock, held by
        // this open file description until every guard has closed it.
        ensure!(
            unsafe { libc::flock(lease.directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "CONTROL_DIRECTORY_BUSY: {}",
            std::io::Error::last_os_error()
        );
        Ok(lease)
    }
    fn path(&self, name: &str) -> PathBuf {
        // Pin every operation to the opened directory, even if an ancestor is
        // renamed. Endpoints/metadata themselves are never followed as symlinks.
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd())).join(name)
    }
    fn stat(&self, name: &str) -> Result<Option<Metadata>> {
        match std::fs::symlink_metadata(self.path(name)) {
            Ok(m) => Ok(Some(m)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
}
impl Identity {
    fn from(m: &Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
        }
    }
    fn matches(self, m: &Metadata) -> bool {
        m.file_type().is_socket()
            && m.uid() == uid()
            && m.mode() & 0o777 == 0o600
            && Self::from(m) == self
    }
}
#[derive(Clone, Copy)]
struct Proof([Identity; 2]);
impl Proof {
    fn encode(self) -> [u8; PROOF_LEN] {
        let mut b = [0; PROOF_LEN];
        b[..8].copy_from_slice(MAGIC);
        for (i, id) in self.0.iter().enumerate() {
            b[8 + i * 16..16 + i * 16].copy_from_slice(&id.dev.to_be_bytes());
            b[16 + i * 16..24 + i * 16].copy_from_slice(&id.ino.to_be_bytes());
        }
        let hash = digest::digest(&digest::SHA256, &b[..40]);
        b[40..].copy_from_slice(hash.as_ref());
        b
    }
    fn decode(b: &[u8; PROOF_LEN]) -> Result<Self> {
        ensure!(
            &b[..8] == MAGIC && digest::digest(&digest::SHA256, &b[..40]).as_ref() == &b[40..],
            "INVALID_SOCKET_PROOF"
        );
        let mut ids = [Identity { dev: 0, ino: 0 }; 2];
        for (i, id) in ids.iter_mut().enumerate() {
            id.dev = u64::from_be_bytes(b[8 + i * 16..16 + i * 16].try_into()?);
            id.ino = u64::from_be_bytes(b[16 + i * 16..24 + i * 16].try_into()?);
        }
        Ok(Self(ids))
    }
}
fn read_proof(lease: &Lease) -> Result<Option<(Proof, Identity)>> {
    let Some(before) = lease.stat(PROOF)? else {
        return Ok(None);
    };
    ensure!(
        before.is_file()
            && before.uid() == uid()
            && before.mode() & 0o777 == 0o600
            && before.nlink() == 1
            && before.len() == PROOF_LEN as u64,
        "UNRECOGNIZED_SOCKET_PROOF"
    );
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(lease.path(PROOF))?;
    let m = f.metadata()?;
    ensure!(
        Identity::from(&before) == Identity::from(&m) && m.is_file() && m.len() == PROOF_LEN as u64,
        "SOCKET_PROOF_CHANGED"
    );
    let mut b = [0; PROOF_LEN];
    f.read_exact(&mut b)?;
    Ok(Some((Proof::decode(&b)?, Identity::from(&m))))
}
fn publish(lease: &Lease, proof: Proof, previous: Option<Identity>) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(lease.path(NEXT))?;
    #[cfg(test)]
    test_checkpoint("next_created");
    f.write_all(&proof.encode())?;
    #[cfg(test)]
    test_checkpoint("next_written");
    // Atomic publication is for process death, not a durable policy/WAL. No
    // fsync is needed to survive SIGKILL of this process; corrupt/partial proof
    // after storage/host failure is preserved and rejected on the next start.
    let current = lease.stat(PROOF)?;
    ensure!(
        match (previous, current) {
            (None, None) => true,
            (Some(id), Some(m)) => m.is_file() && Identity::from(&m) == id,
            _ => false,
        },
        "SOCKET_PROOF_CHANGED"
    );
    std::fs::rename(lease.path(NEXT), lease.path(PROOF))?;
    #[cfg(test)]
    test_checkpoint("proof_published");
    Ok(())
}
fn require_dead(path: &Path) -> Result<()> {
    // Nonblocking connect: success, backlog saturation, permission errors and
    // every uncertainty refuse recovery. Only ECONNREFUSED permits reclaim.
    // SAFETY: valid domain/type; returned descriptor is immediately owned.
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    ensure!(raw >= 0, "Socket probe creation failed");
    // SAFETY: raw is a newly created, exclusively owned descriptor.
    let fd = unsafe { File::from_raw_fd(raw) };
    // SAFETY: zeroed sockaddr_un is valid to initialize before connect.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    ensure!(bytes.len() < address.sun_path.len(), "Probe path too long");
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    let len =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    // SAFETY: initialized AF_UNIX sockaddr with checked length and live fd.
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            len,
        )
    };
    let error = std::io::Error::last_os_error();
    ensure!(
        result == -1 && error.raw_os_error() == Some(libc::ECONNREFUSED),
        "SOCKET_NOT_PROVEN_DEAD"
    );
    Ok(())
}
fn validate_pair(lease: &Lease, index: usize, id: Identity) -> Result<bool> {
    let public = lease.stat(NAMES[index])?;
    let anchor = lease.stat(ANCHORS[index])?;
    match (public, anchor) {
        (None, None) => Ok(false),
        (public, Some(anchor)) => {
            ensure!(
                id.matches(&anchor) && anchor.nlink() == if public.is_some() { 2 } else { 1 },
                "SOCKET_OWNERSHIP_MISMATCH"
            );
            if let Some(m) = public {
                ensure!(id.matches(&m), "SOCKET_OWNERSHIP_MISMATCH");
            }
            require_dead(&lease.path(ANCHORS[index]))?;
            Ok(true)
        }
        _ => anyhow::bail!("SOCKET_ANCHOR_MISSING"),
    }
}
fn remove_identity(lease: &Lease, name: &str, id: Identity) -> Result<()> {
    if let Some(m) = lease.stat(name)? {
        ensure!(id.matches(&m), "SOCKET_CHANGED_BEFORE_RECLAIM");
        // The user authorized recovery of this instance's verified sockets.
        std::fs::remove_file(lease.path(name))?;
    }
    Ok(())
}
struct SocketOwner {
    lease: Arc<Lease>,
    index: usize,
    id: Identity,
    cleanup: bool,
}
impl Drop for SocketOwner {
    fn drop(&mut self) {
        if self.cleanup {
            // Keep the anchor until the public name has been removed. Never
            // unlink a path whose identity no longer matches our own creation.
            if remove_identity(&self.lease, NAMES[self.index], self.id).is_ok() {
                let _ = remove_identity(&self.lease, ANCHORS[self.index], self.id);
            }
        }
    }
}
pub struct Owners {
    _normal: SocketOwner,
    _emergency: SocketOwner,
}
fn bind(lease: Arc<Lease>, index: usize, backlog: i32) -> Result<(UnixListener, SocketOwner)> {
    let path = lease.path(NAMES[index]);
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let id = Identity::from(&std::fs::symlink_metadata(&path)?);
    let owner = SocketOwner {
        lease,
        index,
        id,
        cleanup: true,
    };
    #[cfg(test)]
    test_checkpoint(if index == 0 {
        "normal_bound"
    } else {
        "emergency_bound"
    });
    std::fs::hard_link(&path, owner.lease.path(ANCHORS[index]))?;
    #[cfg(test)]
    test_checkpoint(if index == 0 {
        "normal_anchored"
    } else {
        "emergency_anchored"
    });
    // SAFETY: live listening Unix socket; bound backlog, then Tokio ownership.
    ensure!(
        unsafe { libc::listen(listener.as_raw_fd(), backlog) } == 0,
        "listen backlog failed"
    );
    listener.set_nonblocking(true)?;
    Ok((UnixListener::from_std(listener)?, owner))
}
pub struct ControlSockets {
    pub normal: UnixListener,
    pub emergency: UnixListener,
    pub owners: Owners,
}
impl ControlSockets {
    pub fn bind(dir: &Path) -> Result<Self> {
        let lease = Lease::acquire(dir)?;
        #[cfg(test)]
        test_checkpoint("lease_acquired");
        // A partial publication has no trustworthy ownership proof. Preserve
        // it and refuse; never guess that an unfamiliar temporary file is ours.
        ensure!(
            lease.stat(NEXT)?.is_none(),
            "INCOMPLETE_SOCKET_PROOF_PRESERVED"
        );
        let previous = read_proof(&lease)?;
        if let Some((proof, _)) = previous {
            // Validate BOTH pairs before removing either one.
            let reclaim = [
                validate_pair(&lease, 0, proof.0[0])?,
                validate_pair(&lease, 1, proof.0[1])?,
            ];
            for (i, yes) in reclaim.into_iter().enumerate() {
                if yes {
                    remove_identity(&lease, NAMES[i], proof.0[i])?;
                    remove_identity(&lease, ANCHORS[i], proof.0[i])?;
                }
            }
        } else {
            for name in NAMES.into_iter().chain(ANCHORS) {
                ensure!(lease.stat(name)?.is_none(), "UNOWNED_SOCKET_PATH_PRESERVED");
            }
        }
        let (normal, normal_owner) = bind(lease.clone(), 0, 8)?;
        let (emergency, emergency_owner) = bind(lease.clone(), 1, 1)?;
        publish(
            &lease,
            Proof([normal_owner.id, emergency_owner.id]),
            previous.map(|(_, id)| id),
        )?;
        Ok(Self {
            normal,
            emergency,
            owners: Owners {
                _normal: normal_owner,
                _emergency: emergency_owner,
            },
        })
    }
}

/// A diagnostic snapshot, not permission to reclaim. No bind, connect, unlink,
/// rename or file writes. A free advisory lock does NOT prove a socket dead.
pub fn inspect(dir: &Path) -> Result<String> {
    let lease = Lease::open(dir)?;
    // SAFETY: live descriptor; nonblocking advisory lock only. If successful,
    // keep the snapshot lock until this function returns, then release by Drop.
    let acquired =
        unsafe { libc::flock(lease.directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    let lock = if acquired == 0 {
        "available"
    } else {
        let e = std::io::Error::last_os_error();
        ensure!(
            e.kind() == std::io::ErrorKind::WouldBlock,
            "Inspection lock failed: {e}"
        );
        "busy"
    };
    let mut report = format!("INSPECTED\nsnapshot_only=true\nrecovery_attempted=false\nsocket_liveness=not_probed\nowner_lock={lock}\n");
    match read_proof(&lease) {
        Ok(Some(_)) => report += "owner_record=valid\n",
        Ok(None) => report += "owner_record=missing\n",
        Err(e) => report += &format!("owner_record=invalid\nowner_record_error={e}\n"),
    }
    report += if lease.stat(NEXT)?.is_some() {
        "pending_record=present\n"
    } else {
        "pending_record=missing\n"
    };
    for name in NAMES.into_iter().chain(ANCHORS).chain([PROOF, NEXT]) {
        match lease.stat(name)? {
            None => report += &format!("path.{name}=missing\n"),
            Some(m) => {
                let kind = if m.file_type().is_socket() {
                    "socket"
                } else if m.is_file() {
                    "file"
                } else if m.file_type().is_symlink() {
                    "symlink"
                } else {
                    "other"
                };
                report += &format!(
                    "path.{name}={kind},uid={},mode={:04o},dev={},ino={},links={},bytes={}\n",
                    m.uid(),
                    m.mode() & 0o777,
                    m.dev(),
                    m.ino(),
                    m.nlink(),
                    m.len()
                );
            }
        }
    }
    Ok(report)
}

// No production feature, environment switch or stopping code is compiled in.
// Only the explicit ignored fixture in the library test executable uses this.
#[cfg(test)]
pub(super) fn test_checkpoint(stage: &str) {
    if std::env::var("VEIL_LOCAL_BAN_INIT_FIXTURE").as_deref() == Ok("1")
        && std::env::var("VEIL_LOCAL_BAN_INIT_STAGE").as_deref() == Ok(stage)
    {
        eprintln!("VEIL_LOCAL_BAN_INIT_STAGE={stage}");
        // SAFETY: stop only this explicitly selected test-fixture process.
        assert_eq!(unsafe { libc::raise(libc::SIGSTOP) }, 0);
    }
}

#[cfg(test)]
mod tests;
