use super::*;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
fn path() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "veil-append-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
    p.join("state.wal")
}
#[test]
fn append_and_compact_preserve_latest_and_bound_storage() {
    let path = path();
    let limit = 3 * (OVERHEAD + 40) as u64;
    let store = AppendWal::create_new(&path, limit).unwrap();
    let mut syncs = 0;
    let mut dirs = 0;
    for i in 0..10u8 {
        store
            .persist_observed(&[i; 40], |stage| {
                if stage == "file-sync" {
                    syncs += 1;
                }
                if stage == "directory-sync" {
                    dirs += 1;
                }
            })
            .unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() <= limit);
    }
    assert_eq!((syncs, dirs), (10, 3));
    drop(store);
    let (store, bytes) = AppendWal::open_existing(&path, limit).unwrap();
    assert_eq!(bytes, vec![9; 40]);
    assert_eq!(store.position.lock().unwrap().seq, 10);
    store.persist(&[10; 40]).unwrap();
    drop(store);
    assert_eq!(
        AppendWal::open_existing(&path, limit).unwrap().1,
        vec![10; 40]
    );
}
#[test]
fn partial_corrupt_reordered_and_replayed_frames_are_refused() {
    let a = frame(1, 1, [0; 32], b"first");
    let b = frame(1, 2, hash(&a[..a.len() - 32]), b"second");
    let all = [a.clone(), b.clone()].concat();
    assert_eq!(decode(&all, 4096).unwrap().1, b"second");
    for cut in 0..all.len() {
        if cut != a.len() {
            assert!(decode(&all[..cut], 4096).is_err(), "cut={cut}");
        }
    }
    for at in 0..all.len() {
        let mut bad = all.clone();
        bad[at] ^= 1;
        assert!(decode(&bad, 4096).is_err(), "offset={at}");
    }
    for bad in [
        [a.clone(), a.clone()].concat(),
        [b, a].concat(),
        [all.clone(), vec![0]].concat(),
    ] {
        assert!(decode(&bad, 4096).is_err());
    }
    let mut seq = frame(1, 1, [0; 32], b"x");
    let first_hash = hash(&seq[..seq.len() - 32]);
    seq.extend(frame(2, 2, first_hash, b"y"));
    assert!(decode(&seq, 4096).is_err());
}
#[test]
fn missing_empty_legacy_wrong_permissions_and_oversize_refused() {
    let p = path();
    assert!(AppendWal::open_existing(&p, 4096).is_err());
    let store = AppendWal::create_new(&p, 4096).unwrap();
    assert!(AppendWal::create_new(&p, 4096).is_err());
    drop(store);
    assert!(AppendWal::open_existing(&p, 4096).is_err());
    std::fs::write(&p, b"legacy snapshot").unwrap();
    assert!(AppendWal::open_existing(&p, 4096).is_err());
    std::fs::write(&p, frame(1, 1, [0; 32], b"ok")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(AppendWal::open_existing(&p, 4096).is_err());
    let p = path();
    let s = AppendWal::create_new(&p, (OVERHEAD + 2) as u64).unwrap();
    assert!(s.persist(b"too large").is_err());
    assert!(s.persist(b"ok").is_err());
}
#[test]
fn append_failures_never_ack_or_continue_and_recovery_keeps_full_unacked_frame() {
    for stage in ["write", "after-write", "file-sync", "after-file-sync"] {
        let p = path();
        let s = AppendWal::create_new(&p, 4096).unwrap();
        s.persist(b"old").unwrap();
        assert!(s
            .persist_hook(b"new", |at| if at == stage { Err(error()) } else { Ok(()) })
            .is_err());
        assert!(s.persist(b"forbidden").is_err());
        drop(s);
        let (_, bytes) = AppendWal::open_existing(&p, 4096).unwrap();
        assert_eq!(bytes, if stage == "write" { b"old" } else { b"new" });
    }
    // An actual partial tail is never ignored, even if the preceding frame is valid.
    let p = path();
    let s = AppendWal::create_new(&p, 4096).unwrap();
    s.persist(b"old").unwrap();
    drop(s);
    private()
        .append(true)
        .open(&p)
        .unwrap()
        .write_all(b"VLS")
        .unwrap();
    let before = std::fs::read(&p).unwrap();
    assert!(AppendWal::open_existing(&p, 4096).is_err());
    assert_eq!(std::fs::read(&p).unwrap(), before);
}
#[test]
fn compaction_failures_preserve_evidence_and_never_fallback() {
    for stage in [
        "create",
        "write",
        "after-write",
        "file-sync",
        "after-file-sync",
        "rename",
        "after-rename",
        "directory-sync",
        "after-directory-sync",
    ] {
        let p = path();
        let limit = (OVERHEAD + 3) as u64;
        let s = AppendWal::create_new(&p, limit).unwrap();
        s.persist(b"old").unwrap();
        assert!(s
            .persist_hook(b"new", |at| if at == stage { Err(error()) } else { Ok(()) })
            .is_err());
        assert!(s.persist(b"bad").is_err());
        drop(s);
        let restored = AppendWal::open_existing(&p, limit);
        if stage == "create" {
            assert_eq!(restored.unwrap().1, b"old");
        } else if ["after-rename", "directory-sync", "after-directory-sync"].contains(&stage) {
            assert_eq!(restored.unwrap().1, b"new");
        } else {
            assert!(restored.is_err(), "pending compaction: {stage}");
        }
    }
}

#[test]
fn crash_child() {
    let Ok(path) = std::env::var("VEIL_APPEND_CRASH_PATH") else {
        return;
    };
    let stage = std::env::var("VEIL_APPEND_CRASH_STAGE").unwrap();
    let limit = std::env::var("VEIL_APPEND_CRASH_LIMIT")
        .unwrap()
        .parse()
        .unwrap();
    let (store, _) = AppendWal::open_existing(Path::new(&path), limit).unwrap();
    if stage == "partial" {
        private()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"VLS")
            .unwrap();
        std::process::exit(73);
    }
    store
        .persist_hook(b"new", |at| {
            if at == stage {
                std::process::exit(73);
            }
            Ok(())
        })
        .unwrap();
    panic!("crash stage was not reached");
}
#[test]
fn process_exit_at_append_and_compaction_boundaries() {
    for compact in [false, true] {
        for stage in [
            "partial",
            "after-write",
            "after-file-sync",
            "after-rename",
            "after-directory-sync",
        ] {
            if !compact && ["after-rename", "after-directory-sync"].contains(&stage) {
                continue;
            }
            let p = path();
            let limit = if compact { (OVERHEAD + 3) as u64 } else { 4096 };
            let s = AppendWal::create_new(&p, limit).unwrap();
            s.persist(b"old").unwrap();
            drop(s);
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "infra::snapshot_append_prototype::tests::crash_child",
                    "--exact",
                ])
                .env("VEIL_APPEND_CRASH_PATH", &p)
                .env("VEIL_APPEND_CRASH_STAGE", stage)
                .env("VEIL_APPEND_CRASH_LIMIT", limit.to_string())
                .output()
                .unwrap();
            assert_eq!(
                result.status.code(),
                Some(73),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let recovered = AppendWal::open_existing(&p, limit);
            if stage == "partial" || compact && ["after-write", "after-file-sync"].contains(&stage)
            {
                assert!(recovered.is_err());
            } else {
                assert_eq!(recovered.unwrap().1, b"new");
            }
        }
    }
}

#[test]
#[ignore = "explicit bounded storage comparison; synthetic payload, not consensus acceptance"]
fn controlled_storage_comparison() {
    let backend = std::env::var("VEIL_TEST_WAL").unwrap();
    assert!(["replace", "append"].contains(&backend.as_str()));
    let workers: usize = std::env::var("VEIL_TEST_WAL_THREADS")
        .unwrap()
        .parse()
        .unwrap();
    assert!([1, 4].contains(&workers));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(workers));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let barrier = barrier.clone();
            let backend = backend.clone();
            std::thread::spawn(move || {
                let p = path();
                let limit = 1024 * 1024;
                let wal: Box<dyn ConsensusPersistence> = if backend == "append" {
                    Box::new(AppendWal::create_new(&p, limit).unwrap())
                } else {
                    Box::new(FileWal::create_new(&p).unwrap().with_limit(limit))
                };
                barrier.wait();
                let mut durations = Vec::new();
                let mut payload = vec![42u8; 16 * 1024];
                for seq in 0..64u64 {
                    payload[..8].copy_from_slice(&seq.to_le_bytes());
                    let start = std::time::Instant::now();
                    wal.persist(&payload).unwrap();
                    durations.push(start.elapsed().as_micros());
                    assert!(std::fs::metadata(&p).unwrap().len() <= limit);
                }
                drop(wal);
                let recovered = if backend == "append" {
                    AppendWal::open_existing(&p, limit).unwrap().1
                } else {
                    std::fs::read(&p).unwrap()
                };
                assert_eq!(recovered, payload);
                durations
            })
        })
        .collect();
    let mut durations: Vec<_> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    durations.sort_unstable();
    println!(
        "STORAGE_COMPARISON backend={} workers={} samples={} median_us={} p95_us={} max_us={}",
        backend,
        workers,
        durations.len(),
        durations[durations.len() / 2],
        durations[(durations.len() * 95 / 100).min(durations.len() - 1)],
        durations.last().unwrap()
    );
}
