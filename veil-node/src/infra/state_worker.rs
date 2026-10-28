//! Run synchronous consensus/WAL work without blocking the async I/O executor.
use crate::{domain::Role, usecase::ConsensusState};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
use tokio::sync::Mutex;

pub async fn run<R: Send + 'static>(
    state: Arc<Mutex<ConsensusState>>,
    work: impl FnOnce(&mut ConsensusState) -> R + Send + 'static,
) -> anyhow::Result<R> {
    let mut caller = super::latency::Caller(super::latency::begin());
    let requested = std::time::Instant::now();
    // Acquire asynchronously before spawning: at most one worker per state,
    // rather than filling the blocking pool with jobs waiting on the same lock.
    let mut state = state.lock_owned().await;
    let acquired = std::time::Instant::now();
    let op = caller.0;
    let (result, ended) = tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let started_unix_ns = op.map(|_| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        });
        let capture = op.map(|_| super::latency::Capture::start(started));
        // Once started, cancellation of the caller cannot interrupt a WAL write
        // or release this guard. The entire transition finishes under the lock.
        let mut panicked = false;
        let result = match catch_unwind(AssertUnwindSafe(|| work(&mut state))) {
            Ok(result) => Ok(result),
            Err(_) => {
                panicked = true;
                state.persistence_error.get_or_insert_with(|| {
                    "State worker panicked; restart and recover before voting".into()
                });
                state.role = Role::Follower;
                Err(anyhow::anyhow!("State worker panicked; validator stopped"))
            }
        };
        let completed = capture.map(|capture| super::latency::Completed {
            op: op.expect("capture has operation"),
            node: state.node_id.clone(),
            term: state.current_term,
            index: state.last_log_index(),
            applied: state.last_applied,
            started_unix_ns: started_unix_ns.unwrap_or(0),
            lock_wait: acquired.duration_since(requested),
            queue_wait: started.duration_since(acquired),
            work: std::time::Duration::ZERO,
            panicked,
            persistence_failed: state.persistence_error.is_some(),
            samples: capture.finish(),
        });
        drop(state);
        if let Some(mut c) = completed {
            c.work = started.elapsed();
            super::latency::submit(super::latency::Report::Completed(c));
        }
        (result, std::time::Instant::now())
    })
    .await?;
    caller.resumed(ended);
    result
}

#[cfg(test)]
#[path = "state_worker_tests.rs"]
mod tests;
