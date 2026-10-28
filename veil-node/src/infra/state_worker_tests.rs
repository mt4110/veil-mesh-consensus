use super::*;
use crate::usecase::ports::ConsensusPersistence;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Mutex as StdMutex,
};

struct GatedStore {
    armed: AtomicBool,
    entered: StdMutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: StdMutex<mpsc::Receiver<()>>,
    fail: bool,
}
impl ConsensusPersistence for GatedStore {
    fn persist(&self, _: &[u8]) -> std::io::Result<()> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.entered.lock().unwrap().take().unwrap().send(()).ok();
            // Also bounds a failed test: never leave a blocking worker hung.
            self.release
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(std::io::Error::other)?;
            if self.fail {
                return Err(std::io::Error::other("injected sync failure"));
            }
        }
        Ok(())
    }
}
struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}
fn fixture(
    fail: bool,
) -> (
    Arc<Mutex<ConsensusState>>,
    tokio::sync::oneshot::Receiver<()>,
    Release,
) {
    let (tx, rx) = mpsc::channel();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let store = Arc::new(GatedStore {
        armed: AtomicBool::new(false),
        entered: StdMutex::new(Some(entered)),
        release: StdMutex::new(rx),
        fail,
    });
    let mut state = ConsensusState::new("solo", vec![]);
    state.enable_persistence(store.clone()).unwrap();
    store.armed.store(true, Ordering::SeqCst);
    (Arc::new(Mutex::new(state)), observed, Release(Some(tx)))
}

#[tokio::test(flavor = "current_thread")]
async fn slow_wal_keeps_io_live_but_never_returns_vote_before_sync() {
    let (state, entered, release) = fixture(false);
    let vote = tokio::spawn(run(state.clone(), |s| s.start_signed_election()));
    tokio::time::timeout(std::time::Duration::from_secs(2), entered)
        .await
        .unwrap()
        .unwrap();
    // Another node and the current-thread executor progress while this WAL is blocked.
    let other = Arc::new(Mutex::new(ConsensusState::new("other", vec![])));
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run(other, |s| s.start_signed_election()),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert!(
        !vote.is_finished(),
        "no signed vote before durable completion"
    );
    assert!(
        state.try_lock().is_err(),
        "transition must remain exclusive"
    );
    drop(release);
    vote.await.unwrap().unwrap().unwrap();
    assert_eq!(state.lock().await.voted_for.as_deref(), Some("solo"));
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_waiter_cannot_release_a_running_wal_transition() {
    let (state, entered, release) = fixture(false);
    let caller = tokio::spawn(run(state.clone(), |s| s.start_signed_election()));
    tokio::time::timeout(std::time::Duration::from_secs(2), entered)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(state.try_lock().is_err());
    let next = tokio::spawn(run(state.clone(), |s| {
        (s.current_term, s.voted_for.clone())
    }));
    tokio::task::yield_now().await;
    assert!(
        !next.is_finished(),
        "no concurrent transition after caller cancellation"
    );
    drop(release);
    assert_eq!(next.await.unwrap().unwrap(), (1, Some("solo".into())));
}

#[tokio::test]
async fn failed_sync_and_worker_panic_stop_subsequent_votes() {
    let (state, entered, release) = fixture(true);
    let vote = tokio::spawn(run(state.clone(), |s| s.start_signed_election()));
    entered.await.unwrap();
    drop(release);
    assert!(vote.await.unwrap().unwrap().is_err());
    assert!(run(state.clone(), |s| s.start_signed_election())
        .await
        .unwrap()
        .is_err());
    let clean = Arc::new(Mutex::new(ConsensusState::new("clean", vec![])));
    assert!(run(clean.clone(), |_| panic!("injected transition panic"))
        .await
        .is_err());
    assert!(run(clean.clone(), |s| s.start_signed_election())
        .await
        .unwrap()
        .is_err());
    assert!(clean.lock().await.persistence_error.is_some());
}
