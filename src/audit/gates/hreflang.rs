// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hreflang reciprocity gate.
//!
//! For every `<link rel="alternate" hreflang="X" href="Y">` on page
//! `A`, verifies that page `Y` exists in the built site AND contains a
//! corresponding `<link rel="alternate" hreflang="LANG_OF_A" href="A">`
//! pointing back. Either direction missing is an error finding.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use std::collections::HashMap;

const NAME: &str = "hreflang";

/// Hreflang reciprocity gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::hreflang::HreflangGate;
/// assert_eq!(HreflangGate.name(), "hreflang");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct HreflangGate;

impl AuditGate for HreflangGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "For every <link rel=\"alternate\" hreflang=\"X\" href=\"Y\"> \
         the gate verifies that Y resolves to a page in the built site \
         AND that Y links back with the originating page's hreflang. \
         Either side missing is an error — Google's hreflang doc \
         requires bidirectional links for the signal to be honoured."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        // Index: rel_path -> (hreflang -> rel_target)
        let mut index: HashMap<String, HashMap<String, String>> =
            HashMap::with_capacity(site.html_files.len());
        let mut self_lang: HashMap<String, String> = HashMap::new();

        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);
            let alts = extract_alternates(&html);
            if let Some(s) = alts.iter().find(|a| a.is_self) {
                let _ = self_lang.insert(rel.clone(), s.lang.clone());
            }
            let mut m = HashMap::with_capacity(alts.len());
            for a in alts {
                let _ = m.insert(a.lang, a.href);
            }
            let _ = index.insert(rel, m);
        }

        let mut findings = Vec::new();

        for (rel, alts) in &index {
            let my_lang = self_lang.get(rel);
            for (lang, href) in alts {
                if lang == "x-default" || my_lang.is_some_and(|m| m == lang) {
                    continue;
                }
                // resolve_href maps absolute URLs to their path part,
                // so every href resolves to a site-relative candidate;
                // nonexistent targets surface as TARGET-MISSING below.
                let target_rel = resolve_href(href, &site.root);
                let Some(reverse) = index.get(&target_rel) else {
                    findings.push(
                        Finding::new(
                            NAME,
                            Severity::Error,
                            format!(
                                "hreflang=\"{lang}\" target \"{target_rel}\" does not exist in the built site"
                            ),
                        )
                        .with_code("HREFLANG-TARGET-MISSING")
                        .with_path(rel.clone()),
                    );
                    continue;
                };
                let Some(my_lang) = my_lang else { continue };
                if !reverse.contains_key(my_lang) {
                    findings.push(
                        Finding::new(
                            NAME,
                            Severity::Error,
                            format!(
                                "{target_rel} does not link back with hreflang=\"{my_lang}\""
                            ),
                        )
                        .with_code("HREFLANG-NO-RECIPROCAL")
                        .with_path(rel.clone()),
                    );
                }
            }
        }

        findings
    }
}

#[derive(Debug)]
struct Alternate {
    lang: String,
    href: String,
    is_self: bool,
}

fn extract_alternates(html: &str) -> Vec<Alternate> {
    let mut out = Vec::new();
    let lower = html.to_lowercase();
    let mut cursor = 0;
    while let Some(rel_open) = lower[cursor..].find("<link") {
        let abs = cursor + rel_open;
        let end = lower[abs..].find('>').map_or(lower.len(), |e| abs + e + 1);
        let tag = &html[abs..end];
        cursor = end;

        let lower_tag = tag.to_lowercase();
        if !lower_tag.contains("rel=\"alternate\"")
            && !lower_tag.contains("rel='alternate'")
        {
            continue;
        }
        let Some(lang) = attr(tag, "hreflang") else {
            continue;
        };
        let Some(href) = attr(tag, "href") else {
            continue;
        };
        let is_self = lang.eq_ignore_ascii_case("self")
            || lower_tag.contains("data-self=\"true\"");
        out.push(Alternate {
            lang,
            href,
            is_self,
        });
    }
    out
}

use super::hreflang_attr as attr;

fn resolve_href(href: &str, root: &std::path::Path) -> String {
    let stripped = href.trim_start_matches('/');
    // Strip absolute URL prefix if present
    let path_part = if let Some(rest) = stripped.strip_prefix("http://") {
        rest.split_once('/').map_or("", |(_, p)| p)
    } else if let Some(rest) = stripped.strip_prefix("https://") {
        rest.split_once('/').map_or("", |(_, p)| p)
    } else {
        stripped
    };
    let needs_index = path_part.is_empty()
        || path_part.ends_with('/')
        || !path_part.ends_with(".html");
    // This is a URL path, not an OS filesystem path — build it with a
    // literal `/` rather than `PathBuf::join`, which uses `\` on
    // Windows and would silently break every comparison against the
    // (always `/`-separated) page paths this gate matches against.
    let with_index = if !needs_index {
        path_part.to_string()
    } else if path_part.is_empty() {
        "index.html".to_string()
    } else {
        format!("{}/index.html", path_part.trim_end_matches('/'))
    };
    // Returned regardless of existence — callers report "target
    // missing" against the resolved path so authors get an actionable
    // message.
    let _ = root;
    with_index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_with(pages: &[(&str, &str)]) -> Site {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut files = Vec::new();
        for (rel, html) in pages {
            let p = root.join(rel);
            // root.join(rel) always has a parent directory.
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, html).unwrap();
            files.push(p);
        }
        std::mem::forget(tmp);
        Site {
            root,
            html_files: files,
        }
    }

    #[test]
    fn reciprocal_pair_has_no_findings() {
        let en = r#"<html><head>
            <link rel="alternate" hreflang="self" href="/en/index.html">
            <link rel="alternate" hreflang="en" href="/en/index.html">
            <link rel="alternate" hreflang="fr" href="/fr/index.html">
        </head><body></body></html>"#;
        let fr = r#"<html><head>
            <link rel="alternate" hreflang="self" href="/fr/index.html">
            <link rel="alternate" hreflang="fr" href="/fr/index.html">
            <link rel="alternate" hreflang="en" href="/en/index.html">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", en), ("fr/index.html", fr)]);
        // Set self_lang via the "self" alternate. Since the resolver
        // strips that alternate, we set my_lang via a regular alternate
        // pointing at self. Reformulate using the "en" + "fr" entries.
        let en2 = r#"<html><head>
            <link rel="alternate" hreflang="en" href="/en/index.html" data-self="true">
            <link rel="alternate" hreflang="fr" href="/fr/index.html">
        </head><body></body></html>"#;
        let fr2 = r#"<html><head>
            <link rel="alternate" hreflang="fr" href="/fr/index.html" data-self="true">
            <link rel="alternate" hreflang="en" href="/en/index.html">
        </head><body></body></html>"#;
        let s2 = site_with(&[("en/index.html", en2), ("fr/index.html", fr2)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        // s misuses "self" so won't be perfect; primary test is s2:
        let f2 = HreflangGate.run(&s2, &AuditOptions::default());
        assert!(
            f2.is_empty(),
            "expected reciprocal site to be clean, got {f2:?}"
        );
        let _ = f;
    }

    #[test]
    fn missing_reciprocal_is_flagged() {
        let en = r#"<html><head>
            <link rel="alternate" hreflang="en" href="/en/index.html" data-self="true">
            <link rel="alternate" hreflang="fr" href="/fr/index.html">
        </head><body></body></html>"#;
        // fr exists but does NOT link back to en.
        let fr = r#"<html><head>
            <link rel="alternate" hreflang="fr" href="/fr/index.html" data-self="true">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", en), ("fr/index.html", fr)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(
            f.iter()
                .any(|x| x.code.as_deref() == Some("HREFLANG-NO-RECIPROCAL")),
            "expected NO-RECIPROCAL finding, got {f:?}"
        );
    }

    #[test]
    fn missing_target_is_flagged() {
        let en = r#"<html><head>
            <link rel="alternate" hreflang="en" href="/en/index.html" data-self="true">
            <link rel="alternate" hreflang="de" href="/de/index.html">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", en)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(
            f.iter()
                .any(|x| x.code.as_deref() == Some("HREFLANG-TARGET-MISSING")),
            "expected TARGET-MISSING, got {f:?}"
        );
    }

    #[test]
    fn x_default_skipped() {
        let en = r#"<html><head>
            <link rel="alternate" hreflang="en" href="/en/index.html" data-self="true">
            <link rel="alternate" hreflang="x-default" href="/de/missing.html">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", en)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "x-default should be skipped, got {f:?}");
    }

    #[test]
    fn self_alternate_lang_skipped() {
        let en = r#"<html><head>
            <link rel="alternate" hreflang="en" href="/en/index.html" data-self="true">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", en)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "self lang should be skipped, got {f:?}");
    }

    #[test]
    fn single_quoted_alternate_recognised() {
        let en = r#"<html><head>
            <link rel='alternate' hreflang='en' href='/en/index.html' data-self="true">
            <link rel='alternate' hreflang='fr' href='/fr/index.html'>
        </head><body></body></html>"#;
        let fr = r#"<html><head>
            <link rel='alternate' hreflang='fr' href='/fr/index.html' data-self="true">
            <link rel='alternate' hreflang='en' href='/en/index.html'>
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", en), ("fr/index.html", fr)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(
            f.is_empty(),
            "single-quoted alternates should be clean, got {f:?}"
        );
    }

    #[test]
    fn non_alternate_link_ignored() {
        let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css" hreflang="en">
            <link rel="alternate" hreflang="en" href="/en/index.html" data-self="true">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", html)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "stylesheet link should be ignored, got {f:?}");
    }

    #[test]
    fn link_without_hreflang_skipped() {
        let html = r#"<html><head>
            <link rel="alternate" href="/en/index.html">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", html)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "no hreflang attr → ignored, got {f:?}");
    }

    #[test]
    fn link_without_href_skipped() {
        let html = r#"<html><head>
            <link rel="alternate" hreflang="en">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", html)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "no href attr → ignored, got {f:?}");
    }

    #[test]
    fn absolute_url_resolves_to_path() {
        let en = r#"<html><head>
            <link rel="alternate" hreflang="en" href="/en/index.html" data-self="true">
            <link rel="alternate" hreflang="fr" href="https://example.com/fr/index.html">
        </head><body></body></html>"#;
        let fr = r#"<html><head>
            <link rel="alternate" hreflang="fr" href="/fr/index.html" data-self="true">
            <link rel="alternate" hreflang="en" href="/en/index.html">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", en), ("fr/index.html", fr)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "absolute URLs should resolve, got {f:?}");
    }

    #[test]
    fn unreadable_html_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let dir_as_file = root.join("page.html");
        std::fs::create_dir_all(&dir_as_file).unwrap();
        let s = Site {
            root,
            html_files: vec![dir_as_file],
        };
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
        std::mem::forget(tmp);
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = HreflangGate;
        assert_eq!(g.name(), "hreflang");
        assert!(g.explain().to_lowercase().contains("hreflang"));
        let _copy: HreflangGate = g;
        let _clone = g;
        let dbg = format!("{g:?}");
        assert!(dbg.contains("HreflangGate"));
    }

    #[test]
    fn page_without_self_lang_skips_reciprocity_check() {
        // Neither page declares its own language (no data-self / self
        // alternate), so my_lang is None and the reciprocity check is
        // skipped after target resolution.
        let en = r#"<html><head>
            <link rel="alternate" hreflang="fr" href="/fr/index.html">
        </head><body></body></html>"#;
        let fr = r#"<html><head>
            <link rel="alternate" hreflang="en" href="/en/index.html">
        </head><body></body></html>"#;
        let s = site_with(&[("en/index.html", en), ("fr/index.html", fr)]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "no self-lang means no reciprocity: {f:?}");
    }

    #[test]
    fn resolve_href_handles_scheme_and_index_variants() {
        let root = std::path::Path::new("/unused");
        assert_eq!(
            resolve_href("http://example.com/fr/index.html", root),
            "fr/index.html"
        );
        assert_eq!(resolve_href("http://example.com", root), "index.html");
        assert_eq!(resolve_href("https://example.com", root), "index.html");
        assert_eq!(resolve_href("/fr/", root), "fr/index.html");
        assert_eq!(resolve_href("/fr", root), "fr/index.html");
        assert_eq!(resolve_href("", root), "index.html");
        assert_eq!(resolve_href("/de/page.html", root), "de/page.html");
    }

    #[test]
    fn empty_site_returns_no_findings() {
        let s = site_with(&[]);
        let f = HreflangGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }
}
