// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Individual WCAG 2.2 success-criterion checks, plus the ARIA landmark
//! checks. Each `check_*` function scans a raw HTML document and appends
//! any [`AccessibilityIssue`](crate::AccessibilityIssue)s it finds to the
//! caller-supplied `Vec`.

use crate::css::{
    extract_all_style_blocks, first_px_value, parse_top_level_rules,
    preprocess_css, selector_targets_interactive,
};
use crate::html::{
    extract_attr_value, find_tag_end, img_missing_alt, strip_tags_simple,
};
use crate::types::AccessibilityIssue;

/// WCAG 1.1.1: Every <img> must have a non-empty alt attribute.
pub(crate) fn check_img_alt(html: &str, issues: &mut Vec<AccessibilityIssue>) {
    // `to_ascii_lowercase` rather than `to_lowercase`: offsets computed on the
    // lowercased copy are used to slice the ORIGINAL string, and Unicode
    // lowercasing is not length-preserving. `İ` (U+0130) lowercases to two
    // chars, shifting every subsequent byte offset and panicking on the next
    // slice that lands mid-character. Tag and attribute names are ASCII, so an
    // ASCII fold matches identically while leaving every byte offset intact.
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<img") {
        let abs = pos + start;
        let tag_end = find_tag_end(&lower, abs);
        let tag = &lower[abs..tag_end];

        if img_missing_alt(tag) {
            let src = extract_attr_value(&html[abs..tag_end], "src")
                .unwrap_or_default();
            issues.push(AccessibilityIssue {
                criterion: "1.1.1".to_string(),
                severity: "error".to_string(),
                message: format!(
                    "<img> missing alt text: {}",
                    if src.is_empty() { "(no src)" } else { &src }
                ),
            });
        }

        pos = tag_end;
    }
}

/// WCAG 3.1.1: <html> element must have a lang attribute.
pub(crate) fn check_html_lang(
    html: &str,
    issues: &mut Vec<AccessibilityIssue>,
) {
    let lower = html.to_ascii_lowercase();
    if let Some(start) = lower.find("<html") {
        let tag_end =
            lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &lower[start..tag_end];
        if !tag.contains("lang=") {
            issues.push(AccessibilityIssue {
                criterion: "3.1.1".to_string(),
                severity: "error".to_string(),
                message: "<html> missing lang attribute".to_string(),
            });
        }
    }
}

/// WCAG 2.4.4: Links must have discernible text.
pub(crate) fn check_link_text(
    html: &str,
    issues: &mut Vec<AccessibilityIssue>,
) {
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<a ") {
        let abs = pos + start;
        let close = lower[abs..].find("</a>").unwrap_or(lower.len() - abs);
        let full = &lower[abs..abs + close];

        // Get inner content (between > and </a>)
        if let Some(gt) = full.find('>') {
            let inner = &full[gt + 1..];
            let text = strip_tags_simple(inner);
            let has_aria = full.contains("aria-label=");
            let has_title = full.contains("title=");

            if text.trim().is_empty() && !has_aria && !has_title {
                let href = extract_attr_value(&html[abs..abs + close], "href")
                    .unwrap_or_default();
                issues.push(AccessibilityIssue {
                    criterion: "2.4.4".to_string(),
                    severity: "warning".to_string(),
                    message: format!(
                        "<a> has no discernible text: href={}",
                        if href.is_empty() { "(none)" } else { &href }
                    ),
                });
            }
        }

        pos = abs + close.max(1);
    }
}

/// WCAG 1.3.1: Heading levels must not skip (e.g. h1 → h3).
pub(crate) fn check_heading_hierarchy(
    html: &str,
    issues: &mut Vec<AccessibilityIssue>,
) {
    let lower = html.to_ascii_lowercase();
    let mut last_level: u8 = 0;

    for level in 1..=6u8 {
        let tag = format!("<h{level}");
        if lower.contains(&tag) {
            if last_level > 0 && level > last_level + 1 {
                issues.push(AccessibilityIssue {
                    criterion: "1.3.1".to_string(),
                    severity: "warning".to_string(),
                    message: format!(
                        "Heading hierarchy skips from h{last_level} to h{level}"
                    ),
                });
            }
            last_level = level;
        }
    }
}

/// WCAG 2.3.1: No <marquee> or <blink> elements.
pub(crate) fn check_banned_elements(
    html: &str,
    issues: &mut Vec<AccessibilityIssue>,
) {
    let lower = html.to_ascii_lowercase();
    for tag in &["<marquee", "<blink"] {
        if lower.contains(tag) {
            issues.push(AccessibilityIssue {
                criterion: "2.3.1".to_string(),
                severity: "error".to_string(),
                message: format!("Banned element {} found", &tag[1..]),
            });
        }
    }
}

/// Lowercases `html` and removes HTML comments, `<style>` blocks, and
/// `<script>` blocks, so landmark counting isn't confused by markup that
/// only appears inside those (e.g. a commented-out `<main>`).
pub(crate) fn strip_non_content_blocks(html: &str) -> String {
    let mut clean = html.to_ascii_lowercase();

    // Remove HTML comments
    while let Some(start) = clean.find("<!--") {
        if let Some(end) = clean[start..].find("-->") {
            clean.replace_range(start..start + end + 3, "");
        } else {
            break;
        }
    }

    // Remove style blocks
    while let Some(start) = clean.find("<style") {
        if let Some(end) = clean[start..].find("</style>") {
            clean.replace_range(start..start + end + 8, "");
        } else {
            break;
        }
    }

    // Remove script blocks
    while let Some(start) = clean.find("<script") {
        if let Some(end) = clean[start..].find("</script>") {
            clean.replace_range(start..start + end + 9, "");
        } else {
            break;
        }
    }

    clean
}

/// ARIA landmark checks: one <main>, nav has aria-label.
pub(crate) fn check_aria_landmarks(
    html: &str,
    issues: &mut Vec<AccessibilityIssue>,
) {
    let clean = strip_non_content_blocks(html);

    // Count <main> elements
    let main_count = clean.matches("<main").count();
    if main_count == 0 {
        issues.push(AccessibilityIssue {
            criterion: "ARIA".to_string(),
            severity: "warning".to_string(),
            message: "Page has no <main> landmark".to_string(),
        });
    } else if main_count > 1 {
        issues.push(AccessibilityIssue {
            criterion: "ARIA".to_string(),
            severity: "warning".to_string(),
            message: format!(
                "Page has {main_count} <main> elements (expected 1)"
            ),
        });
    }

    // Check <nav> elements have aria-label
    let mut pos = 0;
    while let Some(start) = clean[pos..].find("<nav") {
        let abs = pos + start;
        let tag_end = clean[abs..].find('>').map_or(clean.len(), |e| abs + e);
        let tag = &clean[abs..tag_end];
        if !tag.contains("aria-label") && !tag.contains("aria-labelledby") {
            issues.push(AccessibilityIssue {
                criterion: "ARIA".to_string(),
                severity: "warning".to_string(),
                message: "<nav> missing aria-label".to_string(),
            });
        }
        pos = tag_end;
    }
}

/// WCAG 2.2 — 2.5.8 Target Size (Minimum, AA).
///
/// Heuristic: scan every inline `<style>` block. Flag any declaration
/// that sets `width` or `height` to a value smaller than 24 px on a
/// selector that targets `button`, `a`, `input`, or `[role="button"]`.
/// We can't fully verify rendered size at build time (that's a job for
/// a runtime tool such as axe-core) but explicit sub-24 px declarations
/// are unambiguous regressions.
pub(crate) fn check_target_size(
    html: &str,
    issues: &mut Vec<AccessibilityIssue>,
) {
    for css in extract_all_style_blocks(html) {
        let cleaned = preprocess_css(&css);
        for (selector, body) in parse_top_level_rules(&cleaned) {
            if !selector_targets_interactive(&selector) {
                continue;
            }
            for prop in ["width", "height"] {
                if let Some(px) = first_px_value(&body, prop) {
                    if px > 0 && px < 24 {
                        issues.push(AccessibilityIssue {
                            criterion: "2.5.8".to_string(),
                            severity: "warning".to_string(),
                            message: format!(
                                "Target size {prop}={px}px on `{selector}` \
                                 is below the 24×24 minimum (WCAG 2.2 AA)"
                            ),
                        });
                    }
                }
            }
        }
    }
}

/// WCAG 2.2 — 2.4.13 Focus Appearance (AAA).
///
/// Detects `:focus { outline: none }` (or `outline: 0`) without a
/// compensating `outline-style`, `box-shadow`, or `border` declaration
/// in the same rule.
pub(crate) fn check_focus_appearance(
    html: &str,
    issues: &mut Vec<AccessibilityIssue>,
) {
    for css in extract_all_style_blocks(html) {
        let cleaned = preprocess_css(&css);
        for (selector, body) in parse_top_level_rules(&cleaned) {
            if !selector.contains(":focus") {
                continue;
            }
            let kills_outline = body.contains("outline:none")
                || body.contains("outline: none")
                || body.contains("outline:0")
                || body.contains("outline: 0");
            let has_replacement = body.contains("outline-style")
                || body.contains("outline-color")
                || body.contains("box-shadow")
                || body.contains("border:");

            if kills_outline && !has_replacement {
                issues.push(AccessibilityIssue {
                    criterion: "2.4.13".to_string(),
                    severity: "warning".to_string(),
                    message: "`:focus { outline: none }` without a \
                         compensating outline-style/box-shadow/border \
                         (WCAG 2.2 AAA — Focus Appearance)"
                        .to_string(),
                });
            }
        }
    }
}

/// WCAG 2.2 — 3.2.6 Consistent Help (Level A).
///
/// Build-time verification is partial — full conformance requires
/// cross-page comparison of help-mechanism placement. This check
/// emits an *info* note when a page contains no detectable help link
/// (anchor text matching `help`, `contact`, `support`, `faq`).
/// Cross-page placement consistency is left to a runtime audit and
/// human review — that's why [`crate::CriterionStatus::Manual`] is used
/// for this criterion in the compliance matrix rather than wiring this
/// helper into [`crate::check_page`] directly (it would be too noisy on
/// every clean page that simply omits a help link). Callers that want
/// cross-page analysis can run this themselves across every page's HTML
/// and look for at least one match site-wide.
pub fn check_consistent_help(html: &str, issues: &mut Vec<AccessibilityIssue>) {
    let lower = html.to_ascii_lowercase();
    let has_help_link = lower.contains(">help<")
        || lower.contains(">contact<")
        || lower.contains(">support<")
        || lower.contains(">faq<")
        || lower.contains("aria-label=\"help\"")
        || lower.contains("aria-label=\"contact\"")
        || lower.contains("aria-label=\"support\"");

    if !has_help_link {
        issues.push(AccessibilityIssue {
            criterion: "3.2.6".to_string(),
            severity: "info".to_string(),
            message: "No detectable help/contact/support link on page; \
                      verify that the site provides a consistent help \
                      mechanism across pages (WCAG 2.2 A — Consistent \
                      Help)"
                .to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_html_lang_skips_fragment_without_html_tag() {
        let mut issues = Vec::new();
        check_html_lang("<p>fragment only</p>", &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn check_link_text_tolerates_unterminated_anchor() {
        // `<a ` with no `>` before EOF — inner content can't be found,
        // so no issue is raised and the scanner terminates.
        let mut issues = Vec::new();
        check_link_text("<a href=/x", &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn strip_non_content_blocks_removes_html_comments() {
        let out = strip_non_content_blocks("<body><!-- <main> --></body>");
        assert!(!out.contains("<main>"));
        assert!(out.contains("<body>"));
    }

    #[test]
    fn strip_non_content_blocks_tolerates_unterminated_comment() {
        let out = strip_non_content_blocks("<body><!-- no close");
        assert!(out.contains("<!--"), "unterminated comment kept: {out}");
    }

    #[test]
    fn strip_non_content_blocks_tolerates_unterminated_style() {
        let out = strip_non_content_blocks("<body><style>a{}");
        assert!(out.contains("<style>"), "unterminated style kept: {out}");
    }

    #[test]
    fn strip_non_content_blocks_tolerates_unterminated_script() {
        let out = strip_non_content_blocks("<body><script>let x=1;");
        assert!(out.contains("<script>"), "unterminated script kept: {out}");
    }

    #[test]
    fn test_consistent_help_helper_detects_link() {
        // The cross-page checker isn't wired into per-page validation
        // (it would be too noisy on every clean page that omits a
        // help link), but the helper itself still works and we want
        // it covered for future cross-page use.
        let html_with = r#"<html lang="en"><body><a href="/contact">Contact</a></body></html>"#;
        let html_without =
            r#"<html lang="en"><body><p>nothing</p></body></html>"#;
        let mut buf = Vec::new();
        check_consistent_help(html_with, &mut buf);
        assert!(buf.is_empty(), "with link, no issue");
        check_consistent_help(html_without, &mut buf);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].criterion, "3.2.6");
    }
}
