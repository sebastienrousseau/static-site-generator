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
        let json = format!(
            r#"{{"title": "Post {i}", "date": "{year}-{month:02}-{day:02}", "tags": "{tag}", "language": "{lang}"}}"#
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

fn eight_listings() -> Vec<ListingConfig> {
    let mut out = one_listing();
    for tag in ["rust", "wasm", "security", "perf"] {
        out.push(ListingConfig {
            name: tag.to_string(),
            tag: Some(tag.to_string()),
            per_page: Some(20),
            ..ListingConfig::default()
        });
    }
    for lang in ["en", "fr", "de"] {
        out.push(ListingConfig {
            name: format!("lang-{lang}"),
            language: Some(lang.to_string()),
            per_page: Some(20),
            ..ListingConfig::default()
        });
    }
    out
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

/// Eight listings over the same corpus must not cost eight times one.
///
/// This is the "cached frontmatter" half of the claim, and unlike the
/// wall-clock ceiling it fails loudly on the regression it describes:
/// re-reading sidecars per listing turns this ratio into roughly 8.
#[test]
fn listing_count_does_not_multiply_the_work() {
    let dir = corpus_dir();

    // Warm the cache first so neither measurement pays for the other's
    // cold read.
    let _ = timed(&dir, one_listing());

    let one = timed(&dir, one_listing());
    let eight = timed(&dir, eight_listings());

    // The bound is calibrated against the regression, not guessed. With
    // the shared read it measures ~0.85x; with a per-listing re-read of
    // all 10,000 sidecars it measures ~3.5x. An earlier version of this
    // test used 4.0 and so passed under the very regression it claims
    // to catch.
    let ratio = eight.as_secs_f64() / one.as_secs_f64().max(0.001);
    println!("[scale] one={one:?} eight={eight:?} ratio={ratio:.2}x");
    assert!(
        ratio < 2.0,
        "eight listings took {eight:?} against {one:?} for one \
         (ratio {ratio:.1}x) — sidecars look re-read per listing"
    );
}
