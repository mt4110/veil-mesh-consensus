//! Test-only full-snapshot append container. Not a daemon storage format.
//! Failed writes poison the writer; recovery refuses partial tails and pending
//! compactions. Payload validation remains the consensus engine's responsibility.
use super::wal::FileWal;
use crate::usecase::ConsensusPersistence;
use ring::digest::{digest, SHA256};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

const MAGIC: &[u8; 8] = b"VLSNAP01";
const HEADER: usize = 8 + 8 + 8 + 8 + 32;
const OVERHEAD: usize = HEADER + 32;
const MAX_FRAMES: u64 = 16;
static SERIAL: AtomicU64 = AtomicU64::new(0);

fn error() -> io::Error {
    io::Error::other("Invalid/incomplete prototype WAL; preserve and refuse recovery")
}
fn private() -> OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut o = OpenOptions::new();
    o.read(true).write(true).mode(0o600);
    o
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    digest(&SHA256, bytes).as_ref().try_into().unwrap()
}
fn frame(generation: u64, seq: u64, previous: [u8; 32], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + OVERHEAD);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&previous);
    out.extend_from_slice(payload);
    out.extend_from_slice(&hash(&out));
    out
}
#[derive(Default)]
struct Position {
    generation: u64,
    seq: u64,
    previous: [u8; 32],
    bytes: u64,
    frames: u64,
    failed: bool,
}
fn decode(bytes: &[u8], limit: u64) -> io::Result<(Position, Vec<u8>)> {
    if bytes.is_empty() || bytes.len() as u64 > limit {
        return Err(error());
    }
    let mut pos = Position::default();
    let mut offset = 0;
    let mut latest = Vec::new();
    while offset < bytes.len() {
        let tail = &bytes[offset..];
        if tail.len() < OVERHEAD || &tail[..8] != MAGIC {
            return Err(error());
        }
        let number = |at| u64::from_le_bytes(tail[at..at + 8].try_into().unwrap());
        let generation = number(8);
        let seq = number(16);
        let len = number(24);
        let len: usize = len.try_into().map_err(|_| error())?;
        let end = HEADER.checked_add(len).ok_or_else(error)?;
        let total = end.checked_add(32).ok_or_else(error)?;
        if len == 0
            || total > tail.len()
            || total as u64 > limit
            || seq == 0
            || generation == 0
            || tail[32..HEADER] != pos.previous
            || hash(&tail[..end]) != tail[end..total]
        {
            return Err(error());
        }
        if pos.frames > 0 && (generation != pos.generation || pos.seq.checked_add(1) != Some(seq)) {
            return Err(error());
        }
        pos.generation = generation;
        pos.seq = seq;
        pos.previous = tail[end..total].try_into().unwrap();
        pos.frames += 1;
        if pos.frames > MAX_FRAMES {
            return Err(error());
        }
        latest = tail[HEADER..end].to_vec();
        offset += total;
    }
    pos.bytes = bytes.len() as u64;
    Ok((pos, latest))
}

pub(crate) struct AppendWal {
    path: PathBuf,
    limit: u64,
    position: Mutex<Position>,
    _guard: FileWal,
}
impl AppendWal {
    pub(crate) fn create_new(path: &Path, limit: u64) -> io::Result<Self> {
        let guard = FileWal::open(path)?; // stable owner lock and pending-temp refusal
        let path = std::path::absolute(path)?;
        let f = private().create_new(true).open(&path)?;
        f.sync_all()?;
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok(Self {
            path,
            limit,
            position: Mutex::new(Position::default()),
            _guard: guard,
        })
    }
    pub(crate) fn open_existing(path: &Path, limit: u64) -> io::Result<(Self, Vec<u8>)> {
        let guard = FileWal::open_existing(path)?;
        let path = std::path::absolute(path)?;
        if File::open(&path)?.metadata()?.len() > limit {
            return Err(error());
        }
        let mut bytes = Vec::new();
        File::open(&path)?
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)?;
        let (position, latest) = decode(&bytes, limit)?;
        // A complete frame can survive a process exit only in page cache.
        // Seal adopted state before the caller can release any signed vote.
        private().open(&path)?.sync_all()?;
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok((
            Self {
                path,
                limit,
                position: Mutex::new(position),
                _guard: guard,
            },
            latest,
        ))
    }
    pub(crate) fn persist_observed(
        &self,
        bytes: &[u8],
        mut observe: impl FnMut(&'static str),
    ) -> io::Result<()> {
        self.persist_hook(bytes, |stage| {
            observe(stage);
            Ok(())
        })
    }
    fn persist_hook(
        &self,
        payload: &[u8],
        mut hook: impl FnMut(&'static str) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut p = self.position.lock().map_err(|_| error())?;
        if p.failed {
            return Err(error());
        }
        let result = (|| {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::symlink_metadata(&self.path)?;
            if !meta.is_file()
                || meta.permissions().mode() & 0o777 != 0o600
                || meta.len() != p.bytes
            {
                return Err(error());
            }
            if payload.is_empty()
                || payload.len().checked_add(OVERHEAD).ok_or_else(error)? as u64 > self.limit
            {
                return Err(error());
            }
            let seq = p.seq.checked_add(1).ok_or_else(error)?;
            let size = (payload.len() + OVERHEAD) as u64;
            let compact =
                p.frames == MAX_FRAMES || p.bytes.checked_add(size).ok_or_else(error)? > self.limit;
            let generation = if p.frames == 0 || compact {
                p.generation.checked_add(1).ok_or_else(error)?
            } else {
                p.generation
            };
            let bytes = frame(
                generation,
                seq,
                if compact { [0; 32] } else { p.previous },
                payload,
            );
            if compact {
                let temp = PathBuf::from(format!(
                    "{}.{}-{}.tmp",
                    self.path.display(),
                    std::process::id(),
                    SERIAL.fetch_add(1, Ordering::Relaxed)
                ));
                hook("create")?;
                let mut f = private().create_new(true).open(&temp)?;
                hook("write")?;
                f.write_all(&bytes)?;
                hook("after-write")?;
                hook("file-sync")?;
                f.sync_all()?;
                hook("after-file-sync")?;
                hook("rename")?;
                std::fs::rename(&temp, &self.path)?;
                hook("after-rename")?;
                hook("directory-sync")?;
                File::open(self.path.parent().unwrap())?.sync_all()?;
                hook("after-directory-sync")?;
            } else {
                // Reopening after compaction cannot accidentally append to the retired inode.
                let mut f = private().append(true).open(&self.path)?;
                if f.metadata()?.len() != p.bytes {
                    return Err(error());
                }
                hook("write")?;
                f.write_all(&bytes)?;
                hook("after-write")?;
                hook("file-sync")?;
                f.sync_all()?;
                hook("after-file-sync")?;
            }
            p.generation = generation;
            p.seq = seq;
            p.previous = bytes[bytes.len() - 32..].try_into().unwrap();
            p.bytes = if compact { size } else { p.bytes + size };
            p.frames = if compact { 1 } else { p.frames + 1 };
            Ok(())
        })();
        if result.is_err() {
            p.failed = true;
        }
        result
    }
}
impl ConsensusPersistence for AppendWal {
    fn persist(&self, snapshot: &[u8]) -> io::Result<()> {
        self.persist_observed(snapshot, |_| {})
    }
}

#[cfg(test)]
mod tests;
