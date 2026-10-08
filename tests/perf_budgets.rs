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
//! Every test creates a fresh `TempDir`, generates a deterministic
//! corpus (`generate_corpus` mirrors the bench harness in
//! `benches/bench_scalability.rs`), runs one warmup iteration to prime
//! caches, then takes the **fastest** of five measured runs. The
//! minimum rather than a median, because runner interference is
//! one-sided — see `best_of`.
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
    time::{Duration, Instant},
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

/// Returns a fresh tempdir + the four directories the build needs.
fn fresh_layout() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let content = tmp.path().join("content");
    let build = tmp.path().join("build");
    let site = tmp.path().join("site");
    let template = tmp.path().join("templates");
    for d in [&content, &build, &site, &template] {
        fs::create_dir_all(d).unwrap();
    }
    // Copy example templates so the compiler can render. If templates
    // are missing, the test signals a skip via the empty TempDir
    // marker in the fingerprint (see `compile_n_pages`).
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_tpl = workspace.join("examples/templates/en");
    if src_tpl.is_dir() {
        for entry in fs::read_dir(&src_tpl).unwrap().flatten() {
            let _ = fs::copy(entry.path(), template.join(entry.file_name()));
        }
    }
    (tmp, content, build, site, template)
}

/// Compiles a corpus of `n` pages, returning the wall-clock duration.
/// `None` if templates are missing on this checkout (rare).
fn compile_n_pages(n: usize) -> Option<Duration> {
    let (_tmp, content, build, site, template) = fresh_layout();
    let template_empty =
        fs::read_dir(&template).map_or(true, |mut d| d.next().is_none());
    if template_empty {
        return None;
    }
    generate_corpus(&content, n);

    let start = Instant::now();
    let _ = ssg::compile_site(&build, &content, &site, &template);
    Some(start.elapsed())
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
    // Warmup primes the file-system cache for the measured runs.
    let _ = compile_n_pages(n)?;
    let mut best = compile_n_pages(n)?;
    for _ in 1..SAMPLES {
        let sample = compile_n_pages(n)?;
        best = best.min(sample);
    }
    Some(best)
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

/// Corpus sizes for the scaling gate. Both are large enough that fixed
/// start-up cost has amortised, which is what makes the comparison
/// stable — see [`build_cost_per_page_does_not_grow_with_corpus_size`].
const SCALE_SMALL: usize = 100;
const SCALE_LARGE: usize = 500;

/// The best small and large build times for the scaling gate, each the
/// fastest of [`SAMPLES`] runs after a warmup.
///
/// Interleaved, small then large in every round. Taking all the small
/// samples first and all the large ones after meant a runner that slowed
/// down part-way through charged the slowdown to the large corpus alone,
/// which reads as super-linear scaling: that is how identical code went
/// from a pass to 3.52x on `windows-latest` (#794). Interleaved, a
/// slowdown lands on both sizes, and the best-of in each still finds the
/// fast rounds.
fn scaling_samples<F>(mut build: F) -> Option<(Duration, Duration)>
where
    F: FnMut(usize) -> Option<Duration>,
{
    let _ = build(SCALE_SMALL)?;
    let _ = build(SCALE_LARGE)?;
    let mut small = build(SCALE_SMALL)?;
    let mut large = build(SCALE_LARGE)?;
    for _ in 1..SAMPLES {
        small = small.min(build(SCALE_SMALL)?);
        large = large.min(build(SCALE_LARGE)?);
    }
    Some((small, large))
}

/// Per-page growth that [`scaling_samples`] reports for a simulated
/// machine, so the estimator itself can be tested without a build.
fn simulated_growth<F>(mut cost: F) -> f64
where
    F: FnMut(usize, usize) -> f64,
{
    let mut call = 0_usize;
    let (small, large) = scaling_samples(|n| {
        call += 1;
        Some(Duration::from_secs_f64(cost(n, call)))
    })
    .expect("the simulated build never fails");
    (large.as_secs_f64() / SCALE_LARGE as f64)
        / (small.as_secs_f64() / SCALE_SMALL as f64)
}

/// A runner that slows down part-way through the gate must not read as
/// super-linear scaling. This is the shape of the failure on #794
/// (3.52x on `windows-latest`, then a pass on the same commit): the
/// machine got slower after the first few builds, and only the later
/// measurements paid for it.
#[test]
fn a_runner_slowing_mid_gate_is_not_read_as_super_linear_scaling() {
    // Linear cost, 1 ms per page; from the seventh build on, the
    // machine runs 3.5x slower and stays that way.
    let growth = simulated_growth(|n, call| {
        let slow = if call >= 7 { 3.5 } else { 1.0 };
        n as f64 * 0.001 * slow
    });
    assert!(
        growth <= MAX_PER_PAGE_GROWTH,
        "a mid-gate slowdown read as {growth:.2}x growth"
    );
}

/// The gate must still catch the regression it exists for.
#[test]
fn quadratic_cost_still_fails_the_scaling_gate() {
    let growth = simulated_growth(|n, _| (n as f64).powi(2) * 1e-6);
    assert!(growth > MAX_PER_PAGE_GROWTH, "O(n^2) read as {growth:.2}x");
}

/// How much per-page cost may grow between the two sizes.
///
/// Measured growth across four runs, including two on a machine loaded
/// by a concurrent test suite, was 0.47, 0.50, 1.14 and 1.19 — at or
/// below parity, as linear scaling predicts once fixed cost amortises.
///
/// The threshold sits well clear of both ends. Above it: O(n^2) puts
/// this at 5.0 and O(n^1.5) at 2.24, both caught; O(n log n) reaches
/// only 1.35, correctly allowed. Below it: 1.7x headroom over the worst
/// observed legitimate sample.
///
/// Noise is not one-sided across the whole gate, though. A runner that
/// slows down part-way through inflates whichever measurements come
/// after the slowdown; with all large samples taken last, that read as
/// 3.52x on `windows-latest` for code that passed on the next run.
/// [`scaling_samples`] interleaves the two sizes so a slowdown lands on
/// both, and two tests pin the estimator against a simulated machine.
const MAX_PER_PAGE_GROWTH: f64 = 2.0;

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
    let Some((small, large)) = scaling_samples(compile_n_pages) else {
        eprintln!("[perf_budgets] templates missing — skipping scaling");
        return;
    };

    let per_page_small = small.as_secs_f64() / SCALE_SMALL as f64;
    let per_page_large = large.as_secs_f64() / SCALE_LARGE as f64;
    let growth = per_page_large / per_page_small;

    eprintln!(
        "[perf_budgets] per-page cost: {:.3}ms at {SCALE_SMALL} pages, \
         {:.3}ms at {SCALE_LARGE} pages (growth {growth:.2}x, \
         limit {MAX_PER_PAGE_GROWTH:.2}x)",
        per_page_small * 1000.0,
        per_page_large * 1000.0,
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
    let Some(t) = best_of(500) else {
        eprintln!("[perf_budgets] templates missing — skipping 500-page");
        return;
    };
    assert_under_budget("500-page build", t, Duration::from_secs(2));
}
