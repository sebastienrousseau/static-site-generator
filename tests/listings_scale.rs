// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Scale gate for named listings (#587).
//!
//! The tracker's definition of done says `ListingsPlugin` must handle a
//! 10,000-item corpus quickly, with "cached frontmatter, single-pass
//! pagination". That is two claims, and only one of them is about time:
//!
//! 1. **Cost does not scale with listing count.** Each listing is a
//!    filtered view of one set of entries that was read once. A listing
//!    that re-read the sidecars would read them once per listing, and
//!    the wall-clock difference between one listing and eight would be
//!    roughly eightfold.
//! 2. **Cost does not scale with page count.** Pagination slices an
//!    already-sorted vector rather than re-scanning per page.
//!
//! Wall-clock assertions are the weakest kind of test — a loaded runner
//! can miss any budget. So the budget here is a *ceiling* set well above
//! the local baseline, in the same spirit as `tests/perf_budgets.rs`
//! (~10x), and the ratio assertion carries the algorithmic claim, which
//! is the part that actually regresses.

// `clippy.toml`'s `allow-expect-in-tests` only reaches `#[test]` functions
// and `#[cfg(test)]` modules. This is an integration test crate, built
// without `--cfg test`, so its module-level helpers need their own
// header — as the other suites in `tests/` already carry.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ssg::listings::ListingConfig;
use ssg::pagination::PaginationPlugin;
use ssg::plugin::{Plugin, PluginContext};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Items in the corpus. The tracker names 10,000.
const CORPUS: usize = 10_000;

/// Ceiling for the whole `after_compile` pass over `CORPUS` items.
///
/// The local M-arm64 baseline is well under a second. This is set far
/// above it so shared CI hardware cannot produce a false failure; a
/// genuine algorithmic regression (a re-read per listing, or per page)
/// blows past it by an order of magnitude rather than a few percent.
const BUDGET: Duration = Duration::from_secs(20);

fn corpus(meta: &Path, n: usize) {
    fs::create_dir_all(meta).expect("create .meta");
    for i in 0..n {
        // Spread across years, tags and languages so the filters have
        // real work rather than matching everything or nothing.
        let year = 2016 + (i % 10);
        let month = 1 + (i % 12);
        let day = 1 + (i % 28);
        let tag = ["rust", "wasm", "security", "perf"][i % 4];
        let lang = ["en", "fr", "de"][i % 3];
        // One of sixteen equal buckets. Sixteen listings filtered on these
        // cover the corpus exactly once between them, so they write
        // about as many pages in total as one unfiltered listing.
        let bucket = i % 16;
        let json = format!(
            r#"{{"title": "Post {i}", "date": "{year}-{month:02}-{day:02}", "tags": "{tag}", "language": "{lang}", "category": "b{bucket}"}}"#
        );
        fs::write(meta.join(format!("post{i}.meta.json")), json)
            .expect("write sidecar");
    }
}

fn ctx_with(dir: &TempDir, listings: Vec<ListingConfig>) -> PluginContext {
    let build = dir.path().join("build");
    let site = dir.path().join("site");
    fs::create_dir_all(&site).expect("create site");
    let mut cfg = ssg::cmd::default_config().as_ref().clone();
    cfg.site_name = "Scale".to_string();
    cfg.base_url = "https://example.com".to_string();
    cfg.listings = listings;
    PluginContext::with_config(dir.path(), &build, &site, dir.path(), cfg)
}

fn one_listing() -> Vec<ListingConfig> {
    vec![ListingConfig {
        name: "archive".to_string(),
        per_page: Some(20),
        ..ListingConfig::default()
    }]
}

/// Sixteen listings that partition the corpus.
///
/// Each takes one sixteenth, so between them they write about as many
/// pages as [`one_listing`] does. That matters: the first version of
/// this test compared one unfiltered listing against eight *additional*
/// ones, so the eight arm wrote far more pages. On macOS writes are
/// cheap and the ratio stayed near 1; on Windows they dominate and it
/// reached 2.1x on correct code. The metric was measuring write volume,
/// not reads.
fn many_listings() -> Vec<ListingConfig> {
    (0..16)
        .map(|b| ListingConfig {
            name: format!("b{b}"),
            category: Some(format!("b{b}")),
            per_page: Some(20),
            ..ListingConfig::default()
        })
        .collect()
}

fn timed(dir: &TempDir, listings: Vec<ListingConfig>) -> Duration {
    let ctx = ctx_with(dir, listings);
    let started = Instant::now();
    PaginationPlugin::default()
        .after_compile(&ctx)
        .expect("after_compile");
    started.elapsed()
}

/// One corpus, written once, reused by every measurement.
///
/// The first version of this file built a fresh corpus per measurement
/// and compared across them. That made eight listings look *faster*
/// than one — the second corpus was reading a warm filesystem cache, so
/// the ratio measured cache warmth rather than the algorithm.
fn corpus_dir() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    corpus(&dir.path().join("build").join(".meta"), CORPUS);
    dir
}

#[test]
fn ten_thousand_items_stay_within_the_budget() {
    let dir = corpus_dir();
    let elapsed = timed(&dir, one_listing());

    // The listing must actually have been written — a pass that wrote
    // nothing would be fast and meaningless.
    let page_one = dir.path().join("site/archive/index.html");
    assert!(
        page_one.is_file(),
        "no listing written: {}",
        page_one.display()
    );

    println!("[scale] {CORPUS} items in {elapsed:?} (ceiling {BUDGET:?})");
    assert!(
        elapsed < BUDGET,
        "{CORPUS} items took {elapsed:?}, over the {BUDGET:?} ceiling"
    );
}

/// Sixteen listings over the same corpus must not cost sixteen times one.
///
/// This is the "cached frontmatter" half of the claim, and unlike the
/// wall-clock ceiling it fails loudly on the regression it describes:
/// re-reading sidecars per listing turns this ratio into roughly 16.
#[test]
fn listing_count_does_not_multiply_the_work() {
    let dir = corpus_dir();

    // Warm the cache first so neither measurement pays for the other's
    // cold read.
    let _ = timed(&dir, one_listing());

    let one = timed(&dir, one_listing());
    let many = timed(&dir, many_listings());

    // The bound is calibrated by breaking the code, not guessed.
    // Measured on this corpus with sixteen partitions:
    //
    //   shared read (correct)        ~0.90x
    //   re-read per listing (broken) ~4.53x
    //
    // 2.5 sits between them with roughly 2.8x headroom over the correct
    // value, which Windows needs: file writes there are expensive
    // enough that an earlier version of this test — where the extra
    // listings wrote extra pages rather than partitioning the same
    // corpus — reached 2.1x on correct code and failed CI.
    let ratio = many.as_secs_f64() / one.as_secs_f64().max(0.001);
    println!("[scale] one={one:?} many={many:?} ratio={ratio:.2}x");
    assert!(
        ratio < 2.5,
        "sixteen listings took {many:?} against {one:?} for one \
         (ratio {ratio:.1}x) — sidecars look re-read per listing"
    );
}
