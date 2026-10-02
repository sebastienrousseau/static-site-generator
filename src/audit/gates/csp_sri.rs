// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! CSP + SRI hash validation gate.
//!
//! Per page:
//! - Asserts a `Content-Security-Policy` is present (either as a
//!   `_headers` rule or a `<meta http-equiv="Content-Security-Policy">`
//!   tag).
//! - Asserts every cross-origin `<script src>` and
//!   `<link rel="stylesheet" href>` carries an `integrity` SRI hash.
//!
//! The CSP-consistency check across pages is a warning (different
//! pages with different policies are a smell but legitimate when a
//! single section needs a stricter policy).

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use std::collections::HashSet;

const NAME: &str = "csp_sri";

/// CSP + SRI hash validation gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::csp_sri::CspSriGate;
/// assert_eq!(CspSriGate.name(), "csp_sri");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct CspSriGate;

impl AuditGate for CspSriGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Asserts every page declares a Content-Security-Policy (via a \
         <meta http-equiv=\"Content-Security-Policy\"> tag or a site \
         _headers file) and that every cross-origin <script src> / \
         <link rel=\"stylesheet\"> carries a valid `integrity` SRI \
         attribute. Policy drift between pages is reported as a warn."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        let mut policies: HashSet<String> = HashSet::new();

        // Detect a site-level _headers file.
        let site_headers = site.root.join("_headers");
        let has_site_csp = site_headers.exists()
            && std::fs::read_to_string(&site_headers).is_ok_and(|s| {
                s.to_lowercase().contains("content-security-policy")
            });

        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);

            let policy = extract_meta_csp(&html);
            if !has_site_csp && policy.is_none() {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Error,
                        "Page has no Content-Security-Policy (no <meta http-equiv> and no _headers)",
                    )
                    .with_code("CSP-MISSING")
                    .with_path(rel.clone()),
                );
            }
            if let Some(p) = policy {
                let _ = policies.insert(p);
            }

            for asset in extract_remote_assets(&html) {
                if asset.integrity.is_none() {
                    findings.push(
                        Finding::new(
                            NAME,
                            Severity::Error,
                            format!(
                                "{} {} missing SRI `integrity` attribute",
                                asset.kind, asset.href
                            ),
                        )
                        .with_code("SRI-MISSING")
                        .with_path(rel.clone()),
                    );
                }
            }
        }

        if policies.len() > 1 {
            findings.push(
                Finding::new(
                    NAME,
                    Severity::Warn,
                    format!(
                        "Found {} distinct CSP policies across pages",
                        policies.len()
                    ),
                )
                .with_code("CSP-DRIFT"),
            );
        }

        findings
    }
}

fn extract_meta_csp(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let needle = "<meta";
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find(needle) {
        let abs = cursor + rel;
        let end = super::find_tag_end(html, abs);
        let tag = &html[abs..end];
        cursor = end;
        // Attribute-based match: tolerant of quoting style, case, and
        // attribute order — minifiers emit unquoted
        // `http-equiv=Content-Security-Policy`.
        let is_csp = super::hreflang_attr(tag, "http-equiv")
            .is_some_and(|v| v.eq_ignore_ascii_case("content-security-policy"));
        if is_csp {
            if let Some(c) = super::hreflang_attr(tag, "content") {
                return Some(c);
            }
        }
    }
    None
}

struct RemoteAsset {
    kind: &'static str,
    href: String,
    integrity: Option<String>,
}

fn extract_remote_assets(html: &str) -> Vec<RemoteAsset> {
    let mut out = Vec::new();
    let lower = html.to_lowercase();

    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<script") {
        let abs = cursor + rel;
        let end = super::find_tag_end(html, abs);
        let tag = &html[abs..end];
        cursor = end;
        let Some(src) = super::hreflang_attr(tag, "src") else {
            continue;
        };
        if !is_remote(&src) {
            continue;
        }
        out.push(RemoteAsset {
            kind: "<script src=>",
            href: src,
            integrity: super::hreflang_attr(tag, "integrity"),
        });
    }

    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<link") {
        let abs = cursor + rel;
        let end = super::find_tag_end(html, abs);
        let tag = &html[abs..end];
        cursor = end;
        // `rel` may be unquoted (minified) and space-separated
        // (`rel="stylesheet preload"`).
        let is_stylesheet = super::hreflang_attr(tag, "rel").is_some_and(|r| {
            r.split_ascii_whitespace()
                .any(|t| t.eq_ignore_ascii_case("stylesheet"))
        });
        if !is_stylesheet {
            continue;
        }
        let Some(href) = super::hreflang_attr(tag, "href") else {
            continue;
        };
        if !is_remote(&href) {
            continue;
        }
        out.push(RemoteAsset {
            kind: "<link rel=stylesheet>",
            href,
            integrity: super::hreflang_attr(tag, "integrity"),
        });
    }

    out
}

fn is_remote(href: &str) -> bool {
    href.starts_with("http://")
        || href.starts_with("https://")
        || href.starts_with("//")
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
    fn page_with_csp_and_sri_is_clean() {
        let html = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
            <script src="https://cdn.example/x.js" integrity="sha256-abc"></script>
            <link rel="stylesheet" href="/local.css">
        </head><body></body></html>"#;
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn page_missing_csp_and_sri_flagged() {
        let html = r#"<html><head>
            <script src="https://cdn.example/x.js"></script>
        </head><body></body></html>"#;
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("CSP-MISSING")));
        assert!(f.iter().any(|x| x.code.as_deref() == Some("SRI-MISSING")));
    }

    #[test]
    fn site_headers_csp_satisfies_requirement() {
        let html = r#"<html><head>
            <script src="https://cdn.example/x.js" integrity="sha256-z"></script>
        </head><body></body></html>"#;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(
            root.join("_headers"),
            "/*\n  Content-Security-Policy: default-src 'self'\n",
        )
        .unwrap();
        let p = root.join("index.html");
        std::fs::write(&p, html).unwrap();
        let s = Site {
            root,
            html_files: vec![p],
        };
        std::mem::forget(tmp);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn protocol_relative_stylesheet_needs_integrity() {
        let html = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
            <link rel="stylesheet" href="//cdn.example/styles.css">
        </head><body></body></html>"#;
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("SRI-MISSING")));
    }

    #[test]
    fn local_assets_dont_need_integrity() {
        let html = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
            <script src="/local.js"></script>
            <link rel="stylesheet" href="/local.css">
        </head><body></body></html>"#;
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(
            f.is_empty(),
            "local assets should not require SRI, got {f:?}"
        );
    }

    #[test]
    fn http_remote_script_needs_integrity() {
        let html = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
            <script src="http://cdn.example/x.js"></script>
        </head><body></body></html>"#;
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("SRI-MISSING")));
    }

    #[test]
    fn csp_drift_warns() {
        let a = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
        </head><body></body></html>"#;
        let b = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self' https:">
        </head><body></body></html>"#;
        let s = site_with(&[("a.html", a), ("b.html", b)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("CSP-DRIFT")),
            "expected CSP-DRIFT, got {f:?}"
        );
    }

    #[test]
    fn script_without_src_skipped() {
        let html = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
            <script>console.log('inline');</script>
        </head><body></body></html>"#;
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "inline script should not need SRI, got {f:?}");
    }

    #[test]
    fn link_non_stylesheet_skipped() {
        let html = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
            <link rel="preconnect" href="https://cdn.example">
        </head><body></body></html>"#;
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "preconnect should not need SRI, got {f:?}");
    }

    #[test]
    fn single_quoted_stylesheet_recognised() {
        let html = r#"<html><head>
            <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
            <link rel='stylesheet' href='https://cdn.example/s.css'>
        </head><body></body></html>"#;
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("SRI-MISSING")));
    }

    #[test]
    fn minified_unquoted_csp_meta_recognised() {
        // Regression: minified HTML emits unquoted attribute values in
        // original case; the gate must not report CSP-MISSING.
        // The SRI-less remote script keeps `f` non-empty so the
        // no-CSP-MISSING predicate actually evaluates.
        let html = "<html><head>\
            <meta content=\"default-src 'self'\" http-equiv=Content-Security-Policy>\
            <script src=\"https://cdn.example/x.js\"></script>\
        </head><body></body></html>";
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(
            f.iter().all(|x| x.code.as_deref() != Some("CSP-MISSING")),
            "unquoted http-equiv must count: {f:?}"
        );
    }

    #[test]
    fn other_http_equiv_meta_does_not_count_as_csp() {
        // True positive preserved: a page whose only http-equiv is
        // something else still has no CSP.
        let html = "<html><head>\
            <meta http-equiv=X-UA-Compatible content=\"IE=edge\">\
        </head><body></body></html>";
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("CSP-MISSING")),
            "non-CSP http-equiv must still flag: {f:?}"
        );
    }

    #[test]
    fn minified_unquoted_stylesheet_rel_needs_integrity() {
        let html = "<html><head>\
            <meta http-equiv=Content-Security-Policy content=\"default-src 'self'\">\
            <link href=https://cdn.example/s.css rel=stylesheet>\
        </head><body></body></html>";
        let s = site_with(&[("index.html", html)]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("SRI-MISSING")),
            "unquoted rel=stylesheet must be seen: {f:?}"
        );
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = CspSriGate;
        assert_eq!(g.name(), "csp_sri");
        assert!(g.explain().contains("Content-Security-Policy"));
        let _copy: CspSriGate = g;
        let _clone = g;
        let dbg = format!("{g:?}");
        assert!(dbg.contains("CspSriGate"));
    }

    #[test]
    fn empty_site_returns_no_findings() {
        let s = site_with(&[]);
        let f = CspSriGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn meta_csp_without_content_attr_yields_none() {
        // http-equiv matches but there is no content= value to return;
        // the scanner keeps looking and ends with None.
        let html = r#"<meta http-equiv="Content-Security-Policy">"#;
        assert_eq!(extract_meta_csp(html), None);
    }

    #[test]
    fn stylesheet_link_without_href_is_skipped() {
        let html = r#"<link rel="stylesheet"><link rel="stylesheet" href="https://cdn.example/a.css">"#;
        let assets = extract_remote_assets(html);
        assert_eq!(assets.len(), 1, "href-less link must be skipped");
        assert_eq!(assets[0].href, "https://cdn.example/a.css");
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
        let _ = CspSriGate.run(&s, &AuditOptions::default());
        std::mem::forget(tmp);
    }
}
