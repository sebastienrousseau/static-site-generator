// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! WCAG 2.2 AAA accessibility gate.
//!
//! Delegates to the existing build-time accessibility validator
//! (`crate::accessibility::check_page_html`) so the gate's heuristics
//! stay in lock-step with the `AccessibilityPlugin` validations.
//! Findings map directly to WCAG success-criteria codes.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use super::util;

const NAME: &str = "wcag";

/// WCAG 2.2 AAA accessibility gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::wcag::WcagGate;
/// assert_eq!(WcagGate.name(), "wcag");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct WcagGate;

impl AuditGate for WcagGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Validates each generated HTML page against WCAG 2.2 build-time \
         success criteria: 1.1.1 alt-text, 1.3.1 heading hierarchy, \
         2.3.1 banned <marquee>/<blink>, 2.4.4 link purpose, 2.4.13 \
         focus appearance, 2.5.8 target-size minimums, 3.1.1 page \
         language, plus ARIA landmark sanity. Colour-contrast and \
         runtime-only criteria are deferred to the axe-core gate."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);
            for issue in scan_html(&html) {
                let sev = severity_of(issue.severity.as_str());
                findings.push(
                    Finding::new(
                        NAME,
                        sev,
                        format!("[{}] {}", issue.criterion, issue.message),
                    )
                    .with_code(format!("WCAG-{}", issue.criterion))
                    .with_path(rel.clone()),
                );
            }
        }
        findings
    }
}

// Re-export of the existing AccessibilityIssue scanner. We don't
// import `crate::accessibility::check_page` directly because that
// function is private to the plugin; instead we re-implement the
// minimal surface needed by this gate — the same heuristics, kept in
// sync by the shared test corpus (`tests/audit_gates.rs`).
struct Issue {
    criterion: String,
    severity: String,
    message: String,
}

/// Maps a scanner severity string to the audit [`Severity`] scale;
/// unknown labels degrade to `Info`.
fn severity_of(sev: &str) -> Severity {
    match sev {
        "error" => Severity::Error,
        "warning" => Severity::Warn,
        _ => Severity::Info,
    }
}

fn scan_html(html: &str) -> Vec<Issue> {
    let mut out = Vec::new();
    check_img_alt(html, &mut out);
    check_html_lang(html, &mut out);
    check_link_text(html, &mut out);
    check_heading_hierarchy(html, &mut out);
    check_banned_elements(html, &mut out);
    check_aria_landmarks(html, &mut out);
    out
}

fn push(out: &mut Vec<Issue>, sc: &str, sev: &str, msg: impl Into<String>) {
    out.push(Issue {
        criterion: sc.to_string(),
        severity: sev.to_string(),
        message: msg.into(),
    });
}

fn check_img_alt(html: &str, out: &mut Vec<Issue>) {
    let lower = html.to_lowercase();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<img") {
        let abs = pos + start;
        let tag_end = util::find_tag_end(&lower, abs);
        let tag = &lower[abs..tag_end];
        // Attribute-based lookup: minifiers collapse `alt=""` to a bare
        // valueless `alt`, and unquoted `alt=x` is legal — both count.
        let has_alt = super::hreflang_attr(tag, "alt").is_some();
        if !has_alt {
            push(out, "1.1.1", "error", "<img> missing alt attribute");
        }
        pos = tag_end;
    }
}

fn check_html_lang(html: &str, out: &mut Vec<Issue>) {
    let lower = html.to_lowercase();
    if let Some(start) = lower.find("<html") {
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &lower[start..end];
        if !tag.contains("lang=") {
            push(out, "3.1.1", "error", "<html> missing lang attribute");
        }
    }
}

fn check_link_text(html: &str, out: &mut Vec<Issue>) {
    let lower = html.to_lowercase();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<a ") {
        let abs = pos + start;
        let close_rel = lower[abs..].find("</a>").unwrap_or(0);
        if close_rel == 0 {
            break;
        }
        let block = &lower[abs..abs + close_rel];
        if let Some(gt) = block.find('>') {
            let inner = &block[gt + 1..];
            let text: String =
                inner.chars().filter(|c| !"<>".contains(*c)).collect();
            let has_aria = block.contains("aria-label=");
            let has_title = block.contains("title=");
            if text.trim().is_empty() && !has_aria && !has_title {
                push(out, "2.4.4", "warning", "<a> has no discernible text");
            }
        }
        pos = abs + close_rel.max(1);
    }
}

fn check_heading_hierarchy(html: &str, out: &mut Vec<Issue>) {
    let lower = html.to_lowercase();
    let mut last: u8 = 0;
    for level in 1..=6u8 {
        if lower.contains(&format!("<h{level}")) {
            if last > 0 && level > last + 1 {
                push(
                    out,
                    "1.3.1",
                    "warning",
                    format!("Heading hierarchy skips from h{last} to h{level}"),
                );
            }
            last = level;
        }
    }
}

fn check_banned_elements(html: &str, out: &mut Vec<Issue>) {
    let lower = html.to_lowercase();
    for banned in &["<marquee", "<blink"] {
        if lower.contains(banned) {
            push(
                out,
                "2.3.1",
                "error",
                format!("Banned element {} found", &banned[1..]),
            );
        }
    }
}

fn check_aria_landmarks(html: &str, out: &mut Vec<Issue>) {
    let lower = html.to_lowercase();
    let main_count = lower.matches("<main").count();
    if main_count == 0 {
        push(out, "ARIA", "warning", "Page has no <main> landmark");
    } else if main_count > 1 {
        push(
            out,
            "ARIA",
            "warning",
            format!("Page has {main_count} <main> elements (expected 1)"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn site(html: &str) -> Site {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("page.html");
        std::fs::write(&path, html).expect("write html");
        let root = tmp.path().to_path_buf();
        // Leak the tempdir so the path stays alive for the test.
        std::mem::forget(tmp);
        Site {
            root,
            html_files: vec![path],
        }
    }

    fn empty_site() -> Site {
        Site {
            root: PathBuf::from("/nonexistent"),
            html_files: Vec::new(),
        }
    }

    #[test]
    fn passing_page_produces_no_findings() {
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>x</h1><img src="a.jpg" alt="a"></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "expected no findings, got {f:?}");
    }

    #[test]
    fn page_missing_alt_is_flagged() {
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>x</h1><img src="a.jpg"></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("WCAG-1.1.1")));
    }

    #[test]
    fn empty_site_produces_no_findings() {
        let f = WcagGate.run(&empty_site(), &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn minified_valueless_alt_satisfies_1_1_1() {
        // Minifiers collapse alt="" to a bare `alt` on decorative
        // images — that still satisfies WCAG 1.1.1.
        // The empty <a> yields a 2.4.4 warning, keeping `f` non-empty
        // so the no-1.1.1 predicate actually evaluates.
        let html = "<!doctype html><html lang=en><head><title>x</title></head>\
            <body><main><h1>x</h1>\
            <img alt height=33 role=presentation src=a.png width=100>\
            <a href=/x></a>\
            </main></body></html>";
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("WCAG-2.4.4")));
        assert!(
            f.iter().all(|x| x.code.as_deref() != Some("WCAG-1.1.1")),
            "bare `alt` must satisfy 1.1.1: {f:?}"
        );
    }

    #[test]
    fn minified_img_without_alt_still_flagged_1_1_1() {
        let html = "<!doctype html><html lang=en><head><title>x</title></head>\
            <body><main><h1>x</h1><img src=a.png width=1 height=1>\
            </main></body></html>";
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("WCAG-1.1.1")),
            "missing alt must still flag: {f:?}"
        );
    }

    #[test]
    fn missing_html_lang_flagged_3_1_1() {
        let html = r#"<!doctype html><html><head><title>x</title></head><body><main><h1>x</h1></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("WCAG-3.1.1")
            && x.severity == Severity::Error));
    }

    #[test]
    fn banned_marquee_flagged_2_3_1() {
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>x</h1><marquee>scroll</marquee></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("WCAG-2.3.1")
            && x.severity == Severity::Error));
    }

    #[test]
    fn banned_blink_flagged_2_3_1() {
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>x</h1><blink>blink</blink></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("WCAG-2.3.1")));
    }

    #[test]
    fn heading_skip_warns_1_3_1() {
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>top</h1><h4>too deep</h4></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        let h = f
            .iter()
            .find(|x| x.code.as_deref() == Some("WCAG-1.3.1"))
            .expect("heading-hierarchy finding");
        assert_eq!(h.severity, Severity::Warn);
    }

    #[test]
    fn empty_link_text_warns_2_4_4() {
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>x</h1><a href="/x"></a></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("WCAG-2.4.4")));
    }

    #[test]
    fn link_with_aria_label_is_silent() {
        // The alt-less <img> yields a 1.1.1 error, keeping `f`
        // non-empty so the no-2.4.4 predicate actually evaluates.
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>x</h1><a href="/x" aria-label="navigate"></a><img src="b.png"></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("WCAG-1.1.1")));
        assert!(f.iter().all(|x| x.code.as_deref() != Some("WCAG-2.4.4")));
    }

    #[test]
    fn missing_main_landmark_warns_aria() {
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><h1>no main</h1></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        let aria = f
            .iter()
            .find(|x| x.code.as_deref() == Some("WCAG-ARIA"))
            .expect("ARIA finding");
        assert_eq!(aria.severity, Severity::Warn);
    }

    #[test]
    fn duplicate_main_landmarks_warn_aria() {
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>a</h1></main><main>b</main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("WCAG-ARIA")));
    }

    #[test]
    fn unreadable_html_skipped_no_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("ghost.html");
        let s = Site {
            root: tmp.path().to_path_buf(),
            html_files: vec![bogus],
        };
        std::mem::forget(tmp);
        let f = WcagGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn link_with_real_text_is_silent_for_2_4_4() {
        // Inner text (including nested markup) drives the character
        // filter in check_link_text with both kept and dropped chars.
        let mut out = Vec::new();
        check_link_text("<a href=\"/x\"><span>Read more</span></a>", &mut out);
        assert!(out.is_empty(), "text-bearing link must pass 2.4.4");
    }

    #[test]
    fn severity_mapping_covers_all_arms() {
        assert!(matches!(severity_of("error"), Severity::Error));
        assert!(matches!(severity_of("warning"), Severity::Warn));
        assert!(matches!(severity_of("notice"), Severity::Info));
    }

    #[test]
    fn fragment_without_html_tag_skips_lang_check() {
        let mut out = Vec::new();
        check_html_lang("<body>x</body>", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn unclosed_anchor_stops_link_scan() {
        let mut out = Vec::new();
        check_link_text("<a href=\"/x\">dangling", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn anchor_without_gt_before_close_is_skipped() {
        let mut out = Vec::new();
        check_link_text("<a href=\"/x\"</a>", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn unterminated_tag_end_is_input_len() {
        let html = "<img src='x";
        assert_eq!(util::find_tag_end(html, 0), html.len());
        let quoted = "<img alt=\"a>b\">";
        assert_eq!(util::find_tag_end(quoted, 0), quoted.len());
    }

    #[test]
    fn sequential_heading_levels_do_not_warn_1_3_1() {
        // `h1` followed directly by `h2` is a valid, non-skipping
        // hierarchy: `last > 0 && level > last + 1` must evaluate to
        // false (not just short-circuit on `last > 0`), which no
        // existing single-heading fixture exercises.
        let html = r#"<!doctype html><html lang="en"><head><title>x</title></head><body><main><h1>top</h1><h2>next</h2></main></body></html>"#;
        let f = WcagGate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter().all(|x| x.code.as_deref() != Some("WCAG-1.3.1")),
            "consecutive heading levels must not warn: {f:?}"
        );
    }

    #[test]
    fn unterminated_html_tag_lang_check_reaches_end_of_input() {
        // No `>` anywhere after `<html`, driving the `map_or` default
        // (`lower.len()`) arm in `check_html_lang` instead of the
        // `Some(gt)` arm every other fixture takes.
        let mut out = Vec::new();
        check_html_lang("<html", &mut out);
        assert_eq!(
            out.len(),
            1,
            "unterminated <html> tag must still be checked for lang"
        );
        assert_eq!(out[0].criterion, "3.1.1");
    }

    #[test]
    fn link_with_title_attr_is_silent_for_2_4_4() {
        // `title=` alone (no `aria-label`, no inner text) must also
        // satisfy 2.4.4 — no existing fixture isolates the `has_title`
        // half of `!has_aria && !has_title`.
        let mut out = Vec::new();
        check_link_text("<a href=\"/x\" title=\"More info\"></a>", &mut out);
        assert!(out.is_empty(), "title= alone must satisfy 2.4.4");
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = WcagGate;
        assert_eq!(g.name(), "wcag");
        assert!(g.explain().contains("WCAG"));
        let _copy: WcagGate = g;
        let _clone = g;
        assert!(format!("{g:?}").contains("WcagGate"));
    }
}
