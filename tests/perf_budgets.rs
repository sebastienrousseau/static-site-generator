// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hard performance budgets — these tests fail CI when wall-clock
//! exceeds the documented threshold.
//!
//! Where `tests/perf_regression.rs` checks fine-grained per-operation
//! sub-millisecond budgets (slugify, URL parsing, frontmatter walk),
//! this suite checks **end-to-end build budgets** at realistic page
//! counts, against the same `compile_site` entry point a user
//! invokes via `cargo run`.
//!
//! ## Budget table (resolves issue #471 P0 + analysis batch E)
//!
//! | Pages | Local M-arm64 baseline | CI budget (this gate) |
//! |-------|------------------------|------------------------|
//! |   10  | < 5 ms                | **< 100 ms**           |
//! |  100  | 18.7 ms (full build)  | **< 500 ms**           |
//! |  500  | extrapolated < 100 ms | **< 2,000 ms**         |
//!
//! The CI budgets are deliberately ~10× the local baseline to absorb
//! GitHub Actions runner variance (cold-cache compile, shared CPU,
//! macOS/Windows runners ~3× slower than Linux). They are *ceilings*
//! — any genuine algorithmic regression will blow well past the
//! ceiling. The aggressive sub-50ms target tracked in #471 lives in
//! `tests/perf_regression.rs` once the bench corpus template is
//! fixed.
//!
//! ## Skip behaviour
//!
//! These tests run a real `compile_site`, which requires the
//! example template directory. If it's missing (rare, only on
//! corrupted checkouts), the tests skip with an explanatory
//! message rather than failing.
//!
//! ## Determinism
//!
//! Every test generates a deterministic corpus once (`generate_corpus`
//! mirrors the bench harness in `benches/bench_scalability.rs`) and
//! builds that same corpus for its warmup and every measured run, each
//! into fresh output directories. Writing the source files is never
//! adjacent to a timed build, and the warmup reads the very files the
//! measured runs read. The absolute budgets take the **fastest** of five
//! measured runs, because runner interference on a single build is
//! one-sided (see `best_of`); the scaling gate takes the median of five
//! paired ratios, because interference on a ratio is not (see
//! `growth_estimate`). The tests that time builds run one at a time
//! (see `TIMING`).
//!
//! ## Two kinds of gate
//!
//! The absolute ceilings in the table above are enforced on Linux only,
//! where they were calibrated. A wall-clock ceiling measures the machine
//! as much as the code, and on the shared Windows runner the same
//! 10-page build produced 131.00 ms and then 323.61 ms on consecutive
//! runs.
//!
//! The gate that runs everywhere is
//! `build_cost_per_page_does_not_grow_with_corpus_size`, which compares
//! per-page cost at two large corpus sizes. Both measurements happen on
//! the same machine moments apart, so machine speed divides out and what
//! remains is whether compilation still scales linearly — the regression
//! class actually worth gating.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

// A crate root looks for its modules beside it, so the path is explicit.
#[path = "perf_budgets/scaling.rs"]
pub mod scaling;

use scaling::{
    growth_estimate, scaling_rounds, MAX_PER_PAGE_GROWTH, SCALE_LARGE,
    SCALE_SMALL, SMALL_PER_ROUND,
};

/// Generates `n` synthetic Markdown files with realistic frontmatter.
/// Matches the harness in `benches/bench_scalability.rs:generate_corpus`
/// so timings here align with the criterion baseline.
fn generate_corpus(dir: &Path, n: usize) {
    fs::create_dir_all(dir).expect("create content dir");
    for i in 0..n {
        let content = format!(
            "---\n\
             title: \"Page {i}\"\n\
             date: \"2026-01-15T09:00:00+00:00\"\n\
             description: \"Performance budget page {i}\"\n\
             language: \"en-GB\"\n\
             layout: \"page\"\n\
             permalink: \"https://example.com/page-{i}\"\n\
             charset: \"utf-8\"\n\
             viewport: \"width=device-width, initial-scale=1, shrink-to-fit=no\"\n\
             author: \"hello@example.com\"\n\
             cdn: \"https://cloudcdn.pro\"\n\
             copyright: \"Copyright © 2026. All rights reserved.\"\n\
             hreflang: \"en\"\n\
             id: \"https://example.com\"\n\
             image: \"data:image/svg+xml;utf8,<svg></svg>\"\n\
             logo_alt: \"Logo\"\n\
             logo_height: \"33\"\n\
             logo_width: \"100\"\n\
             logo: \"\"\n\
             name: \"Benchmark\"\n\
             short_name: \"kaishi\"\n\
             subtitle: \"Performance budget page {i}\"\n\
             theme-color: \"26, 58, 138\"\n\
             url: \"https://example.com/page-{i}\"\n\
             item_pub_date: \"2026-01-15T09:00:00+00:00\"\n\
             last_build_date: \"2026-01-15T09:00:00+00:00\"\n\
             primary: \"\"\n\
             opengraph: \"\"\n\
             apple: \"\"\n\
             microsoft: \"\"\n\
             twitter: \"\"\n\
             ---\n\n\
             # Page {i}\n\n\
             Body content for performance budget testing.\n\n\
             ## Section\n\n\
             - Item 1\n\
             - Item 2\n\
             - Item 3\n"
        );
        let filename = if i == 0 {
            "index.md".to_string()
        } else {
            format!("page-{i}.md")
        };
        fs::write(dir.join(filename), content).expect("write page");
    }
}

/// A generated corpus and its templates, written once and built many
/// times.
///
/// Generating the corpus inside every timed sample put 100 or 500 fresh
/// file writes immediately before each build, so on a Windows runner any
/// real-time scanning of those new files could run inside the timed
/// build. Whether it did on #805 is not known; with the writes moved out
/// of the timing, the question does not arise. It also meant the warmup
/// primed a corpus the measured runs never read. Here the source tree is
/// written once, before any timing, and every build reads the same
/// files.
struct Corpus {
    root: tempfile::TempDir,
    content: PathBuf,
    template: PathBuf,
}

impl Corpus {
    /// Writes `n` pages and copies the example templates. `None` if the
    /// templates are missing on this checkout (rare).
    fn generate(n: usize) -> Option<Self> {
        let root = tempfile::tempdir().expect("tempdir");
        let content = root.path().join("content");
        let template = root.path().join("templates");
        fs::create_dir_all(&template).unwrap();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
        let src_tpl = workspace.join("examples/templates/en");
        if src_tpl.is_dir() {
            for entry in fs::read_dir(&src_tpl).unwrap().flatten() {
                let _ =
                    fs::copy(entry.path(), template.join(entry.file_name()));
            }
        }
        if fs::read_dir(&template).map_or(true, |mut d| d.next().is_none()) {
            return None;
        }
        generate_corpus(&content, n);
        Some(Self {
            root,
            content,
            template,
        })
    }

    /// Compiles the corpus into fresh output directories and returns the
    /// wall-clock time of `compile_site` alone. The outputs are removed
    /// after the clock stops.
    fn build(&self) -> Duration {
        let out = tempfile::tempdir_in(self.root.path()).expect("tempdir");
        let build = out.path().join("build");
        let site = out.path().join("site");
        for d in [&build, &site] {
            fs::create_dir_all(d).unwrap();
        }
        let start = Instant::now();
        let _ = ssg::compile_site(&build, &self.content, &site, &self.template);
        start.elapsed()
    }
}

/// Held by every test that times a build, so they run one at a time.
///
/// The harness runs tests on parallel threads, and these builds share
/// the process's rayon pool, its disk and its CPUs. On the #805 failure
/// the 100-page test was still running for the first 32 of the scaling
/// gate's 77 seconds (the 10-page test for the first 7), so whichever
/// samples overlapped them paid for both. A timing test should measure
/// its own build.
static TIMING: Mutex<()> = Mutex::new(());

fn timing_lock() -> MutexGuard<'static, ()> {
    TIMING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Samples taken per measurement.
const SAMPLES: usize = 5;

/// The fastest of [`SAMPLES`] runs, after a warmup iteration.
///
/// The minimum, not the median. Interference on a shared runner is
/// one-sided: contention, page-cache misses and co-tenant load make a
/// run slower and never faster, so the fastest observed run is the
/// least-contaminated estimate of the true cost. A median still carries
/// whatever noise the middle sample happened to pick up.
///
/// That is not a stylistic preference — it was measured. Switching from
/// median-of-3 to min-of-5 moved the local 10-page figure from 128 ms to
/// roughly 45 ms on the same machine and the same code. The old number
/// was mostly noise, which is precisely why budgets calibrated against
/// it kept moving.
fn best_of(n: usize) -> Option<Duration> {
    let corpus = Corpus::generate(n)?;
    // Warmup primes the file-system cache with the files the measured
    // runs read.
    let _ = corpus.build();
    (0..SAMPLES).map(|_| corpus.build()).min()
}

/// Whether the wall-clock budgets are enforced on this platform.
///
/// Only on Linux, and that is a deliberate narrowing rather than a
/// weakening. These budgets are calibrated against the Linux runner —
/// the numbers in `docs/perf/baseline-100p.md` were taken there, and the
/// `examples` job enforces them there — and a wall-clock ceiling is only
/// meaningful against a baseline measured on the same kind of machine.
///
/// The Windows runner is not that kind of machine. Measured on identical
/// code, the 10-page build came in at:
///
///   * 131.08 ms — a local macOS run
///   * 131.00 ms — `test · windows-latest`, one run
///   * 323.61 ms — `test · windows-latest`, the next run
///
/// That is 2.5x run-to-run variance on a shared, virtualised runner
/// whose filesystem calls dominate a build this small. A ceiling loose
/// enough never to flake there is far too loose to catch the
/// algorithmic regression these tests exist for, so it would be a gate
/// that reports success without asserting anything — the failure mode
/// this suite is meant to prevent.
///
/// This replaced a 3x platform multiplier, which was itself picked from
/// a single 131 ms sample and duly failed at 323.61 ms on the next run.
/// Choosing another constant from a fourth sample would repeat the
/// mistake; the honest fix is to assert only where the measurement
/// means something.
///
/// Off Linux the build still runs — so a panic, a hang or wrong output
/// is still caught on every platform — and the timing is printed for
/// information rather than asserted on.
const fn budgets_are_enforced_here() -> bool {
    cfg!(target_os = "linux")
}

fn assert_under_budget(label: &str, actual: Duration, budget: Duration) {
    if !budgets_are_enforced_here() {
        eprintln!(
            "[perf_budgets] {label}: {actual:.2?} (informational — the \
             budget of {budget:.2?} is enforced on Linux only; see \
             `budgets_are_enforced_here`)"
        );
        return;
    }
    assert!(
        actual <= budget,
        "{label}: {actual:.2?} exceeds budget {budget:.2?} \
         — this is a hard CI gate; please profile the regression \
         (see docs/perf/baseline-100p.md for the recipe)"
    );
    eprintln!("[perf_budgets] {label}: {actual:.2?} (budget {budget:.2?})");
}

// =====================================================================
// Budget gates
// =====================================================================

/// The gate that survives a noisy runner: per-page cost must not grow
/// as the corpus grows.
///
/// # Why this exists alongside the absolute budgets
///
/// A wall-clock ceiling measures the machine at least as much as the
/// code. On the shared, virtualised Windows runner the same 10-page
/// build produced 131.00 ms on one run and 323.61 ms on the next —
/// 2.5x variance on identical code — so any ceiling loose enough never
/// to flake there is far too loose to catch a real regression.
///
/// A ratio does not have that problem. Both measurements are taken on
/// the same machine moments apart, so machine speed divides out almost
/// entirely, and what is left is the property the suite actually cares
/// about: whether compilation still scales linearly in the number of
/// pages. That is the regression class worth gating — an accidental
/// O(n^2) in a lookup, a per-page full-corpus scan — and it shows up
/// here as a growth factor of ~5 rather than a few percent.
///
/// Both sizes are deliberately large. Measured spread across rounds was
/// 26% at n=10 but 1% at n=500: fixed start-up cost and scheduler
/// jitter are roughly constant, so they dominate a small corpus and
/// amortise on a big one. Comparing two large corpora is what makes
/// this stable enough to assert on every platform.
#[test]
fn build_cost_per_page_does_not_grow_with_corpus_size() {
    let _timing = timing_lock();
    // Both corpora are written before anything is timed.
    let (Some(small), Some(large)) =
        (Corpus::generate(SCALE_SMALL), Corpus::generate(SCALE_LARGE))
    else {
        eprintln!("[perf_budgets] templates missing — skipping scaling");
        return;
    };
    let rounds = scaling_rounds(|n| {
        if n == SCALE_SMALL {
            small.build()
        } else {
            large.build()
        }
    });
    let growth = growth_estimate(&rounds);

    // Every round, so a failure shows whether the growth was uniform
    // (the code) or confined to some rounds (the machine).
    for (i, round) in rounds.iter().enumerate() {
        eprintln!(
            "[perf_budgets] round {}: {SMALL_PER_ROUND} x {SCALE_SMALL} \
             pages {:.0?}, {SCALE_LARGE} pages {:.2?}, growth {:.2}x",
            i + 1,
            round.small,
            round.large,
            round.growth(),
        );
    }
    eprintln!(
        "[perf_budgets] per-page growth {SCALE_SMALL} -> {SCALE_LARGE} \
         pages: median {growth:.2}x over {SAMPLES} rounds (limit \
         {MAX_PER_PAGE_GROWTH:.2}x)"
    );

    assert!(
        growth <= MAX_PER_PAGE_GROWTH,
        "per-page build cost grew {growth:.2}x between {SCALE_SMALL} and \
         {SCALE_LARGE} pages (limit {MAX_PER_PAGE_GROWTH:.2}x). Linear \
         scaling keeps this at or below 1.0; a value near 5 means the \
         pipeline has picked up a per-page pass over the whole corpus. \
         This gate is machine-independent, so a shared or slow runner is \
         not the explanation — profile it."
    );
}

#[test]
fn build_10_pages_within_budget() {
    let _timing = timing_lock();
    let Some(t) = best_of(10) else {
        eprintln!("[perf_budgets] templates missing — skipping 10-page");
        return;
    };

    // Reported, not asserted — even on Linux.
    //
    // A 10-page build is almost entirely fixed start-up cost, and the
    // measured spread across rounds is 26% at this size against 1% at
    // n=500. A ceiling on that measures the runner, not the code.
    //
    // The proof arrived from CI rather than from argument: commit
    // 3cc1e96 produced two runs of the same workflow on the same code,
    // one passing and one failing at 103.27 ms against a 100 ms
    // ceiling. Two opposite verdicts, same commit, three per cent
    // apart.
    //
    // Raising the number would be the fourth time a constant here was
    // fitted to whichever sample had just failed. The gate that catches
    // the regression this suite exists for is
    // `build_cost_per_page_does_not_grow_with_corpus_size`, which is
    // machine-independent; the 100- and 500-page ceilings still catch a
    // uniform slowdown, and they have real headroom — 147-188 ms
    // against 800 ms, and ~800 ms against 2 s.
    eprintln!(
        "[perf_budgets] 10-page build: {t:.2?} (informational — too \
         noisy at this size to assert; see \
         build_cost_per_page_does_not_grow_with_corpus_size)"
    );
}

#[test]
fn build_100_pages_within_budget() {
    let _timing = timing_lock();
    let Some(t) = best_of(100) else {
        eprintln!("[perf_budgets] templates missing — skipping 100-page");
        return;
    };
    // Budget raised from 500 → 800 ms in v0.0.45 (#583) to absorb the
    // `content_stager` shim cost: one pre-staging pass that copies +
    // frontmatter-transforms every `.md`, plus a second pass that
    // injects template-default keys. The shim itself is parallelised
    // via Rayon, but the per-file I/O is still on the critical path
    // until upstream issues #67–#71 land (tracked in #585) and the
    // shim is deleted in v0.0.46.
    assert_under_budget("100-page build", t, Duration::from_millis(800));
}

#[test]
#[ignore = "500-page build is slow on cold runners; opt-in via \
            `cargo test --test perf_budgets -- --ignored`"]
fn build_500_pages_within_budget() {
    let _timing = timing_lock();
    let Some(t) = best_of(500) else {
        eprintln!("[perf_budgets] templates missing — skipping 500-page");
        return;
    };
    assert_under_budget("500-page build", t, Duration::from_secs(2));
}
