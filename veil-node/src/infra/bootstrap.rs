//! One-use permission to create the first WAL, issued only with a new signing key.
//! This is a local crash-safety guard, not protection against restored/cloned disks.
use crate::adapter::NodeCryptoIdentity;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

fn permit_path(key: &Path) -> PathBuf {
    let mut name = key.as_os_str().to_owned();
    name.push(".bootstrap");
    name.into()
}

fn permission(identity: &NodeCryptoIdentity) -> Vec<u8> {
    let public: String = identity
        .public_key_bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("VEIL_INITIAL_STATE_V1\n{}\n{public}\n", identity.node_id).into_bytes()
}

fn sync_parent(path: &Path) -> io::Result<()> {
    File::open(
        std::path::absolute(path)?
            .parent()
            .expect("absolute parent"),
    )?
    .sync_all()
}

/// Reject symlinks, unsafe modes and replacement between lstat/open. Keep the
/// returned lock alive for the entire daemon, including initialization/recovery.
pub fn lock_private_file(path: &Path) -> io::Result<File> {
    let before = std::fs::symlink_metadata(path)?;
    if !before.is_file() || before.permissions().mode() & 0o777 != 0o600 {
        return Err(io::Error::other(
            "State identity files must be regular files with mode 0600",
        ));
    }
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let after = file.metadata()?;
    if before.dev() != after.dev() || before.ino() != after.ino() {
        return Err(io::Error::other(
            "State identity file changed while opening",
        ));
    }
    file.try_lock()
        .map_err(|e| io::Error::other(format!("Signing identity/state already in use: {e}")))?;
    Ok(file)
}

pub fn provision_key(node: &str, path: &Path) -> io::Result<NodeCryptoIdentity> {
    // Refuse an old permit even if its key has disappeared. No repair/reset mode.
    match std::fs::symlink_metadata(permit_path(path)) {
        Ok(_) => {
            return Err(io::Error::other(
                "Bootstrap record already exists; preserve identity state",
            ))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let absolute = std::path::absolute(path)?;
    let mut missing = Vec::new();
    let mut parent = absolute.parent().expect("absolute parent");
    while !parent.try_exists()? {
        missing.push(parent.to_owned());
        parent = parent
            .parent()
            .ok_or_else(|| io::Error::other("Missing key ancestor"))?;
    }
    for directory in missing.iter().rev() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(directory)?;
        sync_parent(directory)?;
    }
    let identity = NodeCryptoIdentity::create_new_file(node.into(), path)?;
    sync_parent(path)?;
    let permit = permit_path(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&permit)?;
    file.write_all(&permission(&identity))?;
    file.sync_all()?;
    sync_parent(&permit)?;
    Ok(identity)
}

/// Consume before creating any usable WAL. A crash/failed fsync leaves this
/// identity unavailable for reinitialization; never truncate or reset the record.
pub fn consume_permission(key: &Path, identity: &NodeCryptoIdentity) -> io::Result<()> {
    let path = permit_path(key);
    let mut file = lock_private_file(&path)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(1024).read_to_end(&mut bytes)?;
    if bytes != permission(identity) {
        return Err(io::Error::other("Initial-state permission consumed, mismatched or damaged; never reinitialize a used signing key"));
    }
    file.write_all(b"CLAIMED\n")?;
    file.sync_all()?;
    sync_parent(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::wal::FileWal, usecase::ConsensusPersistence};

    fn directory() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "veil-bootstrap-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn permit_is_one_use_and_cannot_be_reissued_for_existing_key() {
        let root = directory();
        let key = root.join("new/identity.pk8");
        let identity = provision_key("a", &key).unwrap();
        let original = std::fs::read(&key).unwrap();
        consume_permission(&key, &identity).unwrap();
        let claimed = std::fs::read(permit_path(&key)).unwrap();
        assert!(consume_permission(&key, &identity).is_err());
        assert!(provision_key("a", &key).is_err());
        assert_eq!(std::fs::read(&key).unwrap(), original);
        assert_eq!(std::fs::read(permit_path(&key)).unwrap(), claimed);
        // An older key without a permit is not proof of an unused identity.
        let legacy = root.join("legacy.pk8");
        let legacy_id = NodeCryptoIdentity::create_new_file("a".into(), &legacy).unwrap();
        assert!(consume_permission(&legacy, &legacy_id).is_err());
        assert!(provision_key("a", &legacy).is_err());
        assert!(!permit_path(&legacy).exists());
    }

    #[test]
    fn mismatched_partial_and_unsafe_permits_fail_closed() {
        let root = directory();
        let key = root.join("a.pk8");
        let identity = provision_key("a", &key).unwrap();
        let other = NodeCryptoIdentity::generate_random("a");
        assert!(consume_permission(&key, &other).is_err());
        let permit = permit_path(&key);
        std::fs::set_permissions(&permit, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(consume_permission(&key, &identity).is_err());
        std::fs::set_permissions(&permit, std::fs::Permissions::from_mode(0o600)).unwrap();
        // A partial claim after a crash must not become a fresh permission.
        OpenOptions::new()
            .append(true)
            .open(&permit)
            .unwrap()
            .write_all(b"C")
            .unwrap();
        assert!(consume_permission(&key, &identity).is_err());
    }

    #[test]
    fn identity_lock_excludes_other_wal_paths_and_symlinks() {
        let root = directory();
        let key = root.join("a.pk8");
        provision_key("a", &key).unwrap();
        let guard = lock_private_file(&key).unwrap();
        assert!(lock_private_file(&key).is_err());
        let alias = root.join("alias.pk8");
        std::fs::hard_link(&key, &alias).unwrap();
        assert!(lock_private_file(&alias).is_err());
        let link = root.join("symlink.pk8");
        std::os::unix::fs::symlink(&key, &link).unwrap();
        assert!(lock_private_file(&link).is_err());
        drop(guard);
        assert!(lock_private_file(&key).is_ok());
    }

    #[test]
    fn existing_wal_never_creates_missing_state_or_overwrites_during_bootstrap() {
        let root = directory();
        let absent = root.join("absent/state.wal");
        assert!(FileWal::open_existing(&absent).is_err());
        assert!(!root.join("absent").exists());
        let wal = root.join("node.wal");
        let store = FileWal::create_new(&wal).unwrap();
        assert!(FileWal::open_existing(&wal).is_err()); // empty/incomplete
        store.persist(b"saved state").unwrap();
        drop(store);
        assert!(FileWal::create_new(&wal).is_err());
        assert_eq!(std::fs::read(&wal).unwrap(), b"saved state");
        let store = FileWal::open_existing(&wal).unwrap();
        // Preserve rather than delete the fixture to simulate loss at its path.
        std::fs::rename(&wal, root.join("retained.wal")).unwrap();
        assert!(store.persist(b"must not recreate").is_err());
        assert!(!wal.exists());
    }

    #[test]
    fn failed_initial_wal_creation_does_not_restore_permission() {
        let root = directory();
        let key = root.join("a.pk8");
        let identity = provision_key("a", &key).unwrap();
        let wal = root.join("node.wal");
        let store = FileWal::create_new(&wal).unwrap();
        store.persist(b"existing").unwrap();
        drop(store);
        consume_permission(&key, &identity).unwrap();
        assert!(FileWal::create_new(&wal).is_err());
        assert!(consume_permission(&key, &identity).is_err());
        assert_eq!(std::fs::read(wal).unwrap(), b"existing");
    }
}
