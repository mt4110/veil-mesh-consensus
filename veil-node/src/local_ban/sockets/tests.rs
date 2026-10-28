use super::*;
use std::os::unix::{fs::symlink, net::UnixListener as Listener};

fn directory() -> PathBuf {
    let mut random = [0; 12];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut random).unwrap();
    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let p = std::env::temp_dir().join(format!("vlbs-{suffix}"));
    std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
    p
}
use std::os::unix::fs::DirBuilderExt;
fn crash(mut sockets: ControlSockets) {
    // Close the listener/lock FDs but skip the destructor's unlink, as SIGKILL
    // does. Actual process death is separately exercised on isolated Linux.
    sockets.owners._normal.cleanup = false;
    sockets.owners._emergency.cleanup = false;
    drop(sockets);
}
fn identity(p: impl AsRef<Path>) -> Identity {
    Identity::from(&std::fs::symlink_metadata(p).unwrap())
}
fn assert_owned_names(dir: &Path, exist: bool) {
    for name in NAMES.into_iter().chain(ANCHORS) {
        assert_eq!(dir.join(name).exists(), exist, "{name}");
    }
}

#[tokio::test]
async fn dead_recorded_sockets_are_reclaimed_but_proof_contains_no_ban_state() {
    let dir = directory();
    let first = ControlSockets::bind(&dir).unwrap();
    let original = [identity(dir.join(NORMAL)), identity(dir.join(EMERGENCY))];
    let b = std::fs::read(dir.join(PROOF)).unwrap();
    assert_eq!(b.len(), 72);
    let old = Proof::decode(&b.try_into().unwrap()).unwrap();
    assert_eq!(old.0, original);
    crash(first);
    assert_owned_names(&dir, true);
    let second = ControlSockets::bind(&dir).unwrap();
    let next = [identity(dir.join(NORMAL)), identity(dir.join(EMERGENCY))];
    // At least the generation proof is regenerated with the current inodes;
    // a filesystem is free to reuse an inode after the old anchor is removed.
    let proof = std::fs::read(dir.join(PROOF)).unwrap();
    assert_eq!(Proof::decode(&proof.try_into().unwrap()).unwrap().0, next);
    drop(second);
    assert_owned_names(&dir, false);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}
#[tokio::test]
async fn exclusive_directory_lease_preserves_live_daemon_and_is_cloexec() {
    let dir = directory();
    let first = ControlSockets::bind(&dir).unwrap();
    let id = identity(dir.join(NORMAL));
    let result = ControlSockets::bind(&dir);
    assert!(result.is_err());
    assert_eq!(identity(dir.join(NORMAL)), id);
    let fd = first.owners._normal.lease.directory.as_raw_fd();
    // SAFETY: live descriptor and read-only F_GETFD.
    assert_ne!(
        unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    assert!(std::os::unix::net::UnixStream::connect(dir.join(NORMAL)).is_ok());
    drop(first);
    assert_owned_names(&dir, false);
}
#[tokio::test]
async fn live_listener_is_preserved_even_when_directory_lock_is_free() {
    let dir = directory();
    let mut first = ControlSockets::bind(&dir).unwrap();
    let retained = first.normal.into_std().unwrap();
    first.owners._normal.cleanup = false;
    first.owners._emergency.cleanup = false;
    drop(first.emergency);
    drop(first.owners);
    let before = identity(dir.join(NORMAL));
    assert!(ControlSockets::bind(&dir).is_err());
    assert_eq!(identity(dir.join(NORMAL)), before);
    assert!(std::os::unix::net::UnixStream::connect(dir.join(NORMAL)).is_ok());
    drop(retained);
    drop(ControlSockets::bind(&dir).unwrap());
    assert_owned_names(&dir, false);
}
#[tokio::test]
async fn unknown_live_dead_regular_and_symlink_paths_are_preserved() {
    for kind in 0..4 {
        let dir = directory();
        let target = dir.join("unrelated-data");
        std::fs::write(&target, b"keep me").unwrap();
        let path = dir.join(NORMAL);
        let mut listener = None;
        match kind {
            0 => listener = Some(Listener::bind(&path).unwrap()),
            1 => drop(Listener::bind(&path).unwrap()),
            2 => std::fs::write(&path, b"foreign regular file").unwrap(),
            _ => symlink(&target, &path).unwrap(),
        }
        let before = identity(&path);
        assert!(ControlSockets::bind(&dir).is_err());
        assert_eq!(identity(&path), before);
        assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
        assert!(!dir.join(PROOF).exists());
        drop(listener);
    }
}
#[tokio::test]
async fn replacement_rejects_all_recovery_before_removing_other_valid_pair() {
    let dir = directory();
    crash(ControlSockets::bind(&dir).unwrap());
    let normal = identity(dir.join(NORMAL));
    std::fs::rename(dir.join(EMERGENCY), dir.join("preserved-original-socket")).unwrap();
    let foreign = Listener::bind(dir.join(EMERGENCY)).unwrap();
    let replaced = identity(dir.join(EMERGENCY));
    assert!(ControlSockets::bind(&dir).is_err());
    assert_eq!(identity(dir.join(NORMAL)), normal);
    assert_eq!(identity(dir.join(EMERGENCY)), replaced);
    assert!(std::os::unix::net::UnixStream::connect(dir.join(EMERGENCY)).is_ok());
    drop(foreign);
}
#[tokio::test]
async fn missing_public_name_with_valid_anchor_recovers_interrupted_reclaim() {
    let dir = directory();
    crash(ControlSockets::bind(&dir).unwrap());
    let lease = Lease::acquire(&dir).unwrap();
    let (proof, _) = read_proof(&lease).unwrap().unwrap();
    remove_identity(&lease, NORMAL, proof.0[0]).unwrap();
    drop(lease);
    drop(ControlSockets::bind(&dir).unwrap());
    assert_owned_names(&dir, false);
}
#[tokio::test]
async fn corrupt_symlink_hardlinked_or_incomplete_proof_preserves_endpoints() {
    for kind in 0..4 {
        let dir = directory();
        crash(ControlSockets::bind(&dir).unwrap());
        let before = identity(dir.join(NORMAL));
        match kind {
            0 => {
                let mut b = std::fs::read(dir.join(PROOF)).unwrap();
                b[71] ^= 1;
                std::fs::write(dir.join(PROOF), &b).unwrap();
            }
            1 => {
                std::fs::rename(dir.join(PROOF), dir.join("saved-proof")).unwrap();
                symlink(dir.join("saved-proof"), dir.join(PROOF)).unwrap();
            }
            2 => std::fs::hard_link(dir.join(PROOF), dir.join("extra-proof-link")).unwrap(),
            _ => std::fs::write(dir.join(NEXT), b"preserve incomplete file").unwrap(),
        }
        assert!(ControlSockets::bind(&dir).is_err());
        assert_eq!(identity(dir.join(NORMAL)), before);
    }
}
#[tokio::test]
async fn pinned_directory_cleanup_cannot_follow_replaced_parent_path() {
    let dir = directory();
    let first = ControlSockets::bind(&dir).unwrap();
    let moved = dir.with_extension("moved");
    std::fs::rename(&dir, &moved).unwrap();
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let foreign = Listener::bind(dir.join(NORMAL)).unwrap();
    let id = identity(dir.join(NORMAL));
    drop(first);
    assert_owned_names(&moved, false);
    assert_eq!(identity(dir.join(NORMAL)), id);
    drop(foreign);
}

#[tokio::test]
async fn lost_anchor_is_not_replaced_by_matching_old_inode_record() {
    let dir = directory();
    crash(ControlSockets::bind(&dir).unwrap());
    let before = identity(dir.join(NORMAL));
    let lease = Lease::acquire(&dir).unwrap();
    let (proof, _) = read_proof(&lease).unwrap().unwrap();
    // Remove only our verified anchor to exercise an interrupted/tampered
    // runtime directory. A numeric record alone must never authorize removal.
    remove_identity(&lease, ANCHORS[0], proof.0[0]).unwrap();
    drop(lease);
    assert!(ControlSockets::bind(&dir).is_err());
    assert_eq!(identity(dir.join(NORMAL)), before);
    assert!(dir.join(EMERGENCY).exists());
}

#[tokio::test]
async fn inspection_does_not_steal_a_live_lease_or_probe_listener() {
    let dir = directory();
    let first = ControlSockets::bind(&dir).unwrap();
    let proof = std::fs::read(dir.join(PROOF)).unwrap();
    let ids = [identity(dir.join(NORMAL)), identity(dir.join(EMERGENCY))];
    let report = inspect(&dir).unwrap();
    assert!(report.starts_with("INSPECTED\n"));
    assert!(report.contains("owner_lock=busy\n"));
    assert!(report.contains("socket_liveness=not_probed\n"));
    assert_eq!(std::fs::read(dir.join(PROOF)).unwrap(), proof);
    assert_eq!(
        [identity(dir.join(NORMAL)), identity(dir.join(EMERGENCY))],
        ids
    );
    assert!(ControlSockets::bind(&dir).is_err());
    let listener = first.normal.into_std().unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(listener);
    drop(first.emergency);
    drop(first.owners);
}
#[tokio::test]
async fn inspection_reports_corruption_and_pending_file_without_repairing_them() {
    let dir = directory();
    crash(ControlSockets::bind(&dir).unwrap());
    std::fs::write(dir.join(PROOF), b"invalid owner metadata").unwrap();
    std::fs::write(dir.join(NEXT), b"unfinished").unwrap();
    let before = [identity(dir.join(NORMAL)), identity(dir.join(EMERGENCY))];
    let report = inspect(&dir).unwrap();
    assert!(report.contains("owner_record=invalid\n"));
    assert!(report.contains("pending_record=present\n"));
    assert!(report.contains("recovery_attempted=false\n"));
    assert!(report.contains("owner_lock=available\n"));
    assert_eq!(
        std::fs::read(dir.join(PROOF)).unwrap(),
        b"invalid owner metadata"
    );
    assert_eq!(std::fs::read(dir.join(NEXT)).unwrap(), b"unfinished");
    assert_eq!(
        [identity(dir.join(NORMAL)), identity(dir.join(EMERGENCY))],
        before
    );
    assert!(ControlSockets::bind(&dir).is_err());
}
