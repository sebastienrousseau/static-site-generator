// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Dev-server glue (issue #526) — wires [`EventWatcher`] → [`DepGraph`]
//! → [`HmrBroadcaster`] into one loop.
//!
//! Lifted out of `bin/dev.rs` so each policy decision (classify a change,
//! pick the HMR frame type, resolve invalidated outputs) is unit-testable
//! without spawning threads or binding a port.
//!
//! ```text
//!   FS event   ──▶ ChangeBatch ──▶ process_batch ──▶ HmrMessage ──▶ tabs
//!     (notify)        (debounced)    (DepGraph)        (broadcaster)
//! ```
//!
//! See [`process_batch`] for the policy table and [`run_dev_loop`] for
//! the integration point.

use std::path::{Path, PathBuf};

use crate::core_group::depgraph::DepGraph;
use crate::server_group::event_watch::{ChangeBatch, EventWatcher};
use crate::server_group::hmr::{HmrBroadcaster, HmrMessage};
use crate::server_group::watch::{classify_change, ChangeKind};

/// Outcome of processing one debounced batch — the frame (if any) the
/// caller should broadcast and the page paths invalidated by the batch
/// (used by the rebuild step).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOutcome {
    /// The HMR frame to push to every connected tab. `None` if every
    /// path in the batch was classified [`ChangeKind::Other`] (e.g.
    /// editor backup, build artefact under the watched root).
    pub frame: Option<HmrMessage>,
    /// Page output paths the dep graph says were invalidated. These
    /// drive the rebuild scope.
    pub invalidated_outputs: Vec<PathBuf>,
}

/// Policy table: turn a [`ChangeBatch`] + a [`DepGraph`] into the right
/// HMR frame.
///
/// Rules:
/// * **All CSS** → `hmr-css` (AC2) — list every changed `.css` path.
/// * **Markdown / HTML content (no templates)** → `hmr-html` (AC3) —
///   list invalidated output URLs from the dep graph.
/// * **Template touched** → `hmr-html` if the dep graph resolves
///   affected pages (AC4); falls back to `reload` if no pages are
///   linked (e.g. a brand-new template).
/// * **Mixed CSS + content** → `reload` to keep the protocol simple.
/// * **All other** (data, fonts, images that aren't fingerprinted) →
///   `reload` so the user always sees the latest build.
///
/// # Examples
///
/// ```rust
/// use ssg::dev_server::process_batch;
/// use ssg::depgraph::DepGraph;
/// use ssg::event_watch::ChangeBatch;
/// use std::path::Path;
///
/// let batch = ChangeBatch { paths: vec![] };
/// let outcome = process_batch(&batch, &DepGraph::new(), Path::new("build"));
/// assert!(outcome.frame.is_none());
/// ```
#[must_use]
pub fn process_batch(
    batch: &ChangeBatch,
    graph: &DepGraph,
    output_dir: &Path,
) -> BatchOutcome {
    if batch.is_empty() {
        return BatchOutcome {
            frame: None,
            invalidated_outputs: Vec::new(),
        };
    }

    let mut css_paths: Vec<String> = Vec::new();
    let mut content_or_template = false;
    let mut other = false;

    for path in &batch.paths {
        match classify_change(path) {
            ChangeKind::Css => {
                css_paths.push(path.to_string_lossy().into_owned());
            }
            ChangeKind::Content | ChangeKind::Template => {
                content_or_template = true;
            }
            ChangeKind::Other => {
                other = true;
            }
        }
    }

    let outputs = graph.invalidated_outputs(&batch.paths);
    let url_paths: Vec<String> = outputs
        .iter()
        .map(|p| output_to_url(p, output_dir))
        .collect();

    let frame = match (!css_paths.is_empty(), content_or_template, other) {
        // CSS only (no content/template/other) → hmr-css.
        (true, false, false) => Some(HmrMessage::css(css_paths)),
        // Pure content/template change (no CSS, no other) → hmr-html
        // with the dep-graph-resolved page list. If the graph has no
        // edges yet, the URL list is empty — fall back to reload.
        (false, true, false) => {
            if url_paths.is_empty() {
                Some(HmrMessage::reload())
            } else {
                Some(HmrMessage::html(url_paths))
            }
        }
        // Anything mixed, or only "other" → full reload.
        _ => Some(HmrMessage::reload()),
    };

    BatchOutcome {
        frame,
        invalidated_outputs: outputs,
    }
}

/// Convert an output filesystem path (e.g. `build/blog/foo/index.html`)
/// into the URL the browser tab is loaded at (`/blog/foo/`).
///
/// Strips the `output_dir` prefix and the trailing `index.html` so
/// directory-style URLs match the path the browser is sitting at.
///
/// # Examples
///
/// ```rust
/// use ssg::dev_server::output_to_url;
/// use std::path::Path;
///
/// let url = output_to_url(Path::new("build/blog/foo/index.html"), Path::new("build"));
/// assert_eq!(url, "/blog/foo/");
/// ```
#[must_use]
pub fn output_to_url(output: &Path, output_dir: &Path) -> String {
    let rel = output.strip_prefix(output_dir).unwrap_or(output);
    let mut s = rel.to_string_lossy().replace('\\', "/");
    // Strip trailing index.html to match clean URLs.
    for trailing in ["index.html", "index.htm"] {
        if let Some(stripped) = s.strip_suffix(trailing) {
            s = stripped.to_string();
            break;
        }
    }
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    s
}

/// Pumps debounced batches off `watcher` and broadcasts each frame.
///
/// The closure `rebuild` is called once per batch with the invalidated
/// output list so callers can plug in the incremental pipeline.
///
/// Returns the number of batches processed before the watcher closed,
/// which is useful for tests that inject a finite event stream.
///
/// # Examples
///
/// ```ignore
/// // Requires a real EventWatcher + HmrBroadcaster bound to a port,
/// // which a doctest sandbox can't safely set up. See the `dev` binary
/// // (`src/bin/dev.rs`) for a runnable wiring.
/// use ssg::dev_server::run_dev_loop;
/// ```
pub fn run_dev_loop<F: FnMut(&[PathBuf])>(
    watcher: &EventWatcher,
    graph: &DepGraph,
    broadcaster: &HmrBroadcaster,
    output_dir: &Path,
    mut rebuild: F,
) -> usize {
    let mut processed = 0usize;
    while let Some(batch) = watcher.recv() {
        let outcome = process_batch(&batch, graph, output_dir);
        rebuild(&outcome.invalidated_outputs);
        if let Some(frame) = outcome.frame {
            let _ = broadcaster.broadcast(&frame);
        }
        processed += 1;
    }
    processed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn empty_batch_yields_no_frame() {
        let batch = ChangeBatch { paths: vec![] };
        let graph = DepGraph::new();
        let out = process_batch(&batch, &graph, Path::new("build"));
        assert!(out.frame.is_none());
        assert!(
            out.invalidated_outputs.is_empty(),
            "{:?}",
            out.invalidated_outputs
        );
    }

    #[test]
    fn pure_css_batch_yields_hmr_css_frame() {
        let batch = ChangeBatch {
            paths: vec![pb("assets/style.css")],
        };
        let graph = DepGraph::new();
        let out = process_batch(&batch, &graph, Path::new("build"));
        let frame = out.frame.expect("frame");
        let json = frame.to_json();
        assert!(json.contains("hmr-css"));
        assert!(json.contains("assets/style.css"));
    }

    #[test]
    fn pure_content_batch_without_graph_falls_back_to_reload() {
        // No edges in the graph => empty invalidated list => reload.
        let batch = ChangeBatch {
            paths: vec![pb("content/blog/foo.md")],
        };
        let graph = DepGraph::new();
        let out = process_batch(&batch, &graph, Path::new("build"));
        let json = out.frame.unwrap().to_json();
        assert!(json.contains("\"type\":\"reload\""));
    }

    #[test]
    fn mixed_css_and_content_batch_yields_reload() {
        let batch = ChangeBatch {
            paths: vec![pb("assets/style.css"), pb("content/foo.md")],
        };
        let graph = DepGraph::new();
        let out = process_batch(&batch, &graph, Path::new("build"));
        assert!(out.frame.unwrap().to_json().contains("reload"));
    }

    #[test]
    fn other_only_batch_yields_reload() {
        let batch = ChangeBatch {
            paths: vec![pb("data/config.json")],
        };
        let graph = DepGraph::new();
        let out = process_batch(&batch, &graph, Path::new("build"));
        assert!(out.frame.unwrap().to_json().contains("reload"));
    }

    #[test]
    fn output_to_url_strips_index_html_and_prefix() {
        let out = pb("build/blog/foo/index.html");
        let url = output_to_url(&out, Path::new("build"));
        assert_eq!(url, "/blog/foo/");
    }

    #[test]
    fn output_to_url_keeps_non_index_files() {
        let out = pb("build/feed.xml");
        let url = output_to_url(&out, Path::new("build"));
        assert_eq!(url, "/feed.xml");
    }

    #[test]
    fn output_to_url_handles_root_index() {
        let out = pb("build/index.html");
        let url = output_to_url(&out, Path::new("build"));
        assert_eq!(url, "/");
    }

    #[test]
    fn output_to_url_normalises_backslashes() {
        let out = pb("build\\blog\\index.html");
        // strip_prefix won't match because backslashes aren't a path
        // separator on Unix. Use a path that strip_prefix accepts.
        let url = output_to_url(&out, Path::new("build"));
        // The function still normalises any embedded backslashes.
        assert!(url.starts_with('/'));
        assert!(!url.contains('\\'));
    }

    #[test]
    fn output_to_url_strips_index_htm_suffix() {
        // Covers the second loop iteration of the `trailing` array in
        // `output_to_url` — a path ending in the shorter "index.htm"
        // (no trailing "l") never matches the first `strip_suffix`
        // check, so the loop must advance to the second entry and take
        // its `if let Some(stripped)` branch.
        let out = pb("build/blog/index.htm");
        let url = output_to_url(&out, Path::new("build"));
        assert_eq!(url, "/blog/");
    }

    #[test]
    fn output_to_url_keeps_leading_slash_when_prefix_strip_fails() {
        // An absolute output path outside `output_dir` cannot be
        // stripped; it already starts with '/' so no slash is inserted.
        let out = pb("/abs/feed.xml");
        let url = output_to_url(&out, Path::new("build"));
        assert_eq!(url, "/abs/feed.xml");
    }

    #[test]
    fn run_dev_loop_processes_live_batches_until_watcher_closes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().expect("tempdir");
        let watcher =
            EventWatcher::with_debounce(dir.path(), Duration::from_millis(30))
                .expect("watcher");
        let graph = DepGraph::new();
        let broadcaster = HmrBroadcaster::new();

        let frames = std::sync::Arc::new(AtomicUsize::new(0));
        let frames_sink = std::sync::Arc::clone(&frames);
        broadcaster.subscribe(Box::new(move |_| {
            let _ = frames_sink.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        // Generate FS events and give the backend + debouncer a bounded
        // window to flush at least one batch into the channel. FSEvents
        // can drop cold-start events, so poll rather than sleep once.
        let deadline = Instant::now() + Duration::from_secs(3);
        while watcher
            .recv_timeout(Duration::from_millis(0))
            .batch()
            .is_none()
        {
            if Instant::now() >= deadline {
                break;
            }
            std::fs::write(dir.path().join("page.md"), b"x").expect("write");
            std::thread::sleep(Duration::from_millis(50));
        }

        // Closing the backend drops the raw sender; the debounce thread
        // flushes anything pending and exits, closing the batched
        // channel — which is what makes run_dev_loop return.
        std::fs::write(dir.path().join("late.md"), b"y").expect("write");
        std::thread::sleep(Duration::from_millis(80));
        watcher.close_backend_for_test();

        let rebuilds = AtomicUsize::new(0);
        let processed = run_dev_loop(
            &watcher,
            &graph,
            &broadcaster,
            Path::new("build"),
            |_outputs| {
                let _ = rebuilds.fetch_add(1, Ordering::SeqCst);
            },
        );

        // Every processed batch triggered exactly one rebuild and one
        // broadcast frame.
        assert_eq!(processed, rebuilds.load(Ordering::SeqCst));
        assert_eq!(processed, frames.load(Ordering::SeqCst));
    }

    #[test]
    fn pure_content_with_graph_edges_yields_hmr_html() {
        // Covers the line ~113 `Some(HmrMessage::html(url_paths))`
        // arm — a content change that resolves to one or more
        // outputs via the dep-graph.
        let mut graph = DepGraph::new();
        // Source content/blog/foo.md produces build/blog/foo/index.html.
        graph.add_output(
            Path::new("content/blog/foo.md"),
            Path::new("build/blog/foo/index.html"),
        );
        let batch = ChangeBatch {
            paths: vec![pb("content/blog/foo.md")],
        };
        let out = process_batch(&batch, &graph, Path::new("build"));
        let json = out.frame.unwrap().to_json();
        assert!(json.contains("hmr-html"), "expected hmr-html, got: {json}");
        assert!(
            json.contains("/blog/foo/"),
            "expected URL list to carry the page, got: {json}"
        );
    }
}
