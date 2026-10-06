// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Language consistency gate (v0.0.47 plan §2 item 1.5 / A5).
//!
//! Per page, compares every JSON-LD `inLanguage` declaration against
//! the page's `<html lang>` attribute. The comparison is BCP-47
//! subtag-aware: only the *base* (primary) language subtag is
//! compared, so `inLanguage: "en-GB"` on `<html lang="en">` is
//! consistent, while `inLanguage: "en-GB"` on `<html lang="hi">` is a
//! `LANG-MISMATCH` warning — search engines receiving two different
//! languages for one document will trust neither.
//!
//! Pages without a `<html lang>` (WCAG 3.1.1 covers that) or without
//! any JSON-LD `inLanguage` produce no findings.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use std::collections::BTreeSet;

const NAME: &str = "lang_consistency";

/// Language consistency gate: JSON-LD `inLanguage` vs `<html lang>`.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::lang_consistency::LangConsistencyGate;
/// assert_eq!(LangConsistencyGate.name(), "lang_consistency");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LangConsistencyGate;

impl AuditGate for LangConsistencyGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Compares every JSON-LD `inLanguage` value on a page against \
         the page's <html lang> attribute. Comparison is BCP-47 \
         subtag-aware: regional variants of the same base language \
         (en vs en-GB) are consistent; different base languages \
         (en-GB on <html lang=\"hi\">) raise a LANG-MISMATCH warning. \
         Pages missing <html lang> or without JSON-LD inLanguage are \
         skipped (the WCAG and jsonld gates own those checks)."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);
            let Some(page_lang) = extract_html_lang(&html) else {
                continue;
            };
            let page_base = base_lang(&page_lang);
            if page_base.is_empty() {
                continue;
            }
            // Dedup: one finding per distinct mismatching tag per page.
            let mut seen: BTreeSet<String> = BTreeSet::new();
            for in_lang in extract_in_languages(&html) {
                let in_base = base_lang(&in_lang);
                if in_base.is_empty() || in_base == page_base {
                    continue;
                }
                if !seen.insert(in_lang.clone()) {
                    continue;
                }
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Warn,
                        format!(
                            "JSON-LD inLanguage `{in_lang}` does not match \
                             <html lang=\"{page_lang}\"> (base `{in_base}` \
                             vs `{page_base}`)"
                        ),
                    )
                    .with_code("LANG-MISMATCH")
                    .with_path(rel.clone()),
                );
            }
        }
        findings
    }
}

/// Returns the `lang` attribute of the first `<html>` tag, if any.
fn extract_html_lang(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<html")?;
    let end = super::find_tag_end(html, start);
    super::hreflang_attr(&html[start..end], "lang").filter(|l| !l.is_empty())
}

/// Collects every string-valued `inLanguage` across all JSON-LD blocks
/// on the page (recursing into `@graph`, arrays, and nested objects;
/// `{"@type": "Language", "name": …}` objects contribute their name).
fn extract_in_languages(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in extract_jsonld_blocks(html) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&block) {
            collect_in_language(&value, &mut out);
        }
    }
    out
}

/// Returns the raw text of every `<script type="application/ld+json">`
/// element. Attribute matching is quoting-, case-, and order-tolerant
/// so minified pages are parsed correctly.
fn extract_jsonld_blocks(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<script") {
        let abs = cursor + rel;
        // A `<script` quoted inside an HTML comment is prose, not a block
        // (see `plugins::seo::jsonld::comment_end_covering`).
        if let Some(end) =
            crate::plugins_group::seo::jsonld::comment_end_covering(&lower, abs)
        {
            cursor = end;
            continue;
        }
        let tag_end = super::find_tag_end(html, abs);
        let is_ld = super::hreflang_attr(&html[abs..tag_end], "type")
            .is_some_and(|t| t.eq_ignore_ascii_case("application/ld+json"));
        let close = lower[tag_end..]
            .find("</script")
            .map_or(lower.len(), |e| tag_end + e);
        if is_ld && close > tag_end {
            out.push(html[tag_end..close].to_string());
        }
        cursor = close.max(tag_end);
    }
    out
}

/// Recursively collects `inLanguage` values from a JSON-LD value.
fn collect_in_language(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if key == "inLanguage" {
                    match val {
                        serde_json::Value::String(s) => out.push(s.clone()),
                        serde_json::Value::Object(obj) => {
                            if let Some(serde_json::Value::String(s)) =
                                obj.get("name")
                            {
                                out.push(s.clone());
                            }
                        }
                        _ => {}
                    }
                }
                collect_in_language(val, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_in_language(item, out);
            }
        }
        _ => {}
    }
}

/// Returns the lowercase BCP-47 primary language subtag (`en-GB` →
/// `en`, `hi` → `hi`).
fn base_lang(tag: &str) -> String {
    tag.trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn site(html: &str) -> Site {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("page.html");
        std::fs::write(&path, html).unwrap();
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        Site {
            root,
            html_files: vec![path],
        }
    }

    fn page(lang: &str, in_language: &str) -> String {
        format!(
            "<!doctype html><html lang={lang}><head>\
             <script type=application/ld+json>\
             {{\"@context\":\"https://schema.org\",\"@type\":\"WebPage\",\
             \"inLanguage\":\"{in_language}\"}}</script>\
             </head><body></body></html>"
        )
    }

    #[test]
    fn matching_base_languages_are_clean() {
        // en vs en-GB share base `en` — consistent.
        let f = LangConsistencyGate
            .run(&site(&page("en", "en-GB")), &AuditOptions::default());
        assert!(f.is_empty(), "regional variant must pass: {f:?}");
    }

    #[test]
    fn exact_match_is_clean() {
        let f = LangConsistencyGate
            .run(&site(&page("en-GB", "en-GB")), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn differing_base_languages_flag_lang_mismatch() {
        // inLanguage en-GB on <html lang="hi"> → base en vs hi.
        let f = LangConsistencyGate
            .run(&site(&page("hi", "en-GB")), &AuditOptions::default());
        let m: Vec<_> = f
            .iter()
            .filter(|x| x.code.as_deref() == Some("LANG-MISMATCH"))
            .collect();
        assert_eq!(m.len(), 1, "expected one mismatch: {f:?}");
        assert_eq!(m[0].severity, Severity::Warn);
        assert!(m[0].message.contains("en-GB"));
        assert!(m[0].message.contains("hi"));
    }

    #[test]
    fn missing_html_lang_is_skipped() {
        let html = "<!doctype html><html><head>\
             <script type=\"application/ld+json\">\
             {\"@type\":\"WebPage\",\"inLanguage\":\"en\"}</script>\
             </head><body></body></html>";
        let f = LangConsistencyGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "no <html lang> is WCAG's job: {f:?}");
    }

    #[test]
    fn no_jsonld_in_language_is_clean() {
        let html = "<!doctype html><html lang=\"en\"><head>\
             <script type=\"application/ld+json\">\
             {\"@type\":\"WebPage\",\"name\":\"x\"}</script>\
             </head><body></body></html>";
        let f = LangConsistencyGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn nested_graph_in_language_is_found() {
        let html = "<!doctype html><html lang=\"hi\"><head>\
             <script type=\"application/ld+json\">\
             {\"@graph\":[{\"@type\":\"Article\",\"inLanguage\":\"en\"}]}\
             </script></head><body></body></html>";
        let f = LangConsistencyGate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("LANG-MISMATCH")),
            "must recurse into @graph: {f:?}"
        );
    }

    #[test]
    fn language_object_form_contributes_name() {
        let html = "<!doctype html><html lang=\"hi\"><head>\
             <script type=\"application/ld+json\">\
             {\"@type\":\"WebPage\",\"inLanguage\":\
             {\"@type\":\"Language\",\"name\":\"en\"}}</script>\
             </head><body></body></html>";
        let f = LangConsistencyGate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("LANG-MISMATCH")),
            "Language object form must be read: {f:?}"
        );
    }

    #[test]
    fn duplicate_mismatches_dedup_per_page() {
        let html = "<!doctype html><html lang=\"hi\"><head>\
             <script type=\"application/ld+json\">\
             {\"@type\":\"WebPage\",\"inLanguage\":\"en\"}</script>\
             <script type=\"application/ld+json\">\
             {\"@type\":\"Article\",\"inLanguage\":\"en\"}</script>\
             </head><body></body></html>";
        let f = LangConsistencyGate.run(&site(html), &AuditOptions::default());
        assert_eq!(f.len(), 1, "same tag reported once per page: {f:?}");
    }

    #[test]
    fn unparseable_jsonld_is_ignored() {
        let html = "<!doctype html><html lang=\"en\"><head>\
             <script type=\"application/ld+json\">{ not json </script>\
             </head><body></body></html>";
        let f = LangConsistencyGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "jsonld gate owns parse errors: {f:?}");
    }

    #[test]
    fn underscore_locale_form_is_tolerated() {
        // og:locale style `en_GB` sometimes leaks into inLanguage.
        let f = LangConsistencyGate
            .run(&site(&page("en", "en_GB")), &AuditOptions::default());
        assert!(f.is_empty(), "underscore variant shares base en: {f:?}");
    }

    #[test]
    fn empty_site_produces_no_findings() {
        let s = Site {
            root: PathBuf::from("/nonexistent"),
            html_files: Vec::new(),
        };
        let f = LangConsistencyGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn base_lang_normalises_case_and_subtags() {
        assert_eq!(base_lang("EN-gb"), "en");
        assert_eq!(base_lang("hi"), "hi");
        assert_eq!(base_lang(" fr-CA "), "fr");
        assert_eq!(base_lang(""), "");
    }

    #[test]
    fn html_lang_with_empty_base_subtag_is_skipped() {
        // `lang="-GB"` yields an empty primary subtag; the page is
        // skipped rather than compared against a meaningless base.
        let f = LangConsistencyGate
            .run(&site(&page("\"-GB\"", "en")), &AuditOptions::default());
        assert!(f.is_empty(), "empty base subtag must skip: {f:?}");
    }

    #[test]
    fn non_string_in_language_value_is_ignored() {
        let html = "<!doctype html><html lang=\"hi\"><head>\
             <script type=\"application/ld+json\">\
             {\"@type\":\"WebPage\",\"inLanguage\":42}</script>\
             </head><body></body></html>";
        let f = LangConsistencyGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "numeric inLanguage is ignored: {f:?}");
    }

    #[test]
    fn unreadable_html_file_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("ghost.html");
        let s = Site {
            root: tmp.path().to_path_buf(),
            html_files: vec![bogus],
        };
        std::mem::forget(tmp);
        let f = LangConsistencyGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn fragment_without_html_tag_yields_no_lang() {
        assert_eq!(extract_html_lang("<body>no html tag</body>"), None);
    }

    #[test]
    fn non_jsonld_script_blocks_are_ignored() {
        let blocks = extract_jsonld_blocks(
            "<script>var x = 1;</script>\
             <script type=\"application/ld+json\">{}</script>",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], "{}");
    }

    #[test]
    fn jsonld_tag_quoted_in_a_comment_is_skipped() {
        let html = r#"<html lang="fr"><head>
<!-- Inline <script type="application/ld+json"> blocks are hashed -->
<script src="/a.js"></script>
<script type="application/ld+json">{"inLanguage":"fr-FR"}</script>
</head></html>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("fr-FR"));
    }

    #[test]
    fn language_object_without_name_is_ignored() {
        let html = "<!doctype html><html lang=\"hi\"><head>\
             <script type=\"application/ld+json\">\
             {\"@type\":\"WebPage\",\"inLanguage\":\
             {\"@type\":\"Language\"}}</script>\
             </head><body></body></html>";
        let f = LangConsistencyGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "nameless Language object is ignored: {f:?}");
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = LangConsistencyGate;
        assert_eq!(g.name(), "lang_consistency");
        assert!(g.explain().contains("inLanguage"));
        let _copy: LangConsistencyGate = g;
        let _clone = g;
        assert!(format!("{g:?}").contains("LangConsistencyGate"));
    }
}
