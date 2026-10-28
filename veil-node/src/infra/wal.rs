//! Snapshot storage for a single node in an operator-owned directory.
//! The stable lock file is kept across snapshot renames and process restarts.
use crate::usecase::ConsensusPersistence;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct FileWal {
    path: PathBuf,
    require_existing: bool,
    max_bytes: u64,
    writer: std::sync::Mutex<()>,
    // Dropping the file releases the OS lock; no lock-file deletion is needed.
    _lock: File,
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn validate_file(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err(io::Error::other("WAL and lock must be regular files"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o777 != 0o600 {
                    return Err(io::Error::other("WAL and lock require mode 0600"));
                }
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn refuse_pending_snapshot(path: &Path) -> io::Result<()> {
    let prefix = format!(
        "{}.",
        path.file_name()
            .ok_or_else(|| io::Error::other("Missing WAL name"))?
            .to_string_lossy()
    );
    for entry in std::fs::read_dir(path.parent().expect("absolute WAL parent"))? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some(suffix) = name
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(".tmp"))
        {
            if let Some((pid, serial)) = suffix.split_once('-') {
                if !pid.is_empty()
                    && !serial.is_empty()
                    && pid.bytes().all(|c| c.is_ascii_digit())
                    && serial.bytes().all(|c| c.is_ascii_digit())
                {
                    return Err(io::Error::other("Incomplete WAL snapshot retained; preserve and quarantine it before reopening the authoritative WAL"));
                }
            }
        }
    }
    Ok(())
}

impl FileWal {
    pub fn with_limit(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// Daemon recovery never creates missing state or directories.
    pub fn open_existing(path: &Path) -> io::Result<Self> {
        validate_file(path)?;
        let meta = std::fs::symlink_metadata(path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("Existing WAL required; refusing empty recovery: {e}"),
            )
        })?;
        if !meta.is_file() || meta.len() == 0 {
            return Err(io::Error::other(
                "Existing nonempty WAL required; restore durable state",
            ));
        }
        let mut store = Self::open(path)?;
        store.require_existing = true;
        Ok(store)
    }

    /// Reserve a new WAL exclusively. Failure retains any partial file for diagnosis.
    pub fn create_new(path: &Path) -> io::Result<Self> {
        let mut store = Self::open(path)?;
        let file = private_options().create_new(true).open(path)?;
        file.sync_all()?;
        File::open(store.path.parent().expect("absolute parent"))?.sync_all()?;
        store.require_existing = true;
        Ok(store)
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("WAL requires parent"))?;
        let mut missing = Vec::new();
        let mut ancestor = parent;
        while !ancestor.try_exists()? {
            missing.push(ancestor.to_path_buf());
            ancestor = ancestor
                .parent()
                .ok_or_else(|| io::Error::other("Missing WAL ancestor"))?;
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
        // Persist directory creation as well as the later snapshot rename.
        for directory in missing.iter().rev() {
            File::open(directory.parent().expect("created directory parent"))?.sync_all()?;
        }
        validate_file(&path)?;
        let mut name = path.as_os_str().to_os_string();
        name.push(".lock");
        let lock_path = PathBuf::from(name);
        validate_file(&lock_path)?;
        let lock = private_options()
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock.try_lock()
            .map_err(|e| io::Error::other(format!("WAL already locked or unavailable: {e}")))?;
        refuse_pending_snapshot(&path)?;
        lock.sync_all()?;
        File::open(parent)?.sync_all()?;
        Ok(Self {
            path,
            require_existing: false,
            max_bytes: crate::domain::DEFAULT_MAX_WAL_BYTES,
            writer: std::sync::Mutex::new(()),
            _lock: lock,
        })
    }
}

impl ConsensusPersistence for FileWal {
    fn persist(&self, snapshot: &[u8]) -> io::Result<()> {
        if !super::latency::active() {
            return self.persist_observed(snapshot, |_| {});
        }
        let mut stage = "start";
        let mut started = std::time::Instant::now();
        let result = self.persist_observed(snapshot, |next| {
            super::latency::record(stage, started.elapsed(), snapshot.len(), true);
            stage = next;
            started = std::time::Instant::now();
        });
        if result.is_err() {
            super::latency::record(stage, started.elapsed(), snapshot.len(), false);
        }
        result
    }
}

impl FileWal {
    // Observer records operation boundaries only, never WAL contents. The
    // default path uses a no-op observer and exactly the same durability calls.
    pub(crate) fn persist_observed(
        &self,
        snapshot: &[u8],
        mut observe: impl FnMut(&'static str),
    ) -> io::Result<()> {
        observe("bounds");
        if snapshot.len() as u64 > self.max_bytes {
            return Err(io::Error::other(
                "WAL exceeds byte limit; original file retained",
            ));
        }
        observe("writer-lock");
        let _writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("WAL writer poisoned"))?;
        observe("validate");
        refuse_pending_snapshot(&self.path)?;
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        validate_file(&self.path)?;
        if self.require_existing {
            std::fs::symlink_metadata(&self.path)?;
        }
        observe("create");
        let (temp, mut file) = loop {
            let mut name = self.path.as_os_str().to_os_string();
            name.push(format!(
                ".{}-{}.tmp",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            let temp = PathBuf::from(name);
            match private_options().create_new(true).open(&temp) {
                Ok(file) => break (temp, file),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        // On failure retain the temporary file for diagnosis. Never remove a
        // pre-existing temporary path or truncate the last durable snapshot.
        observe("write");
        file.write_all(snapshot)?;
        observe("file-sync");
        file.sync_all()?;
        observe("rename");
        std::fs::rename(&temp, &self.path)?;
        observe("directory-open");
        let parent = File::open(self.path.parent().expect("absolute WAL parent"))?;
        observe("directory-sync");
        parent.sync_all()?;
        observe("done");
        Ok(())
    }
}
