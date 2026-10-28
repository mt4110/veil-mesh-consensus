//! Bounded, optional diagnostics. Never perform log I/O under the state/WAL lock.
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{sync_channel, SyncSender},
        OnceLock,
    },
    time::{Duration, Instant},
};

const MAX_SAMPLES: usize = 128;
static NEXT: AtomicU64 = AtomicU64::new(1);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static SINK: OnceLock<Option<SyncSender<Report>>> = OnceLock::new();
thread_local! { static SAMPLES: RefCell<Option<(Instant, Vec<Sample>)>> = const { RefCell::new(None) }; }

#[derive(Debug)]
pub(crate) struct Sample {
    pub stage: &'static str,
    pub elapsed: Duration,
    pub end_offset: Duration,
    pub bytes: usize,
    pub ok: bool,
}

pub(crate) fn active() -> bool {
    SAMPLES.with(|s| s.borrow().is_some())
}

pub(crate) fn record(stage: &'static str, elapsed: Duration, bytes: usize, ok: bool) {
    SAMPLES.with(|s| {
        if let Some((origin, samples)) = s.borrow_mut().as_mut() {
            if samples.len() < MAX_SAMPLES {
                samples.push(Sample {
                    stage,
                    elapsed,
                    end_offset: origin.elapsed(),
                    bytes,
                    ok,
                });
            } else {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
}

pub(crate) struct Capture;
impl Capture {
    pub fn start(origin: Instant) -> Self {
        SAMPLES.with(|s| *s.borrow_mut() = Some((origin, Vec::with_capacity(MAX_SAMPLES))));
        Self
    }
    pub fn finish(self) -> Vec<Sample> {
        SAMPLES.with(|s| {
            s.borrow_mut()
                .take()
                .map_or_else(Vec::new, |(_, samples)| samples)
        })
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        SAMPLES.with(|s| {
            s.borrow_mut().take();
        });
    }
}

pub(crate) struct Completed {
    pub op: u64,
    pub node: String,
    pub term: u64,
    pub index: u64,
    pub applied: u64,
    pub started_unix_ns: u128,
    pub lock_wait: Duration,
    pub queue_wait: Duration,
    pub work: Duration,
    pub panicked: bool,
    pub persistence_failed: bool,
    pub samples: Vec<Sample>,
}
pub(crate) enum Report {
    Completed(Completed),
    Resumed { op: u64, elapsed: Duration },
    Cancelled { op: u64 },
}

fn emit(report: Report) {
    let dropped = DROPPED.load(Ordering::Relaxed);
    match report {
        Report::Completed(c) => {
            tracing::debug!(target: "veil_transition_timing", op=c.op, node=%c.node, term=c.term,
                index=c.index, applied=c.applied, started_unix_ns=c.started_unix_ns, lock_us=c.lock_wait.as_micros(),
                queue_us=c.queue_wait.as_micros(), work_us=c.work.as_micros(),
                panicked=c.panicked, persistence_failed=c.persistence_failed, dropped,
                "State transition completed");
            for (ordinal, s) in c.samples.iter().enumerate() {
                tracing::debug!(target: "veil_transition_timing", op=c.op, ordinal, stage=s.stage,
                    elapsed_us=s.elapsed.as_micros(), end_offset_us=s.end_offset.as_micros(), bytes=s.bytes, ok=s.ok, dropped,
                    "State transition stage");
            }
        }
        Report::Resumed { op, elapsed } => {
            tracing::debug!(target: "veil_transition_timing", op, resume_us=elapsed.as_micros(), dropped, "State caller resumed");
        }
        Report::Cancelled { op } => {
            tracing::debug!(target: "veil_transition_timing", op, dropped, "State caller cancelled; worker may still complete");
        }
    }
}

/// Initialize before acquiring the state lock. Failure only disables diagnostics.
pub(crate) fn begin() -> Option<u64> {
    if !tracing::enabled!(target: "veil_transition_timing", tracing::Level::DEBUG) {
        return None;
    }
    SINK.get_or_init(|| {
        let (tx, rx) = sync_channel(64);
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        std::thread::Builder::new()
            .name("veil-timing".into())
            .spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    while let Ok(report) = rx.recv() {
                        emit(report);
                    }
                });
            })
            .ok()
            .map(|_| tx)
    })
    .as_ref()?;
    Some(NEXT.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn submit(report: Report) {
    if let Some(Some(tx)) = SINK.get() {
        if tx.try_send(report).is_err() {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub(crate) struct Caller(pub Option<u64>);
impl Caller {
    pub fn resumed(&mut self, end: Instant) {
        if let Some(op) = self.0.take() {
            submit(Report::Resumed {
                op,
                elapsed: end.elapsed(),
            });
        }
    }
}
impl Drop for Caller {
    fn drop(&mut self) {
        if let Some(op) = self.0 {
            submit(Report::Cancelled { op });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usecase::ConsensusPersistence;
    #[test]
    fn capture_is_bounded_and_does_not_leak_into_the_next_transition() {
        let c = Capture::start(Instant::now());
        for _ in 0..MAX_SAMPLES + 1 {
            record("file-sync", Duration::ZERO, 12, false);
        }
        let records = c.finish();
        assert_eq!(records.len(), MAX_SAMPLES);
        assert!(!records[0].ok);
        assert!(!active());
        record("outside", Duration::ZERO, 0, true);
        let next = Capture::start(Instant::now());
        assert!(next.finish().is_empty());
    }

    #[test]
    fn measured_storage_preserves_bytes_and_reports_rejected_save_without_later_stages() {
        let root = std::env::var_os("VEIL_TEST_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = root.join(format!(
            "veil-measured-wal-{}-{stamp}/state.wal",
            std::process::id()
        ));
        let store = crate::infra::wal::FileWal::open(&path).unwrap();
        let capture = Capture::start(Instant::now());
        store.persist(b"original durable bytes").unwrap();
        let samples = capture.finish();
        let stages: Vec<_> = samples.iter().map(|s| s.stage).collect();
        let file_sync = stages.iter().position(|s| *s == "file-sync").unwrap();
        let rename = stages.iter().position(|s| *s == "rename").unwrap();
        let directory_sync = stages.iter().position(|s| *s == "directory-sync").unwrap();
        assert!(file_sync < rename && rename < directory_sync);
        assert!(samples.iter().all(|s| s.ok));
        assert!(samples
            .windows(2)
            .all(|s| s[0].end_offset <= s[1].end_offset));
        let capture = Capture::start(Instant::now());
        assert!(store.with_limit(1).persist(b"must not replace").is_err());
        let failed = capture.finish();
        assert_eq!(
            failed.iter().map(|s| s.stage).collect::<Vec<_>>(),
            ["start", "bounds"]
        );
        assert!(!failed[1].ok);
        assert_eq!(std::fs::read(&path).unwrap(), b"original durable bytes");
    }
}
