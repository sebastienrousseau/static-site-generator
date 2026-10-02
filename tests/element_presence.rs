// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Universal HTML element-presence regression suite.
//!
//! Walks every HTML file produced by the example-build phase
//! (`examples/*/public/**/*.html`) and asserts the universal
//! invariants every SSG-generated page must hold:
//!
//! - `<html lang="...">` declared (WCAG 3.1.1)
//! - exactly one `<title>` and it is non-empty (SEO)
//! - `<meta name="description">` present and non-empty (SEO)
//! - `<main>` landmark present (WCAG 1.3.1)
//! - exactly one `<h1>` (WCAG 1.3.1, SEO best practice)
//! - canonical URL declared via `<link rel="canonical">` (SEO)
//! - Open Graph chain: `og:title`, `og:description`, `og:type` (social)
//! - Twitter Card chain: `twitter:card` (social)
//! - viewport meta tag (mobile)
//! - charset declared (HTML5)
//!
//! These are checked element-by-element with file-and-tag-specific
//! failure messages so a regression points the reviewer at exactly
//! which invariant slipped, on which page.
//!
//! ## Why this is separate from `example_outputs`
//!
//! `example_outputs.rs` runs the example builds and exercises a
//! curated set of validators (preload `href`, mobile-menu CSS,
//! manifest icons, etc.). This suite focuses on the *universal*
//! HTML invariants — the things every page on every site must
//! satisfy regardless of the example's purpose.
//!
//! ## Skip behaviour
//!
//! When no built example output exists yet, the test prints a
//! message and exits cleanly (`cargo test --test element_presence`
//! does not require a full example build first). CI runs
//! `cargo build --examples && cargo test --test example_outputs`
//! before this test, populating the artifacts.
//!
//! Resolves the "no regressions / no divergence" requirement of
//! analysis batch E.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{fs, path::Path};

/// Handles the "nothing to scan" case for a gate that reads built
/// example output.
///
/// Locally this is a convenience: `cargo test --test element_presence`
/// on a fresh clone should say what to run rather than fail. Under
/// `CI` it is a failure, because a gate that scans zero pages and
/// reports `ok` is indistinguishable from one that scanned everything
/// and found nothing wrong.
///
/// That distinction is not hypothetical. `examples/multilingual_full`
/// carried 125 invariant violations for months while this suite
/// reported success on every run, because nothing in CI populated its
/// `public/` directory (#676). The gate was green and blind.
///
/// Returns `true` when the caller should return early.
fn bail_on_empty(gate: &str, count: usize) -> bool {
    if count > 0 {
        return false;
    }
    let msg = format!(
        "[{gate}] no built example output found under examples/*/public \
         — run `cargo build --examples && cargo test --test \
         example_outputs` first to populate."
    );
    // `SSG_REQUIRE_EXAMPLES` rather than `CI`.
    //
    // The intent of #676 stands: a gate that scans nothing must not report
    // success in the job that exists to run it. But keying on `CI` alone
    // meant the target hard-failed in *every* CI job, including the
    // per-OS `test` job, which does not build the examples and never
    // claimed to. The examples job sets this variable; everywhere else the
    // target skips, which is honest rather than green-and-blind because
    // the job that owns the check still enforces it.
    assert!(
        std::env::var_os("SSG_REQUIRE_EXAMPLES").is_none(),
        "{msg}\n\nThis is a hard failure where examples are required: a \
         gate that scans nothing must not report success."
    );
    eprintln!("{msg} Skipping (local run).");
    true
}

fn collect_html(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_html(&p, out);
        } else if p.extension().is_some_and(|e| e == "html") {
            out.push(p);
        }
    }
}

/// Counts non-overlapping case-insensitive substring matches.
fn count_ci(haystack: &str, needle: &str) -> usize {
    let h = haystack.to_lowercase();
    let n = needle.to_lowercase();
    let mut count = 0;
    let mut start = 0;
    while let Some(idx) = h[start..].find(&n) {
        count += 1;
        start += idx + n.len();
    }
    count
}

/// Returns true if `html` contains the (case-insensitive) substring,
/// matching the same lowercase normalisation as `count_ci`.
fn contains_ci(html: &str, needle: &str) -> bool {
    html.to_lowercase().contains(&needle.to_lowercase())
}
/// Extracts the value of a meta tag whose `name` attribute equals
/// `name_value`. Returns `None` if absent or value is empty.
fn meta_content(html: &str, name_value: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let pat = format!("name=\"{}\"", name_value.to_lowercase());
    let pat_single = format!("name='{}'", name_value.to_lowercase());
    let pat_none = format!("name={}", name_value.to_lowercase());
    let start = lower
        .find(&pat)
        .or_else(|| lower.find(&pat_single))
        .or_else(|| lower.find(&pat_none))?;
    // Walk forward to find content="..." or content='...' or content=...
    let after = &lower[start..];
    let cstart = after
        .find("content=\"")
        .or_else(|| after.find("content='"))
        .or_else(|| after.find("content="));
    if let Some(cs) = cstart {
        let after_content = &after[cs + "content=".len()..];
        if after_content.starts_with('"') || after_content.starts_with('\'') {
            let q = after_content.as_bytes()[0] as char;
            let after_q = &after_content[1..];
            let end = after_q.find(q)?;
            let value_start = start + cs + "content=".len() + 1;
            let value_end = value_start + end;
            let value = &html[value_start..value_end];
            if value.trim().is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        } else {
            // No quote, read until space or '>'
            let end =
                after_content.find(|c: char| c.is_whitespace() || c == '>')?;
            let value_start = start + cs + "content=".len();
            let value_end = value_start + end;
            let value = &html[value_start..value_end];
            if value.trim().is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
    } else {
        None
    }
}

/// Pages we exempt from the strict invariants because they are
/// generated artifacts that don't need full SEO/landmark structure
/// (e.g. 404, search results, redirect stubs).
fn is_exempt(rel_path: &str) -> bool {
    let lower = rel_path.to_lowercase();
    lower.ends_with("/404.html")
        || lower.ends_with("/offline.html")
        || lower.contains("/search/")
        || lower.contains("/tags/")
        || lower.contains("/categories/")
        || lower.contains("/topics/")
        || lower.contains("examples/basic/")
}

#[derive(Debug)]
struct Failure {
    path: std::path::PathBuf,
    invariant: &'static str,
    detail: String,
}

/// One document per page, for every page, exempt or not.
///
/// A page nested inside a page (two `<!DOCTYPE>`s, two `<html>`s, two
/// `<title>`s, two `<main>`s) is what the template plugin produced for
/// every bundled example until v0.0.66. "`<main>` present" let it
/// through, and `examples/basic` is exempt from the SEO checks, so no
/// gate here could fail on it. Structure is not an SEO property, so this
/// runs before [`is_exempt`].
fn check_structure(
    path: &Path,
    rel: &str,
    html: &str,
    failures: &mut Vec<Failure>,
) {
    for (needle, what) in [
        ("<!doctype", "<!DOCTYPE> declarations"),
        ("<html", "<html> elements"),
        ("<title>", "<title> elements"),
        ("<main", "<main> elements"),
    ] {
        let count = count_ci(html, needle);
        if count > 1 {
            failures.push(Failure {
                path: path.to_path_buf(),
                invariant: "document-count",
                detail: format!("page {rel}: {count} {what} (expected 1)"),
            });
        }
    }
}

fn check_invariants(
    path: &Path,
    rel: &str,
    html: &str,
    failures: &mut Vec<Failure>,
) {
    let mut fail = |invariant: &'static str, detail: String| {
        failures.push(Failure {
            path: path.to_path_buf(),
            invariant,
            detail,
        });
    };

    // 1. <html lang="..."> (WCAG 3.1.1)
    let lower = html.to_lowercase();
    if let Some(html_open) = lower.find("<html") {
        let tag_end = lower[html_open..]
            .find('>')
            .map_or(lower.len(), |e| html_open + e);
        let tag = &lower[html_open..tag_end];
        if !tag.contains("lang=") {
            fail("html-lang", format!("page {rel}: <html> missing lang"));
        }
    } else {
        fail("html-tag", format!("page {rel}: no <html> tag"));
    }

    // 2. Exactly one non-empty <title>
    let title_count = count_ci(html, "<title>");
    if title_count == 0 {
        fail("title-missing", format!("page {rel}: no <title>"));
    } else {
        let lower = html.to_lowercase();
        if let Some(start) = lower.find("<title>") {
            if let Some(end) = lower[start..].find("</title>") {
                let inner = html[start + 7..start + end].trim();
                if inner.is_empty() {
                    fail("title-empty", format!("page {rel}: <title> empty"));
                }
            }
        }
    }

    // 3. <meta name="description"> non-empty
    if meta_content(html, "description").is_none() {
        fail(
            "meta-description",
            format!("page {rel}: missing/empty <meta name=description>"),
        );
    }

    // 4. <main> landmark, and exactly one of it. A page nested inside a
    // page (two <!DOCTYPE>s, two <html>s, two <main>s) is what the
    // template plugin produced for every bundled example until v0.0.66,
    // and "present" let it through.
    let doctype_count = count_ci(html, "<!doctype");
    if doctype_count != 1 {
        fail(
            "document-count",
            format!("page {rel}: {doctype_count} <!DOCTYPE> declarations (expected 1)"),
        );
    }
    let main_count = count_ci(html, "<main");
    if main_count > 1 {
        fail(
            "main-count",
            format!("page {rel}: {main_count} <main> elements (expected 1)"),
        );
    }
    if !contains_ci(html, "<main") {
        fail(
            "main-landmark",
            format!("page {rel}: no <main> landmark (WCAG 1.3.1)"),
        );
    }

    // 5. Exactly one <h1> or <h2> as main heading
    let h1_count = count_ci(html, "<h1");
    let h2_count = count_ci(html, "<h2");
    if h1_count == 0 && h2_count == 0 {
        fail("h1-missing", format!("page {rel}: no <h1> or <h2>"));
    } else if h1_count > 1 {
        fail(
            "h1-multiple",
            format!("page {rel}: {h1_count} <h1> elements (expected 1)"),
        );
    }

    // 6. Canonical URL
    if !contains_ci(html, "rel=\"canonical\"")
        && !contains_ci(html, "rel='canonical'")
        && !contains_ci(html, "rel=canonical")
    {
        fail("canonical", format!("page {rel}: no <link rel=canonical>"));
    }

    // 7. Open Graph chain
    for og in ["og:title", "og:description", "og:type"] {
        let pat = format!("property=\"{og}\"");
        let pat_single = format!("property='{og}'");
        let pat_none = format!("property={og}");
        if !contains_ci(html, &pat)
            && !contains_ci(html, &pat_single)
            && !contains_ci(html, &pat_none)
        {
            fail("og-chain", format!("page {rel}: missing {og}"));
        }
    }

    // 8. Twitter Card
    if !contains_ci(html, "name=\"twitter:card\"")
        && !contains_ci(html, "name='twitter:card'")
        && !contains_ci(html, "name=twitter:card")
    {
        fail("twitter-card", format!("page {rel}: no twitter:card meta"));
    }

    // 9. Viewport
    if !contains_ci(html, "name=\"viewport\"")
        && !contains_ci(html, "name='viewport'")
        && !contains_ci(html, "name=viewport")
    {
        fail("viewport", format!("page {rel}: no viewport meta (mobile)"));
    }

    // 10. charset
    if !contains_ci(html, "charset=") {
        fail("charset", format!("page {rel}: no charset declared"));
    }
}

/// The full 10-invariant gate. **Always-on** in v0.0.45 (#495).
///
/// Originally aspirational because the bundled example templates
/// (basic, landing, plugins, blog taxonomy indexes) shipped without
/// `<h1>`, `<meta name=viewport>`, or the full Open Graph chain;
/// flipping the gate to always-on used to fail for those pages.
/// Subsequent template-coverage work (`v0.0.43` landing + view-
/// transitions, `v0.0.44` SEO `lol_html` port, the `v0.0.45` example
/// matrix sweep) closed those gaps. Every page under `examples/*/
/// public/` that isn't in [`is_exempt`] now satisfies all 10
/// invariants.
///
/// The smaller [`core_invariants_hold_for_every_page`] below is
/// retained as a fast, per-page sanity gate that runs even when
/// `examples/*/public/` hasn't been populated yet.
#[test]
fn every_built_example_page_satisfies_universal_invariants() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let examples = workspace.join("examples");

    let mut html_files = Vec::new();
    if examples.is_dir() {
        for entry in fs::read_dir(&examples).unwrap().flatten() {
            let public = entry.path().join("public");
            if public.is_dir() {
                collect_html(&public, &mut html_files);
            }
        }
    }

    if bail_on_empty("element_presence", html_files.len()) {
        return;
    }

    let mut failures = Vec::new();
    let mut pages_scanned = 0usize;
    let mut pages_exempted = 0usize;
    for path in &html_files {
        let rel = path
            .strip_prefix(workspace)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let html = fs::read_to_string(path).unwrap();
        check_structure(path, &rel, &html, &mut failures);
        if is_exempt(&rel) {
            pages_exempted += 1;
            continue;
        }
        check_invariants(path, &rel, &html, &mut failures);
        pages_scanned += 1;
    }

    if !failures.is_empty() {
        // Group by invariant for a scannable failure summary.
        let mut by_invariant: std::collections::BTreeMap<
            &'static str,
            Vec<&Failure>,
        > = std::collections::BTreeMap::new();
        for f in &failures {
            by_invariant.entry(f.invariant).or_default().push(f);
        }

        let mut msg = format!(
            "{} invariant violation(s) across {} page(s) (out of {} \
             scanned + {} exempted):\n",
            failures.len(),
            failures
                .iter()
                .map(|f| &f.path)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            pages_scanned,
            pages_exempted,
        );
        for (invariant, list) in &by_invariant {
            msg.push_str(&format!(
                "\n  ── {invariant} ({} failure(s)) ──\n",
                list.len()
            ));
            for f in list.iter().take(10) {
                msg.push_str(&format!("    {}\n", f.detail));
            }
            if list.len() > 10 {
                msg.push_str(&format!(
                    "    ... and {} more\n",
                    list.len() - 10
                ));
            }
        }
        panic!("{msg}");
    }

    eprintln!(
        "[element_presence] {} HTML page(s) verified against 10 \
         universal invariants ({} exempt)",
        pages_scanned, pages_exempted
    );
}

/// Per-page invariants every example output currently satisfies and
/// must continue to satisfy. This is the real per-PR regression
/// gate — running on every CI build via `ci.yml`'s `examples` job
/// after `example_outputs`.
///
/// Truly universal subset (4 invariants):
/// - `<html lang="...">` (WCAG 3.1.1)
/// - non-empty `<title>` (SEO)
/// - `<main>` landmark (WCAG 1.3.1)
/// - charset declared (HTML5)
///
/// Invariants covered here are a fast subset of the always-on
/// `every_built_example_page_satisfies_universal_invariants`
/// (above): `<html lang>`, `<title>` non-empty, `<main>` landmark,
/// charset declared. The bigger gate covers `<h1>`-exactly-once,
/// viewport meta, `<meta name=description>`, canonical URL, the
/// Open Graph chain (`og:title`/`og:description`/`og:type`), and
/// `twitter:card`.
fn check_core_invariants(
    path: &Path,
    rel: &str,
    html: &str,
    failures: &mut Vec<Failure>,
) {
    let mut fail = |invariant: &'static str, detail: String| {
        failures.push(Failure {
            path: path.to_path_buf(),
            invariant,
            detail,
        });
    };

    let lower = html.to_lowercase();
    if let Some(html_open) = lower.find("<html") {
        let tag_end = lower[html_open..]
            .find('>')
            .map_or(lower.len(), |e| html_open + e);
        let tag = &lower[html_open..tag_end];
        if !tag.contains("lang=") {
            fail("html-lang", format!("page {rel}: <html> missing lang"));
        }
    } else {
        fail("html-tag", format!("page {rel}: no <html> tag"));
    }

    let title_count = count_ci(html, "<title>");
    if title_count == 0 {
        fail("title-missing", format!("page {rel}: no <title>"));
    } else if let Some(start) = lower.find("<title>") {
        if let Some(end) = lower[start..].find("</title>") {
            let inner = html[start + 7..start + end].trim();
            if inner.is_empty() {
                fail("title-empty", format!("page {rel}: <title> empty"));
            }
        }
    }

    if !contains_ci(html, "<main") {
        fail(
            "main-landmark",
            format!("page {rel}: no <main> landmark (WCAG 1.3.1)"),
        );
    }

    if !contains_ci(html, "charset=") {
        fail("charset", format!("page {rel}: no charset declared"));
    }

    // meta-description, canonical, the og:* chain and twitter:card
    // are deliberately absent from *this* function. They are checked —
    // always, not opt-in — by [`check_invariants`], which
    // `every_built_example_page_satisfies_universal_invariants` runs
    // over the same pages minus [`is_exempt`]. Splitting them this way
    // keeps `core_invariants_hold_for_every_page` reporting the
    // structural failures (missing `<h1>`, no `<main>`, no charset)
    // without burying them under SEO noise.
    //
    // `examples/basic` is exempt from the SEO half because its bundled
    // template is a deliberately minimal single-page demo; every other
    // example gets those tags from the shared SEO plugins.
    //
    // This comment used to end by telling reviewers to opt into a
    // stricter run with `cargo test --test element_presence --
    // --ignored`. That command matches nothing: there is no `#[ignore]`
    // in this file, so it reports `0 passed; 6 filtered out` and looks
    // like a clean run. The instruction outlived the `#[ignore]` it
    // referred to. #668 corrected the same claim in
    // `docs/architecture/regression-contract.md` and missed this copy.
}

#[test]
fn core_invariants_hold_for_every_page() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let examples = workspace.join("examples");

    let mut html_files = Vec::new();
    if examples.is_dir() {
        for entry in fs::read_dir(&examples).unwrap().flatten() {
            let public = entry.path().join("public");
            if public.is_dir() {
                collect_html(&public, &mut html_files);
            }
        }
    }

    if bail_on_empty("element_presence/core", html_files.len()) {
        return;
    }

    let mut failures = Vec::new();
    let mut pages_scanned = 0usize;
    for path in &html_files {
        let rel = path
            .strip_prefix(workspace)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let html = fs::read_to_string(path).unwrap();
        check_structure(path, &rel, &html, &mut failures);
        if is_exempt(&rel) {
            continue;
        }
        check_core_invariants(path, &rel, &html, &mut failures);
        pages_scanned += 1;
    }

    if !failures.is_empty() {
        let mut by_invariant: std::collections::BTreeMap<
            &'static str,
            Vec<&Failure>,
        > = std::collections::BTreeMap::new();
        for f in &failures {
            by_invariant.entry(f.invariant).or_default().push(f);
        }
        let mut msg = format!(
            "{} core-invariant violation(s) across {} page(s) (out of \
             {} scanned):\n",
            failures.len(),
            failures
                .iter()
                .map(|f| &f.path)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            pages_scanned,
        );
        for (invariant, list) in &by_invariant {
            msg.push_str(&format!(
                "\n  ── {invariant} ({} failure(s)) ──\n",
                list.len()
            ));
            for f in list.iter().take(10) {
                msg.push_str(&format!("    {}\n", f.detail));
            }
            if list.len() > 10 {
                msg.push_str(&format!(
                    "    ... and {} more\n",
                    list.len() - 10
                ));
            }
        }
        panic!("{msg}");
    }

    eprintln!(
        "[element_presence] {pages_scanned} page(s) verified against 8 \
         core invariants"
    );
}

// ── Unit tests for the helpers ─────────────────────────────────────

#[test]
fn count_ci_finds_non_overlapping_matches() {
    assert_eq!(count_ci("<H1>a</h1><h1>b</H1>", "<h1"), 2);
    assert_eq!(count_ci("hello", "ll"), 1);
    assert_eq!(count_ci("aaaa", "aa"), 2);
}

#[test]
fn meta_content_extracts_from_double_quoted() {
    let html = r#"<meta name="description" content="Hi there">"#;
    assert_eq!(meta_content(html, "description"), Some("Hi there".into()));
}

#[test]
fn meta_content_returns_none_when_absent() {
    let html = r#"<meta name="other" content="x">"#;
    assert_eq!(meta_content(html, "description"), None);
}

#[test]
fn is_exempt_skips_404_and_search() {
    assert!(is_exempt("examples/blog/public/404.html"));
    assert!(is_exempt("examples/blog/public/search/index.html"));
    assert!(!is_exempt("examples/blog/public/index.html"));
}
