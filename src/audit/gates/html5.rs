// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! HTML5 structural validation gate.
//!
//! Per page:
//! - Exactly one `<h1>` element.
//! - A `<main>` landmark exists.
//! - `<title>` is present and non-empty.
//! - `<meta charset>` is present.
//! - `<!doctype html>` declaration is present.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};

const NAME: &str = "html5";

/// HTML5 structural validation gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::html5::Html5Gate;
/// assert_eq!(Html5Gate.name(), "html5");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Html5Gate;

impl AuditGate for Html5Gate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Validates HTML5 structural invariants on every page: exactly \
         one <h1>, a <main> landmark, a non-empty <title>, a \
         <meta charset>, and a <!doctype html> at the top. Structural \
         omissions are emitted as errors — runtime browsers will \
         silently coerce missing pieces but Lighthouse and a11y tools \
         demand them."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);
            let lower = html.to_lowercase();

            if !lower.trim_start().starts_with("<!doctype html") {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Error,
                        "Missing <!doctype html> at top of file",
                    )
                    .with_code("HTML5-DOCTYPE")
                    .with_path(rel.clone()),
                );
            }

            let h1_count = lower.matches("<h1").count();
            if h1_count == 0 {
                findings.push(
                    Finding::new(NAME, Severity::Error, "No <h1> element")
                        .with_code("HTML5-H1-MISSING")
                        .with_path(rel.clone()),
                );
            } else if h1_count > 1 {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Warn,
                        format!("Multiple <h1> elements ({h1_count})"),
                    )
                    .with_code("HTML5-H1-MULTIPLE")
                    .with_path(rel.clone()),
                );
            }

            if !lower.contains("<main") {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Warn,
                        "No <main> landmark element",
                    )
                    .with_code("HTML5-MAIN-MISSING")
                    .with_path(rel.clone()),
                );
            }

            if !lower.contains("<meta charset") {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Error,
                        "Missing <meta charset>",
                    )
                    .with_code("HTML5-CHARSET")
                    .with_path(rel.clone()),
                );
            }

            if let Some(start) = lower.find("<title") {
                let close = lower[start..].find("</title>").unwrap_or(0);
                if close == 0 {
                    findings.push(
                        Finding::new(
                            NAME,
                            Severity::Error,
                            "Unclosed or missing <title>",
                        )
                        .with_code("HTML5-TITLE-UNCLOSED")
                        .with_path(rel.clone()),
                    );
                } else {
                    let block = &lower[start..start + close];
                    let gt = block.find('>').unwrap_or(0);
                    let text = block[gt + 1..].trim();
                    if text.is_empty() {
                        findings.push(
                            Finding::new(
                                NAME,
                                Severity::Error,
                                "<title> is empty",
                            )
                            .with_code("HTML5-TITLE-EMPTY")
                            .with_path(rel.clone()),
                        );
                    }
                }
            } else {
                findings.push(
                    Finding::new(NAME, Severity::Error, "Missing <title>")
                        .with_code("HTML5-TITLE-MISSING")
                        .with_path(rel.clone()),
                );
            }
        }
        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(html: &str) -> Site {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("page.html");
        std::fs::write(&p, html).unwrap();
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        Site {
            root,
            html_files: vec![p],
        }
    }

    #[test]
    fn well_formed_page_has_no_findings() {
        let html = "<!doctype html><html><head><meta charset=\"utf-8\"><title>x</title></head><body><main><h1>x</h1></main></body></html>";
        let f = Html5Gate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn missing_doctype_charset_title_h1_flagged() {
        let html = "<html><head></head><body><p>x</p></body></html>";
        let f = Html5Gate.run(&site(html), &AuditOptions::default());
        let codes: Vec<_> =
            f.iter().filter_map(|x| x.code.as_deref()).collect();
        assert!(codes.contains(&"HTML5-DOCTYPE"));
        assert!(codes.contains(&"HTML5-H1-MISSING"));
        assert!(codes.contains(&"HTML5-CHARSET"));
        assert!(codes.contains(&"HTML5-TITLE-MISSING"));
    }

    #[test]
    fn multiple_h1_warns() {
        let html = "<!doctype html><html><head><meta charset=\"utf-8\"><title>x</title></head><body><main><h1>a</h1><h1>b</h1><h1>c</h1></main></body></html>";
        let f = Html5Gate.run(&site(html), &AuditOptions::default());
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("HTML5-H1-MULTIPLE")));
        assert!(f.iter().any(|x| matches!(x.severity, Severity::Warn)));
    }

    #[test]
    fn missing_main_warns() {
        let html = "<!doctype html><html><head><meta charset=\"utf-8\"><title>x</title></head><body><h1>x</h1></body></html>";
        let f = Html5Gate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter()
                .any(|x| x.code.as_deref() == Some("HTML5-MAIN-MISSING")),
            "got {f:?}"
        );
    }

    #[test]
    fn empty_title_flagged() {
        let html = "<!doctype html><html><head><meta charset=\"utf-8\"><title>   </title></head><body><main><h1>x</h1></main></body></html>";
        let f = Html5Gate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter()
                .any(|x| x.code.as_deref() == Some("HTML5-TITLE-EMPTY")),
            "got {f:?}"
        );
    }

    #[test]
    fn unclosed_title_flagged() {
        let html = "<!doctype html><html><head><meta charset=\"utf-8\"><title>oops</head><body><main><h1>x</h1></main></body></html>";
        let f = Html5Gate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter()
                .any(|x| x.code.as_deref() == Some("HTML5-TITLE-UNCLOSED")),
            "got {f:?}"
        );
    }

    #[test]
    fn doctype_case_insensitive() {
        let html = "<!DOCTYPE HTML><html><head><meta charset=\"utf-8\"><title>x</title></head><body><main><h1>x</h1></main></body></html>";
        let f = Html5Gate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = Html5Gate;
        assert_eq!(g.name(), "html5");
        assert!(g.explain().to_lowercase().contains("doctype"));
        let _copy: Html5Gate = g;
        let _clone = g;
        let dbg = format!("{g:?}");
        assert!(dbg.contains("Html5Gate"));
    }

    #[test]
    fn empty_site_returns_no_findings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        let s = Site {
            root,
            html_files: Vec::new(),
        };
        let f = Html5Gate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
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
        let _ = Html5Gate.run(&s, &AuditOptions::default());
        std::mem::forget(tmp);
    }

    #[test]
    fn leading_whitespace_before_doctype_ok() {
        let html = "   \n  <!doctype html><html><head><meta charset=\"utf-8\"><title>x</title></head><body><main><h1>x</h1></main></body></html>";
        let f = Html5Gate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }
}
