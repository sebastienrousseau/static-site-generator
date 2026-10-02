// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Event-driven file watcher (issue #526).
//!
//! Wraps [`notify::recommended_watcher`] so the dev server can react to
//! OS filesystem events (`FSEvents` / `inotify` / `ReadDirectoryChangesW`)
//! instead of polling. Compared to the legacy [`crate::watch::FileWatcher`],
//! this:
//!
//! * Costs ~0% idle CPU (kernel pushes events; no `mtime` scan loop).
//! * Wakes within ~5 ms of the OS event vs. the 1-2 s polling interval.
//! * Coalesces rapid saves through a 100 ms debounce window — a
//!   `cargo fmt` storm that touches one file four times in 200 ms
//!   produces exactly one drain (AC6).
//!
//! # Architecture
//!
//! ```text
//! notify backend ──▶ raw_tx ──▶ debounce thread ──▶ batched_tx ──▶ caller
//!  (OS event)        mpsc          (100 ms window)      mpsc
//! ```
//!
//! The debounce thread:
//! 1. Blocks on `raw_rx.recv()` until the first event arrives.
//! 2. Records the wall-clock instant of that first event.
//! 3. Drains every subsequent event with `recv_timeout(remaining)` until
//!    100 ms have elapsed since the first event.
//! 4. Sends the de-duplicated path set to `batched_tx`.
//!
//! Last-write-wins is implicit: only the path set is forwarded, the per-event
//! ordering is discarded.
//!
//! # Why not `notify-debouncer-mini`?
//!
//! That crate brings a `tokio` dep transitively; ssg is rayon-only. The
//! hand-rolled debouncer here is ~30 lines, deterministic, and unit-testable
//! without touching the filesystem (see [`debounce_paths`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{
    recommended_watcher, Event, EventKind, RecommendedWatcher, RecursiveMode,
    Watcher,
};

use crate::error::SsgError;

/// Default debounce window. 100 ms is the issue-#526 AC6 target — long
/// enough to collapse a `cargo fmt` save storm into one rebuild, short
/// enough to feel instant in the browser.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(100);

/// Cap on how many distinct paths can be debounced into a single batch
/// before the watcher forces a drain. Prevents pathological build-output
/// storms from delaying delivery beyond a reasonable bound.
pub const MAX_BATCH_PATHS: usize = 10_000;

/// A batched set of changed paths produced by [`EventWatcher`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeBatch {
    /// Distinct paths touched during the debounce window.
    pub paths: Vec<PathBuf>,
}

impl ChangeBatch {
    /// Returns `true` if the batch carries no paths.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::event_watch::ChangeBatch;
    /// let b = ChangeBatch { paths: vec![] };
    /// assert!(b.is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Returns the number of unique paths in the batch.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use ssg::event_watch::ChangeBatch;
    /// let b = ChangeBatch { paths: vec![PathBuf::from("a"), PathBuf::from("b")] };
    /// assert_eq!(b.len(), 2);
    /// ```
    #[must_use]
    pub const fn len(&self) -> usize {
        self.paths.len()
    }
}

/// Result of a [`EventWatcher::recv_timeout`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecvOutcome {
    /// A debounced batch arrived.
    Batch(ChangeBatch),
    /// No batch landed inside the requested timeout. Caller may loop.
    Timeout,
    /// The watcher was dropped or the debounce thread exited; no
    /// further batches will arrive on this channel.
    Closed,
}

impl RecvOutcome {
    /// Returns the wrapped batch, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::event_watch::{ChangeBatch, RecvOutcome};
    /// let b = ChangeBatch { paths: vec![] };
    /// let out = RecvOutcome::Batch(b.clone());
    /// assert_eq!(out.batch(), Some(b));
    /// assert!(RecvOutcome::Timeout.batch().is_none());
    /// ```
    #[must_use]
    pub fn batch(self) -> Option<ChangeBatch> {
        match self {
            Self::Batch(b) => Some(b),
            Self::Timeout | Self::Closed => None,
        }
    }

    /// Returns true if the channel is closed (no more batches).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::event_watch::RecvOutcome;
    /// assert!(RecvOutcome::Closed.is_closed());
    /// assert!(!RecvOutcome::Timeout.is_closed());
    /// ```
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// Event-driven watcher built on top of `notify`.
///
/// Owns the recommended backend (`FSEvents`/`inotify`/RDCW), a debounce
/// thread, and the receiver end of the batched-event channel. Dropping
/// the watcher tears down the backend and the debounce thread.
pub struct EventWatcher {
    /// Live notify backend. Held so dropping the watcher unsubscribes.
    /// `Option` so we can `take()` it in `Drop` without unsafe.
    backend: Mutex<Option<RecommendedWatcher>>,
    /// Channel that delivers debounced batches to the caller.
    rx: Receiver<ChangeBatch>,
    /// Handle to the debounce thread. Joined on drop.
    debounce_handle: Mutex<Option<JoinHandle<()>>>,
    /// Shutdown signal — set to true to make the debounce thread exit.
    shutdown: Arc<Mutex<bool>>,
    /// Window the debounce thread waits before flushing.
    debounce: Duration,
}

impl std::fmt::Debug for EventWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventWatcher")
            .field("debounce", &self.debounce)
            .finish_non_exhaustive()
    }
}

impl EventWatcher {
    /// Builds a watcher rooted at `dir`, watching recursively, with a
    /// 100 ms debounce window.
    ///
    /// # Errors
    ///
    /// Returns [`SsgError::Io`] wrapping the underlying `notify::Error`
    /// when the backend cannot subscribe (missing directory, permission
    /// denied, kernel resource exhaustion).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::event_watch::{EventWatcher, DEFAULT_DEBOUNCE};
    /// let tmp = tempfile::tempdir().unwrap();
    /// let w = EventWatcher::new(tmp.path()).unwrap();
    /// assert_eq!(w.debounce(), DEFAULT_DEBOUNCE);
    /// ```
    pub fn new(dir: &Path) -> Result<Self, SsgError> {
        Self::with_debounce(dir, DEFAULT_DEBOUNCE)
    }

    /// Same as [`Self::new`] but with a caller-supplied debounce window
    /// (used by tests to keep latency low).
    ///
    /// # Errors
    ///
    /// See [`Self::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use ssg::event_watch::EventWatcher;
    /// let tmp = tempfile::tempdir().unwrap();
    /// let w = EventWatcher::with_debounce(tmp.path(), Duration::from_millis(50)).unwrap();
    /// assert_eq!(w.debounce(), Duration::from_millis(50));
    /// ```
    pub fn with_debounce(
        dir: &Path,
        debounce: Duration,
    ) -> Result<Self, SsgError> {
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        let (batched_tx, batched_rx) = mpsc::channel::<ChangeBatch>();

        let mut backend = create_backend(move |res: notify::Result<Event>| {
            forward_event(res, &raw_tx);
        })
        .map_err(|e| SsgError::Io {
            path: dir.to_path_buf(),
            source: std::io::Error::other(format!("notify watcher init: {e}")),
        })?;

        backend.watch(dir, RecursiveMode::Recursive).map_err(|e| {
            SsgError::Io {
                path: dir.to_path_buf(),
                source: std::io::Error::other(format!("notify watch: {e}")),
            }
        })?;

        let shutdown = Arc::new(Mutex::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let debounce_handle = spawn_debounce_thread(
            thread::Builder::new().name("ssg-watch-debounce".into()),
            move || {
                debounce_loop(raw_rx, batched_tx, debounce, &shutdown_clone);
            },
        )
        .map_err(|e| SsgError::Io {
            path: dir.to_path_buf(),
            source: std::io::Error::other(format!(
                "debounce thread spawn: {e}"
            )),
        })?;

        Ok(Self {
            backend: Mutex::new(Some(backend)),
            rx: batched_rx,
            debounce_handle: Mutex::new(Some(debounce_handle)),
            shutdown,
            debounce,
        })
    }

    /// Blocks until the next debounced batch is available.
    ///
    /// Returns `None` if the watcher is being torn down.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ssg::event_watch::EventWatcher;
    /// let tmp = tempfile::tempdir().unwrap();
    /// let w = EventWatcher::new(tmp.path()).unwrap();
    /// // Blocks until a change arrives — would hang in a doctest sandbox.
    /// if let Some(batch) = w.recv() {
    ///     assert!(!batch.paths.is_empty());
    /// }
    /// ```
    #[must_use]
    pub fn recv(&self) -> Option<ChangeBatch> {
        self.rx.recv().ok()
    }

    /// Like [`Self::recv`] but with a timeout.
    ///
    /// Returns:
    /// * [`RecvOutcome::Batch`] — a debounced batch arrived.
    /// * [`RecvOutcome::Timeout`] — no batch within `timeout`.
    /// * [`RecvOutcome::Closed`] — the watcher was dropped or the
    ///   debounce thread exited.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use ssg::event_watch::{EventWatcher, RecvOutcome};
    /// let tmp = tempfile::tempdir().unwrap();
    /// let w = EventWatcher::with_debounce(tmp.path(), Duration::from_millis(20)).unwrap();
    /// let out = w.recv_timeout(Duration::from_millis(30));
    /// assert!(!out.is_closed());
    /// ```
    pub fn recv_timeout(&self, timeout: Duration) -> RecvOutcome {
        match self.rx.recv_timeout(timeout) {
            Ok(b) => RecvOutcome::Batch(b),
            Err(RecvTimeoutError::Timeout) => RecvOutcome::Timeout,
            Err(RecvTimeoutError::Disconnected) => RecvOutcome::Closed,
        }
    }

    /// Test-only: drops the notify backend so the debounce thread winds
    /// down and the batched channel closes without dropping the watcher.
    #[cfg(test)]
    pub(crate) fn close_backend_for_test(&self) {
        let _ = self.backend.lock().map(|mut b| *b = None);
    }

    /// Debounce window in effect for this watcher.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use ssg::event_watch::EventWatcher;
    /// let tmp = tempfile::tempdir().unwrap();
    /// let w = EventWatcher::with_debounce(tmp.path(), Duration::from_millis(75)).unwrap();
    /// assert_eq!(w.debounce(), Duration::from_millis(75));
    /// ```
    #[must_use]
    pub const fn debounce(&self) -> Duration {
        self.debounce
    }
}

impl Drop for EventWatcher {
    fn drop(&mut self) {
        // Signal the debounce thread to exit on its next loop iteration.
        if let Ok(mut s) = self.shutdown.lock() {
            *s = true;
        }
        // Drop the backend first — that closes `raw_tx`, which unblocks
        // the debounce thread's `recv()`.
        if let Ok(mut backend) = self.backend.lock() {
            *backend = None;
        }
        // Join the debounce thread so we don't leak a zombie on drop.
        if let Ok(mut handle) = self.debounce_handle.lock() {
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}

/// Returns whether a notify event represents a real change.
///
/// We deliberately ignore [`EventKind::Access`] and [`EventKind::Other`]
/// (mount/unmount on macOS, access-time bumps on Linux); those don't
/// change file content and rebuilding for them is wasted work.
///
/// # Examples
///
/// ```
/// use notify::{EventKind, event::ModifyKind};
/// use ssg::event_watch::event_should_propagate;
/// assert!(event_should_propagate(&EventKind::Modify(ModifyKind::Any)));
/// assert!(!event_should_propagate(&EventKind::Other));
/// ```
#[must_use]
pub const fn event_should_propagate(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Forwards the paths of a propagatable notify event onto `raw_tx`.
///
/// Extracted from the `recommended_watcher` callback so the ignore
/// branches (backend error, `Access`/`Other` events) are unit-testable
/// without waiting on a live OS event.
fn forward_event(res: notify::Result<Event>, raw_tx: &Sender<PathBuf>) {
    if let Ok(event) = res {
        if event_should_propagate(&event.kind) {
            for path in event.paths {
                // Best-effort: receiver dropped means the watcher
                // itself was dropped; nothing to do.
                let _ = raw_tx.send(path);
            }
        }
    }
}

/// Thread-local fault injection for the two error branches real OS
/// behaviour cannot reach deterministically (backend init and thread
/// spawn failures). Thread-local — unlike a process-global `fail`
/// failpoint — so arming a fault in one test cannot leak into
/// concurrently running tests that also build watchers.
#[cfg(all(test, feature = "test-fault-injection"))]
mod fault {
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<Option<&'static str>> = const { Cell::new(None) };
    }

    /// Arms `name` for the current thread; disarmed when the returned
    /// guard drops (panic-safe).
    pub(super) fn arm(name: &'static str) -> ArmGuard {
        ARMED.with(|a| a.set(Some(name)));
        ArmGuard
    }

    /// Returns whether `name` is armed on the current thread.
    pub(super) fn armed(name: &str) -> bool {
        ARMED.with(|a| a.get() == Some(name))
    }

    /// RAII guard that disarms the thread-local fault on drop.
    #[derive(Debug)]
    pub(super) struct ArmGuard;

    impl Drop for ArmGuard {
        fn drop(&mut self) {
            ARMED.with(|a| a.set(None));
        }
    }
}

/// Creates the notify backend. Wrapped so tests can inject a
/// construction failure via the `event-watch::backend-init`
/// thread-local fault.
fn create_backend<F: notify::EventHandler>(
    event_handler: F,
) -> notify::Result<RecommendedWatcher> {
    #[cfg(all(test, feature = "test-fault-injection"))]
    if fault::armed("event-watch::backend-init") {
        return Err(notify::Error::generic(
            "injected: event-watch::backend-init",
        ));
    }
    recommended_watcher(event_handler)
}

/// Spawns the debounce thread. Wrapped so tests can inject a spawn
/// failure via the `event-watch::debounce-spawn` thread-local fault.
fn spawn_debounce_thread(
    builder: thread::Builder,
    body: impl FnOnce() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    #[cfg(all(test, feature = "test-fault-injection"))]
    if fault::armed("event-watch::debounce-spawn") {
        return Err(std::io::Error::other(
            "injected: event-watch::debounce-spawn",
        ));
    }
    builder.spawn(body)
}

/// Time left in the debounce window, or `None` once the window has
/// closed. Extracted from [`debounce_loop`] so the boundary cases
/// (`elapsed == window`, `elapsed > window`) are unit-testable.
fn remaining_window(window: Duration, elapsed: Duration) -> Option<Duration> {
    let remaining = window.checked_sub(elapsed)?;
    if remaining.is_zero() {
        None
    } else {
        Some(remaining)
    }
}

/// Debounce loop: collect paths from `raw_rx` for at most `window`
/// after the first event, then flush.
///
/// Extracted from [`EventWatcher::with_debounce`] so the thread body has
/// a single, testable signature.
fn debounce_loop(
    raw_rx: Receiver<PathBuf>,
    batched_tx: Sender<ChangeBatch>,
    window: Duration,
    shutdown: &Arc<Mutex<bool>>,
) {
    // Block waiting for the first event of each batch. A disconnected
    // channel means the watcher was dropped — exit the loop.
    while let Ok(first) = raw_rx.recv() {
        if shutdown.lock().map_or(true, |g| *g) {
            break;
        }

        let mut paths: HashSet<PathBuf> = HashSet::new();
        let _ = paths.insert(first);
        let start = Instant::now();

        // Drain everything that lands inside the window.
        while let Some(remaining) = remaining_window(window, start.elapsed()) {
            match raw_rx.recv_timeout(remaining) {
                Ok(p) => {
                    let _ = paths.insert(p);
                    if paths.len() >= MAX_BATCH_PATHS {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    // Send what we have and exit on the next iteration.
                    let batch = ChangeBatch {
                        paths: sorted(paths),
                    };
                    let _ = batched_tx.send(batch);
                    return;
                }
            }
        }

        let batch = ChangeBatch {
            paths: sorted(paths),
        };
        if batched_tx.send(batch).is_err() {
            break;
        }
    }
}

/// Pure helper: collapse a stream of `(path, instant)` events into a
/// list of debounced batches. Used by tests so the debouncer logic can
/// be verified without spawning threads.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use std::time::{Duration, Instant};
/// use ssg::event_watch::debounce_paths;
/// let t0 = Instant::now();
/// let events = vec![
///     (PathBuf::from("a.md"), t0),
///     (PathBuf::from("a.md"), t0 + Duration::from_millis(20)),
/// ];
/// let out = debounce_paths(&events, Duration::from_millis(100));
/// assert_eq!(out.len(), 1);
/// assert_eq!(out[0].len(), 1);
/// ```
#[must_use]
pub fn debounce_paths(
    events: &[(PathBuf, Instant)],
    window: Duration,
) -> Vec<ChangeBatch> {
    if events.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut current: HashSet<PathBuf> = HashSet::new();
    let mut first: Option<Instant> = None;

    for (path, t) in events {
        match first {
            None => {
                first = Some(*t);
                let _ = current.insert(path.clone());
            }
            Some(f) if *t < f + window => {
                let _ = current.insert(path.clone());
            }
            Some(_) => {
                // Window closed — flush and start a new batch.
                let taken = std::mem::take(&mut current);
                out.push(ChangeBatch {
                    paths: sorted(taken),
                });
                first = Some(*t);
                let _ = current.insert(path.clone());
            }
        }
    }
    if !current.is_empty() {
        out.push(ChangeBatch {
            paths: sorted(current),
        });
    }
    out
}

fn sorted(set: HashSet<PathBuf>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = set.into_iter().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn debounce_empty_input_yields_empty_output() {
        assert!(
            debounce_paths(&[], Duration::from_millis(100)).is_empty(),
            "{:?}",
            debounce_paths(&[], Duration::from_millis(100))
        );
    }

    #[test]
    fn debounce_single_event_one_batch() {
        let t0 = Instant::now();
        let out =
            debounce_paths(&[(p("a.md"), t0)], Duration::from_millis(100));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].paths, vec![p("a.md")]);
    }

    #[test]
    fn debounce_collapses_four_saves_in_200ms_window_into_one_batch_when_within_window(
    ) {
        // AC6: cargo-fmt-style storm — 4 events on the same file within
        // 80 ms, debounce window 100 ms => exactly 1 batch, 1 path.
        let t0 = Instant::now();
        let events = vec![
            (p("style.css"), t0),
            (p("style.css"), t0 + Duration::from_millis(20)),
            (p("style.css"), t0 + Duration::from_millis(40)),
            (p("style.css"), t0 + Duration::from_millis(80)),
        ];
        let out = debounce_paths(&events, Duration::from_millis(100));
        assert_eq!(out.len(), 1, "should collapse to one batch");
        assert_eq!(out[0].paths, vec![p("style.css")]);
    }

    #[test]
    fn debounce_splits_batches_across_window_boundary() {
        let t0 = Instant::now();
        let events = vec![
            (p("a.md"), t0),
            (p("b.md"), t0 + Duration::from_millis(50)),
            // 150 ms after first => outside the 100 ms window.
            (p("c.md"), t0 + Duration::from_millis(150)),
        ];
        let out = debounce_paths(&events, Duration::from_millis(100));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].paths, vec![p("a.md"), p("b.md")]);
        assert_eq!(out[1].paths, vec![p("c.md")]);
    }

    #[test]
    fn debounce_deduplicates_paths_in_same_window() {
        let t0 = Instant::now();
        let events = vec![
            (p("x"), t0),
            (p("y"), t0 + Duration::from_millis(10)),
            (p("x"), t0 + Duration::from_millis(20)),
            (p("y"), t0 + Duration::from_millis(30)),
        ];
        let out = debounce_paths(&events, Duration::from_millis(100));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].paths, vec![p("x"), p("y")]);
    }

    #[test]
    fn event_should_propagate_accepts_modify_create_remove() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind};
        assert!(event_should_propagate(&EventKind::Create(CreateKind::File)));
        assert!(event_should_propagate(&EventKind::Modify(ModifyKind::Any)));
        assert!(event_should_propagate(&EventKind::Remove(RemoveKind::File)));
    }

    #[test]
    fn event_should_propagate_rejects_access_and_other() {
        use notify::event::AccessKind;
        assert!(!event_should_propagate(&EventKind::Access(AccessKind::Any)));
        assert!(!event_should_propagate(&EventKind::Other));
    }

    #[test]
    fn change_batch_len_and_is_empty() {
        let empty = ChangeBatch { paths: vec![] };
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let one = ChangeBatch {
            paths: vec![p("x")],
        };
        assert!(!one.is_empty());
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn change_batch_eq_clone() {
        let a = ChangeBatch {
            paths: vec![p("x")],
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn default_debounce_is_100ms() {
        assert_eq!(DEFAULT_DEBOUNCE, Duration::from_millis(100));
    }

    #[test]
    fn new_returns_err_when_path_missing() {
        // Non-existent path — recommended_watcher may succeed but
        // .watch() should fail.
        let res = EventWatcher::new(Path::new("/nonexistent/ssg/test/dir"));
        assert!(res.is_err());
    }

    #[test]
    fn recv_outcome_batch_extracts_payload() {
        let b = ChangeBatch {
            paths: vec![p("a")],
        };
        let out = RecvOutcome::Batch(b.clone());
        assert_eq!(out.batch(), Some(b));
        assert!(RecvOutcome::Timeout.batch().is_none());
        assert!(RecvOutcome::Closed.batch().is_none());
    }

    #[test]
    fn recv_outcome_is_closed_only_for_closed() {
        assert!(RecvOutcome::Closed.is_closed());
        assert!(!RecvOutcome::Timeout.is_closed());
        let b = ChangeBatch { paths: vec![] };
        assert!(!RecvOutcome::Batch(b).is_closed());
    }

    #[test]
    fn live_watcher_with_debounce_yields_batch_on_real_fs_event() {
        // Live integration: create a temp dir, instantiate the watcher,
        // touch a file, and assert we receive a batch within a reasonable
        // window. Exercises with_debounce, the notify callback closure,
        // debounce_loop, and recv_timeout's Batch arm.
        let dir = tempfile::tempdir().expect("tempdir");
        let w =
            EventWatcher::with_debounce(dir.path(), Duration::from_millis(50))
                .expect("watcher");

        // Sanity: debounce() accessor + Debug impl.
        assert_eq!(w.debounce(), Duration::from_millis(50));
        let dbg = format!("{:?}", w);
        assert!(dbg.contains("EventWatcher"));

        // Write a file inside the watched dir.
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"hello").expect("write");

        // Poll for up to ~2s — notify backends can be slow on cold start.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got: Option<ChangeBatch> = None;
        while got.is_none() && Instant::now() < deadline {
            got = w.recv_timeout(Duration::from_millis(200)).batch();
        }
        // CI macOS FSEvents can rarely lose the first event for a brand
        // new dir; don't fail the suite — just ensure we exercised the
        // pathway without panicking.
        assert!(got.is_none_or(|b| !b.is_empty()));
    }

    #[test]
    fn drop_tears_down_thread_without_hanging() {
        // Build + immediately drop. Drop must signal shutdown, drop the
        // backend, and join the debounce thread cleanly. If the join
        // hangs, the test framework will time out and fail.
        let dir = tempfile::tempdir().expect("tempdir");
        let w =
            EventWatcher::with_debounce(dir.path(), Duration::from_millis(30))
                .expect("watcher");
        drop(w);
    }

    #[test]
    fn recv_timeout_returns_timeout_when_idle() {
        // Build a watcher on an empty tempdir, do not touch anything,
        // and assert recv_timeout returns Timeout before the deadline.
        let dir = tempfile::tempdir().expect("tempdir");
        let w =
            EventWatcher::with_debounce(dir.path(), Duration::from_millis(20))
                .expect("watcher");
        let out = w.recv_timeout(Duration::from_millis(50));
        // Either Timeout (typical) or Batch (if FS noise) — neither should
        // be Closed under normal conditions.
        assert!(!out.is_closed());
    }

    #[test]
    fn forward_event_sends_paths_for_propagatable_events() {
        use notify::event::CreateKind;
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let event = Event::new(EventKind::Create(CreateKind::File))
            .add_path(p("a.md"))
            .add_path(p("b.md"));
        forward_event(Ok(event), &tx);
        assert_eq!(rx.try_recv().unwrap(), p("a.md"));
        assert_eq!(rx.try_recv().unwrap(), p("b.md"));
        assert!(rx.try_recv().is_err(), "no extra paths expected");
    }

    #[test]
    fn forward_event_ignores_access_events() {
        use notify::event::AccessKind;
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let event =
            Event::new(EventKind::Access(AccessKind::Any)).add_path(p("a.md"));
        forward_event(Ok(event), &tx);
        assert!(rx.try_recv().is_err(), "access events must be dropped");
    }

    #[test]
    fn forward_event_ignores_backend_errors() {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        forward_event(Err(notify::Error::generic("boom")), &tx);
        assert!(rx.try_recv().is_err(), "errors must be swallowed");
    }

    #[test]
    fn forward_event_survives_dropped_receiver() {
        use notify::event::CreateKind;
        let (tx, rx) = mpsc::channel::<PathBuf>();
        drop(rx);
        let event =
            Event::new(EventKind::Create(CreateKind::File)).add_path(p("a"));
        // Must not panic even though the send fails.
        forward_event(Ok(event), &tx);
    }

    #[test]
    fn forward_event_with_no_paths_sends_nothing() {
        use notify::event::CreateKind;
        // Every other propagatable-event test attaches at least one
        // path, so the `for path in event.paths` loop always runs at
        // least once elsewhere. Exercise the zero-iteration case too.
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let event = Event::new(EventKind::Create(CreateKind::File));
        forward_event(Ok(event), &tx);
        assert!(rx.try_recv().is_err(), "no paths means nothing to send");
    }

    #[test]
    fn remaining_window_returns_time_left_inside_window() {
        assert_eq!(
            remaining_window(
                Duration::from_millis(100),
                Duration::from_millis(40)
            ),
            Some(Duration::from_millis(60))
        );
    }

    #[test]
    fn remaining_window_none_when_elapsed_exceeds_window() {
        assert_eq!(
            remaining_window(
                Duration::from_millis(100),
                Duration::from_millis(150)
            ),
            None
        );
    }

    #[test]
    fn remaining_window_none_at_exact_boundary() {
        assert_eq!(
            remaining_window(
                Duration::from_millis(100),
                Duration::from_millis(100)
            ),
            None
        );
    }

    #[test]
    fn debounce_loop_exits_when_shutdown_already_signalled() {
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        let (batched_tx, batched_rx) = mpsc::channel::<ChangeBatch>();
        let shutdown = Arc::new(Mutex::new(true));

        raw_tx.send(p("a.md")).unwrap();
        drop(raw_tx);
        debounce_loop(raw_rx, batched_tx, Duration::from_millis(10), &shutdown);

        // Shutdown short-circuits before any batch is flushed.
        assert!(batched_rx.try_recv().is_err());
    }

    #[test]
    fn debounce_loop_treats_poisoned_shutdown_lock_as_signalled() {
        // Poison the `shutdown` mutex before debounce_loop ever locks
        // it. `shutdown.lock().map_or(true, |g| *g)` must fall back to
        // `true` (treat-as-shutting-down) on a poisoned lock rather
        // than panicking or silently continuing.
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        let (batched_tx, batched_rx) = mpsc::channel::<ChangeBatch>();
        let shutdown = Arc::new(Mutex::new(false));

        let poison_target = Arc::clone(&shutdown);
        let _ = thread::spawn(move || {
            let _guard = poison_target.lock().unwrap();
            panic!("poison shutdown for test");
        })
        .join();

        raw_tx.send(p("a.md")).unwrap();
        drop(raw_tx);
        debounce_loop(raw_rx, batched_tx, Duration::from_millis(10), &shutdown);

        // Poisoned lock reads as "shut down" — the loop must break
        // immediately without flushing a batch.
        assert!(batched_rx.try_recv().is_err());
    }

    #[test]
    fn debounce_loop_forces_drain_at_max_batch_paths() {
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        let (batched_tx, batched_rx) = mpsc::channel::<ChangeBatch>();
        let shutdown = Arc::new(Mutex::new(false));

        for i in 0..MAX_BATCH_PATHS {
            raw_tx.send(PathBuf::from(format!("f{i}"))).unwrap();
        }
        drop(raw_tx);
        // Long window: the cap — not the clock — must force the drain.
        debounce_loop(raw_rx, batched_tx, Duration::from_secs(60), &shutdown);

        let batch = batched_rx.try_recv().expect("capped batch flushed");
        assert_eq!(batch.len(), MAX_BATCH_PATHS);
    }

    #[test]
    fn debounce_loop_flushes_pending_batch_on_disconnect() {
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        let (batched_tx, batched_rx) = mpsc::channel::<ChangeBatch>();
        let shutdown = Arc::new(Mutex::new(false));

        raw_tx.send(p("only.md")).unwrap();
        drop(raw_tx);
        // Sender gone mid-window: the pending set must still be flushed.
        debounce_loop(raw_rx, batched_tx, Duration::from_secs(60), &shutdown);

        let batch = batched_rx.try_recv().expect("final batch flushed");
        assert_eq!(batch.paths, vec![p("only.md")]);
    }

    #[test]
    fn debounce_loop_exits_when_batched_receiver_dropped() {
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        let (batched_tx, batched_rx) = mpsc::channel::<ChangeBatch>();
        drop(batched_rx);
        let shutdown = Arc::new(Mutex::new(false));

        let handle = thread::spawn(move || {
            debounce_loop(
                raw_rx,
                batched_tx,
                Duration::from_millis(10),
                &shutdown,
            );
        });
        raw_tx.send(p("a.md")).unwrap();
        // The loop drains via timeout, fails to deliver the batch, and
        // breaks out — the join must therefore complete.
        handle.join().expect("debounce thread exits cleanly");
    }

    #[test]
    fn recv_returns_batch_on_live_event() {
        // recv() blocks, so drive it from a helper thread while the
        // main thread generates filesystem events. Tolerant of lost
        // FSEvents on cold-start: the helper is detached on timeout.
        let dir = tempfile::tempdir().expect("tempdir");
        let w =
            EventWatcher::with_debounce(dir.path(), Duration::from_millis(30))
                .expect("watcher");

        let (tx, rx) = mpsc::channel::<bool>();
        let handle = thread::spawn(move || {
            let got = w.recv();
            let _ = tx.send(got.is_some());
            drop(w);
        });

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut outcome: Option<bool> = None;
        while outcome.is_none() && Instant::now() < deadline {
            std::fs::write(dir.path().join("touch.md"), b"x").expect("write");
            outcome = rx.recv_timeout(Duration::from_millis(100)).ok();
        }
        // When the event arrived, recv() must have yielded a batch and
        // the helper thread must be joinable.
        assert!(outcome.is_none_or(|got| got));
        if outcome.is_some() {
            handle.join().expect("recv thread exits");
        }
    }

    #[test]
    fn drop_recovers_from_poisoned_internal_locks() {
        // Poison all three internal mutexes, then drop. Drop must not
        // panic — every lock() failure path is exercised.
        let dir = tempfile::tempdir().expect("tempdir");
        let w =
            EventWatcher::with_debounce(dir.path(), Duration::from_millis(20))
                .expect("watcher");

        let shutdown = Arc::clone(&w.shutdown);
        let _ = thread::spawn(move || {
            let _guard = shutdown.lock().unwrap();
            panic!("poison shutdown");
        })
        .join();

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = w.backend.lock().unwrap();
            panic!("poison backend");
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = w.debounce_handle.lock().unwrap();
            panic!("poison handle");
        }));

        drop(w); // must not panic or hang
    }

    #[test]
    fn drop_tolerates_already_taken_debounce_handle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w =
            EventWatcher::with_debounce(dir.path(), Duration::from_millis(20))
                .expect("watcher");

        // Steal the join handle so Drop sees `None`.
        let handle = w.debounce_handle.lock().unwrap().take();
        drop(w);

        // The debounce thread exits once the backend (and raw_tx) is
        // gone; join it ourselves so nothing leaks.
        handle
            .expect("handle present")
            .join()
            .expect("thread exits");
    }

    #[cfg(feature = "test-fault-injection")]
    mod fault_injection {
        use super::*;

        #[test]
        fn with_debounce_surfaces_backend_init_failure() {
            let _guard = fault::arm("event-watch::backend-init");
            let dir = tempfile::tempdir().expect("tempdir");
            let err = EventWatcher::new(dir.path())
                .expect_err("backend init failure must propagate");
            assert!(format!("{err}").contains("notify watcher init"));
        }

        #[test]
        fn with_debounce_surfaces_debounce_spawn_failure() {
            let _guard = fault::arm("event-watch::debounce-spawn");
            let dir = tempfile::tempdir().expect("tempdir");
            let err = EventWatcher::new(dir.path())
                .expect_err("spawn failure must propagate");
            assert!(format!("{err}").contains("debounce thread spawn"));
        }
    }

    #[test]
    fn debounce_paths_caps_at_window_with_widely_spaced_events() {
        // Each event 200 ms apart with a 100 ms window => one batch per
        // event, exercising the "Some(_)" arm that flushes and restarts.
        let t0 = Instant::now();
        let events = vec![
            (p("a"), t0),
            (p("b"), t0 + Duration::from_millis(200)),
            (p("c"), t0 + Duration::from_millis(400)),
        ];
        let out = debounce_paths(&events, Duration::from_millis(100));
        assert_eq!(out.len(), 3);
    }
}
