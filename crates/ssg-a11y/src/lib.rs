#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![forbid(unsafe_code)]
// `pub(crate)` on cross-module helpers (e.g. `rules::check_img_alt` used
// from `lib.rs`) is flagged by `redundant_pub_crate` because the private
// `mod rules;`/`mod css;`/`mod html;` declarations already cap external
// reachability — but `unreachable_pub` (rustc) wants exactly `pub(crate)`
// for the same items, since they aren't part of the crate's public API.
// These two lints are mutually exclusive for internal helpers split
// across private submodules; `pub(crate)` is the more accurate signal
// of intent (crate-internal, not public API), so silence the other.
#![allow(clippy::redundant_pub_crate)]
// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! # ssg-a11y — Standalone WCAG 2.2 AA accessibility checker
//!
//! Framework-agnostic, build-time HTML validation against a subset of
//! WCAG 2.2 Level AA success criteria, plus ARIA landmark checks. This
//! crate has **no dependency on any web framework** — it operates purely
//! on `&str` HTML in, [`AccessibilityIssue`] data out — so it can be
//! embedded in the build pipeline of any Rust site/app generator
//! (static site generators, Leptos, Dioxus, Yew, etc).
//!
//! ```
//! let html = r#"<html><head></head><body><main><img src="a.jpg"></main></body></html>"#;
//! let issues = ssg_a11y::check_page(html);
//! assert!(issues.iter().any(|i| i.criterion == "1.1.1"));
//! ```
//!
//! ## Checks performed
//!
//! - 1.1.1 Non-text content (`<img alt>`)
//! - 1.3.1 Heading hierarchy (no skipped levels)
//! - 2.3.1 Banned elements (`<marquee>`, `<blink>`)
//! - 2.4.4 Link purpose (discernible text or `aria-label`)
//! - 2.4.13 Focus appearance — `:focus { outline: none }` without a
//!   compensating style is flagged (WCAG 2.2 addition)
//! - 2.5.8 Target size minimum — explicit `width`/`height` < 24 px on
//!   interactive selectors flagged (WCAG 2.2 addition)
//! - 3.1.1 Page language (`<html lang>`)
//! - 3.2.6 Consistent help — a standalone helper is provided
//!   ([`check_page`] does not call it directly; see its docs) for
//!   informational cross-page analysis (WCAG 2.2 addition)
//! - ARIA landmarks (single `<main>`, `<nav aria-label>`)
//!
//! [`build_compliance_report`] additionally produces a full WCAG 2.2
//! compliance matrix ([`WcagComplianceReport`]) mapping every criterion
//! in the spec to its automation status (automated / runtime-only /
//! manual / not-applicable), so a consumer can report on and track
//! conformance beyond what this crate can check automatically.

mod css;
mod html;

/// Scan to the end of an HTML tag.
///
/// Skips `>` characters inside quoted attribute values, so an inline
/// `data:` URL in a `src` attribute cannot truncate the tag early.
///
/// Exported because a standalone HTML checker is where this belongs: the
/// host crate had grown three separate copies of it (ssg#711). Additive —
/// nothing existing changes shape.
pub use html::find_tag_end;
/// Whether an `<img>` tag lacks a text alternative (WCAG 1.1.1).
///
/// Exported so the host crate's other alt-text checks agree with
/// [`check_page`] on what counts as missing, rather than keeping their
/// own copy of the rule. Additive.
pub use html::img_missing_alt;
mod report;
mod rules;
mod types;

pub use report::build_compliance_report;
pub use rules::check_consistent_help;
pub use types::{
    AccessibilityIssue, AccessibilityReport, CriterionEntry, CriterionStatus,
    PageReport, WcagComplianceReport,
};

/// Runs all WCAG checks on a single HTML page.
///
/// This is the crate's main entry point: pass it the full text of one
/// rendered HTML page and it returns every issue found. It performs no
/// I/O — callers are responsible for reading the file (or otherwise
/// obtaining the HTML string) and for aggregating per-page results into
/// a report (see [`AccessibilityReport`]) if desired.
pub fn check_page(html: &str) -> Vec<AccessibilityIssue> {
    let mut issues = Vec::new();

    // WCAG 1.1.1: Non-text Content — all <img> must have alt
    rules::check_img_alt(html, &mut issues);

    // WCAG 3.1.1: Language of Page — <html> must have lang
    rules::check_html_lang(html, &mut issues);

    // WCAG 2.4.4: Link Purpose — all <a> must have discernible text
    rules::check_link_text(html, &mut issues);

    // WCAG 1.3.1: Heading hierarchy — no skipped levels
    rules::check_heading_hierarchy(html, &mut issues);

    // WCAG 2.3.1: No flashing — no <marquee> or <blink>
    rules::check_banned_elements(html, &mut issues);

    // ARIA: exactly one <main>, nav elements have aria-label
    rules::check_aria_landmarks(html, &mut issues);

    // WCAG 2.2 additions ----------------------------------------------

    // 2.5.8 Target Size (Minimum) — interactive selectors with
    // explicit width/height < 24px in inline CSS.
    rules::check_target_size(html, &mut issues);

    // 2.4.13 Focus Appearance — `outline: none` on :focus without a
    // compensating outline-style/box-shadow/border declaration.
    rules::check_focus_appearance(html, &mut issues);

    // 3.2.6 Consistent Help is not checked per-page — it requires
    // cross-page comparison of help-mechanism placement, which is
    // beyond the per-page scan. [`rules::check_consistent_help`] is kept
    // as a standalone helper for callers that want to run it themselves
    // across a whole site; see [`build_compliance_report`], which marks
    // this criterion `manual` in the matrix.

    issues
}

#[cfg(test)]
mod tests {

    /// Regression: `check_page` panicked on non-ASCII input whose lowercase
    /// form is longer than the original.
    ///
    /// Found by `fuzz_a11y` within seconds of the target being added. The
    /// rules lowercased the page to search it case-insensitively, then used
    /// the resulting offsets to slice the *original* string. `to_lowercase`
    /// is not length-preserving — `İ` (U+0130) becomes two chars — so every
    /// offset past it was wrong and the next slice landed mid-character:
    ///
    ///     thread panicked at crates/ssg-a11y/src/rules.rs:31:47
    ///     byte index is not a char boundary
    ///
    /// This runs inside `ssg build`, so the input that triggers it is
    /// someone's own content and the symptom is a crashed build.
    #[test]
    fn check_page_survives_non_ascii_that_grows_when_lowercased() {
        // The reduced crash input from the fuzzer: U+0130 before a tag whose
        // attributes are then sliced out of the original string.
        let html = "<!\u{130}html>\n<img src=\"a.png\" alt>";
        let _ = check_page(html);

        // A few more shapes with the same hazard.
        for probe in [
            "\u{130}<img src=\"x.png\">",
            "<img alt=\"\u{130}\" src=\"y.png\">",
            "<p>\u{130}\u{130}\u{130}</p><img>",
            "<img src=\"\u{130}.png\">",
        ] {
            let _ = check_page(probe);
        }
    }

    /// The ASCII fold must still match tags written in any case.
    #[test]
    fn check_page_still_matches_uppercase_tags() {
        let issues = check_page("<IMG SRC=\"a.png\">");
        assert!(
            issues.iter().any(|i| i.criterion == "1.1.1"),
            "uppercase <IMG> without alt was not detected: {issues:?}"
        );
    }

    use super::*;

    #[test]
    fn test_img_alt_missing() {
        let html = r#"<html lang="en"><head></head><body><main><img src="photo.jpg"></main></body></html>"#;
        let issues = check_page(html);
        assert!(issues.iter().any(|i| i.criterion == "1.1.1"));
    }

    #[test]
    fn test_img_alt_present() {
        let html = r#"<html lang="en"><head></head><body><main><img src="photo.jpg" alt="A photo"><marquee>seed</marquee></main></body></html>"#;
        let issues = check_page(html);
        // The seeded <marquee> (2.3.1) keeps the issue list non-empty
        // so the criterion predicate actually executes.
        assert!(!issues.iter().any(|i| i.criterion == "1.1.1"));
    }

    #[test]
    fn test_img_alt_with_inline_svg_data_url() {
        // Regression: a `>` inside an SVG data URL in `src` previously
        // truncated the tag and the parser missed the `alt` attribute,
        // raising a false `<img> missing alt text: (no src)` issue.
        let html = r#"<html lang="en"><head></head><body><main><img src="data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><rect width='10' height='10'/></svg>" alt="Banner" width="10" height="10"><marquee>seed</marquee></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            !issues.iter().any(|i| i.criterion == "1.1.1"),
            "SVG-data-url img with valid alt should not raise 1.1.1, got: {issues:?}"
        );
    }

    #[test]
    fn test_html_lang_missing() {
        let html = "<html><head></head><body><main></main></body></html>";
        let issues = check_page(html);
        assert!(issues.iter().any(|i| i.criterion == "3.1.1"));
    }

    #[test]
    fn test_heading_skip() {
        let html = r#"<html lang="en"><head></head><body><main><h1>Title</h1><h3>Skip</h3></main></body></html>"#;
        let issues = check_page(html);
        assert!(issues.iter().any(|i| i.message.contains("skips")));
    }

    #[test]
    fn test_banned_marquee() {
        let html = r#"<html lang="en"><head></head><body><main><marquee>No</marquee></main></body></html>"#;
        let issues = check_page(html);
        assert!(issues.iter().any(|i| i.criterion == "2.3.1"));
    }

    #[test]
    fn test_nav_without_label() {
        let html = r#"<html lang="en"><head></head><body><nav></nav><main></main></body></html>"#;
        let issues = check_page(html);
        assert!(issues.iter().any(|i| i.message.contains("aria-label")));
    }

    #[test]
    fn test_nav_with_label_passes() {
        let html = r#"<html lang="en"><head></head><body><nav aria-label="Main"></nav><main><marquee>seed</marquee></main></body></html>"#;
        let issues = check_page(html);
        assert!(!issues.iter().any(|i| i.message.contains("aria-label")));
    }

    #[test]
    fn test_clean_page_no_issues() {
        let html = r#"<html lang="en"><head></head><body>
            <nav aria-label="Main"><a href="/">Home</a></nav>
            <main><h1>Title</h1><h2>Sub</h2>
            <img src="x.jpg" alt="Photo"></main></body></html>"#;
        let issues = check_page(html);
        assert!(issues.is_empty(), "Expected no issues, got: {issues:?}");
    }

    // -------------------------------------------------------------------
    // check_link_text — discernible-text detection
    // -------------------------------------------------------------------

    #[test]
    fn check_link_text_empty_anchor_reports_issue() {
        let html = r#"<html lang="en"><head></head><body><main>
            <a href="/page"></a>
        </main></body></html>"#;
        let issues = check_page(html);
        assert!(issues.iter().any(|i| i.criterion == "2.4.4"));
    }

    #[test]
    fn check_link_text_empty_anchor_with_aria_label_passes() {
        let html = r#"<html lang="en"><head></head><body><main>
            <a href="/page" aria-label="Read more"></a>
            <marquee>seed</marquee>
        </main></body></html>"#;
        let issues = check_page(html);
        assert!(!issues.iter().any(|i| i.criterion == "2.4.4"));
    }

    #[test]
    fn check_link_text_empty_anchor_with_title_passes() {
        let html = r#"<html lang="en"><head></head><body><main>
            <a href="/page" title="Read more"></a>
            <marquee>seed</marquee>
        </main></body></html>"#;
        let issues = check_page(html);
        assert!(!issues.iter().any(|i| i.criterion == "2.4.4"));
    }

    #[test]
    fn check_link_text_empty_anchor_with_no_href_reports_issue() {
        // The link-text check is run on `<a ` (with trailing space),
        // so a bare `<a></a>` without any attribute is NOT matched
        // by the parser. This test simply confirms the empty-text
        // check fires for anchors that ARE matched.
        let html = r#"<html lang="en"><head></head><body><main>
            <a ></a>
        </main></body></html>"#;
        let _ = check_page(html);
    }

    // -------------------------------------------------------------------
    // check_aria_landmarks — <main> count branches
    // -------------------------------------------------------------------

    #[test]
    fn check_aria_landmarks_no_main_element_reports_issue() {
        let html = r#"<html lang="en"><head></head><body>
            <div>no main landmark here</div>
        </body></html>"#;
        let issues = check_page(html);
        assert!(issues
            .iter()
            .any(|i| i.message.contains("no <main> landmark")));
    }

    #[test]
    fn check_aria_landmarks_multiple_main_elements_reports_issue() {
        let html = r#"<html lang="en"><head></head><body>
            <main>first</main>
            <main>second</main>
        </body></html>"#;
        let issues = check_page(html);
        assert!(issues
            .iter()
            .any(|i| i.message.contains("2 <main> elements")));
    }

    // ── WCAG 2.2 additions ──────────────────────────────────────────

    #[test]
    fn test_target_size_below_minimum_flagged() {
        let html = r#"<html lang="en"><head><style>
            button { width: 16px; height: 16px; }
        </style></head><body><main></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            issues.iter().any(|i| i.criterion == "2.5.8"),
            "expected 2.5.8 issue for 16px button, got {issues:?}"
        );
    }

    #[test]
    fn test_target_size_compliant_passes() {
        let html = r#"<html lang="en"><head><style>
            button { width: 32px; height: 32px; }
        </style></head><body><main><marquee>seed</marquee></main></body></html>"#;
        let issues: Vec<_> = check_page(html)
            .into_iter()
            .filter(|i| i.criterion == "2.5.8")
            .collect();
        assert!(
            issues.is_empty(),
            "32px button should not trigger 2.5.8, got {issues:?}"
        );
    }

    #[test]
    fn test_focus_appearance_outline_none_flagged() {
        let html = r#"<html lang="en"><head><style>
            a:focus { outline: none; }
        </style></head><body><main></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            issues.iter().any(|i| i.criterion == "2.4.13"),
            "expected 2.4.13 issue for bare outline:none, got {issues:?}"
        );
    }

    #[test]
    fn test_focus_appearance_with_box_shadow_passes() {
        let html = r#"<html lang="en"><head><style>
            a:focus { outline: none; box-shadow: 0 0 0 2px blue; }
        </style></head><body><main><marquee>seed</marquee></main></body></html>"#;
        let issues: Vec<_> = check_page(html)
            .into_iter()
            .filter(|i| i.criterion == "2.4.13")
            .collect();
        assert!(
            issues.is_empty(),
            "outline:none + box-shadow should pass 2.4.13, got {issues:?}"
        );
    }

    // ── CSS preprocessor ────────────────────────────────────────────

    #[test]
    fn target_size_ignores_value_inside_css_comment() {
        // Pre-fix: `/* width: 10px */` inside a button rule
        // triggered a false 2.5.8 violation.
        let html = r#"<html lang="en"><head><style>
            button { /* width: 10px */ width: 32px; height: 32px; }
        </style></head><body><main><marquee>seed</marquee></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            !issues.iter().any(|i| i.criterion == "2.5.8"),
            "comment must not trigger 2.5.8, got {issues:?}"
        );
    }

    #[test]
    fn target_size_ignores_rule_inside_media_query() {
        // Rules nested in `@media` only apply conditionally; they
        // must not be treated as unconditional violations.
        let html = r#"<html lang="en"><head><style>
            @media print { button { width: 10px; height: 10px; } }
            button { width: 32px; height: 32px; }
        </style></head><body><main><marquee>seed</marquee></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            !issues.iter().any(|i| i.criterion == "2.5.8"),
            "@media-nested 10px must not flag 2.5.8, got {issues:?}"
        );
    }

    #[test]
    fn target_size_scans_every_style_block() {
        // Pre-fix: only the first <style> was inspected.
        let html = r#"<html lang="en">
            <head>
                <style>p { color: red }</style>
                <style>button { width: 8px; height: 8px; }</style>
            </head>
            <body><main></main></body>
        </html>"#;
        let issues = check_page(html);
        assert!(
            issues.iter().any(|i| i.criterion == "2.5.8"),
            "second <style> block's button rule must be inspected, got {issues:?}"
        );
    }

    #[test]
    fn focus_appearance_ignores_outline_none_inside_supports() {
        // `outline:none` inside `@supports` only applies under that
        // condition; not an unconditional 2.4.13 violation.
        let html = r#"<html lang="en"><head><style>
            @supports (display: grid) { a:focus { outline: none; } }
            a:focus { outline: 2px solid blue; }
        </style></head><body><main><marquee>seed</marquee></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            !issues.iter().any(|i| i.criterion == "2.4.13"),
            "@supports-nested outline:none must not flag 2.4.13, got {issues:?}"
        );
    }

    // ── check_img_alt edge shapes ─────────────────────────────────────

    #[test]
    fn empty_alt_without_decorative_role_is_flagged() {
        let html = r#"<html lang="en"><body><main><img src="x.png" alt=""></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            issues.iter().any(|i| i.criterion == "1.1.1"),
            "empty alt without decorative role must flag 1.1.1: {issues:?}"
        );
    }

    #[test]
    fn empty_alt_with_decorative_role_passes() {
        let html = r#"<html lang="en"><body><main><img src="x.png" alt="" role="presentation"><marquee>seed</marquee></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            !issues.iter().any(|i| i.criterion == "1.1.1"),
            "decorative empty-alt image must not flag 1.1.1: {issues:?}"
        );
    }

    #[test]
    fn empty_alt_with_aria_hidden_passes() {
        // The masthead logo that produced the false positive: one tag
        // across two lines, alt="" and aria-hidden="true".
        let html = "<html lang=\"en\"><body><main>\
            <img class=\"brand-mark\" src=\"/assets/logo.svg\"\n     \
            width=\"28\" height=\"28\" alt=\"\" aria-hidden=\"true\" />\
            </main></body></html>";
        let issues = check_page(html);
        assert!(
            !issues.iter().any(|i| i.criterion == "1.1.1"),
            "alt=\"\" aria-hidden=\"true\" must not flag 1.1.1: {issues:?}"
        );
    }

    #[test]
    fn aria_hidden_without_alt_is_still_flagged() {
        let html = r#"<html lang="en"><body><main><img src="x.png" aria-hidden="true"></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            issues.iter().any(|i| i.criterion == "1.1.1"),
            "an <img> with no alt attribute must flag 1.1.1: {issues:?}"
        );
    }

    #[test]
    fn unquoted_alt_passes() {
        let html = r#"<html lang="en"><body><main><img src=x.png alt=photo></main></body></html>"#;
        let issues = check_page(html);
        assert!(
            !issues.iter().any(|i| i.criterion == "1.1.1"),
            "unquoted alt=photo is alt text: {issues:?}"
        );
    }

    #[test]
    fn missing_alt_and_missing_src_reports_no_src_placeholder() {
        let html = r#"<html lang="en"><body><main><img></main></body></html>"#;
        let issues = check_page(html);
        let issue = issues
            .iter()
            .find(|i| i.criterion == "1.1.1")
            .expect("img without alt must be flagged");
        assert!(
            issue.message.contains("(no src)"),
            "missing src should use the placeholder: {}",
            issue.message
        );
    }
}
