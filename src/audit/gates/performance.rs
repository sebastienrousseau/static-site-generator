// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Performance budget gate.
//!
//! Per page: enforces HTML weight + inline `<style>` against
//! `page_weight_budget` and the sum of inline `<script>` + first-party
//! referenced JS file sizes against `js_budget` (both configurable via
//! `[audit.budgets]`).

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use super::hreflang_attr;

const NAME: &str = "performance";

/// Performance budget gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::performance::PerformanceGate;
/// assert_eq!(PerformanceGate.name(), "performance");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct PerformanceGate;

impl AuditGate for PerformanceGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Enforces per-page weight + JS budgets. Default page weight \
         budget is 100 KiB (HTML + inline CSS); default JS budget is \
         50 KiB (inline + first-party referenced bundles). Tune via \
         the [audit.budgets] table in ssg.toml."
    }

    fn run(&self, site: &Site, opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);

            let html_weight = html.len();
            if html_weight > opts.page_weight_budget {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Warn,
                        format!(
                            "page weight {html_weight} bytes exceeds budget {} bytes",
                            opts.page_weight_budget
                        ),
                    )
                    .with_code("PERF-PAGE-OVER")
                    .with_path(rel.clone()),
                );
            }

            let js_weight = inline_js_bytes(&html)
                + referenced_js_bytes(&site.root, path, &html);
            if js_weight > opts.js_budget {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Warn,
                        format!(
                            "JS weight {js_weight} bytes exceeds budget {} bytes",
                            opts.js_budget
                        ),
                    )
                    .with_code("PERF-JS-OVER")
                    .with_path(rel.clone()),
                );
            }
        }
        findings
    }
}

fn inline_js_bytes(html: &str) -> usize {
    let lower = html.to_lowercase();
    let mut total = 0usize;
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<script") {
        let abs = cursor + rel;
        let open_end =
            lower[abs..].find('>').map_or(lower.len(), |e| abs + e + 1);
        let tag = &html[abs..open_end];
        if hreflang_attr(tag, "src").is_some() {
            cursor = open_end;
            continue;
        }
        cursor = open_end;
        let Some(close) = lower[cursor..].find("</script>") else {
            break;
        };
        total += close;
        cursor += close + "</script>".len();
    }
    total
}

fn referenced_js_bytes(
    root: &std::path::Path,
    page: &std::path::Path,
    html: &str,
) -> usize {
    let lower = html.to_lowercase();
    let mut total = 0usize;
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<script") {
        let abs = cursor + rel;
        let end = lower[abs..].find('>').map_or(lower.len(), |e| abs + e + 1);
        let tag = &html[abs..end];
        cursor = end;
        let Some(src) = hreflang_attr(tag, "src") else {
            continue;
        };
        if src.starts_with("http://")
            || src.starts_with("https://")
            || src.starts_with("//")
        {
            continue;
        }
        let candidate = if let Some(s) = src.strip_prefix('/') {
            root.join(s)
        } else if let Some(parent) = page.parent() {
            parent.join(&src)
        } else {
            root.join(&src)
        };
        if let Ok(meta) = std::fs::metadata(&candidate) {
            total += meta.len() as usize;
        }
    }
    total
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
    fn small_page_passes() {
        let html = "<html><body>tiny</body></html>";
        let f = PerformanceGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn over_budget_page_flagged() {
        let html = "<html>".to_string() + &"x".repeat(2000) + "</html>";
        let opts = AuditOptions {
            page_weight_budget: 100,
            ..AuditOptions::default()
        };
        let f = PerformanceGate.run(&site(&html), &opts);
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("PERF-PAGE-OVER")));
    }

    #[test]
    fn over_budget_inline_js_flagged() {
        let html = format!(
            "<html><body><script>{}</script></body></html>",
            "x".repeat(2000)
        );
        let opts = AuditOptions {
            js_budget: 100,
            page_weight_budget: 1_000_000,
            ..AuditOptions::default()
        };
        let f = PerformanceGate.run(&site(&html), &opts);
        assert!(f.iter().any(|x| x.code.as_deref() == Some("PERF-JS-OVER")));
    }

    #[test]
    fn external_script_src_does_not_count_toward_js_budget() {
        // Tiny page-weight budget yields PERF-PAGE-OVER, keeping `f`
        // non-empty so the no-JS-OVER predicate actually evaluates.
        let html = r#"<html><body><script src="https://cdn.example/big.js"></script></body></html>"#;
        let opts = AuditOptions {
            js_budget: 50,
            page_weight_budget: 10,
            ..AuditOptions::default()
        };
        let f = PerformanceGate.run(&site(html), &opts);
        assert!(
            f.iter().all(|x| x.code.as_deref() != Some("PERF-JS-OVER")),
            "external scripts must not count; got {f:?}"
        );
    }

    #[test]
    fn protocol_relative_script_src_is_external() {
        // Tiny page-weight budget keeps `f` non-empty (see above).
        let html = r#"<html><body><script src="//cdn.example/big.js"></script></body></html>"#;
        let opts = AuditOptions {
            js_budget: 1,
            page_weight_budget: 10,
            ..AuditOptions::default()
        };
        let f = PerformanceGate.run(&site(html), &opts);
        assert!(f.iter().all(|x| x.code.as_deref() != Some("PERF-JS-OVER")));
    }

    #[test]
    fn referenced_root_relative_js_is_summed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(root.join("big.js"), vec![0u8; 5_000]).unwrap();
        let html_path = root.join("page.html");
        std::fs::write(
            &html_path,
            r#"<html><body><script src="/big.js"></script></body></html>"#,
        )
        .unwrap();
        std::mem::forget(tmp);
        let s = Site {
            root,
            html_files: vec![html_path],
        };
        let opts = AuditOptions {
            js_budget: 100,
            page_weight_budget: 1_000_000,
            ..AuditOptions::default()
        };
        let f = PerformanceGate.run(&s, &opts);
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("PERF-JS-OVER")),
            "expected JS-OVER from referenced file; got {f:?}"
        );
    }

    #[test]
    fn referenced_relative_js_uses_page_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("local.js"), vec![0u8; 5_000]).unwrap();
        let html_path = sub.join("page.html");
        std::fs::write(
            &html_path,
            r#"<html><body><script src="local.js"></script></body></html>"#,
        )
        .unwrap();
        std::mem::forget(tmp);
        let s = Site {
            root,
            html_files: vec![html_path],
        };
        let opts = AuditOptions {
            js_budget: 100,
            page_weight_budget: 1_000_000,
            ..AuditOptions::default()
        };
        let f = PerformanceGate.run(&s, &opts);
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("PERF-JS-OVER")),
            "expected JS-OVER from page-relative ref; got {f:?}"
        );
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
        let f = PerformanceGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn inline_js_is_counted_when_no_src_attribute() {
        let html = format!(
            "<html><body><script>{}</script><script>nested</script></body></html>",
            "y".repeat(200)
        );
        let opts = AuditOptions {
            js_budget: 100,
            page_weight_budget: 1_000_000,
            ..AuditOptions::default()
        };
        let f = PerformanceGate.run(&site(&html), &opts);
        assert!(f.iter().any(|x| x.code.as_deref() == Some("PERF-JS-OVER")));
    }

    #[test]
    fn unterminated_inline_script_stops_counting() {
        // No closing </script>: the scanner bails instead of looping.
        assert_eq!(inline_js_bytes("<html><script>var x = 1;"), 0);
    }

    #[test]
    fn missing_referenced_js_file_adds_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let html = r#"<script src="ghost.js"></script>"#;
        let page = root.join("page.html");
        let got = referenced_js_bytes(&root, &page, html);
        std::mem::forget(tmp);
        assert_eq!(got, 0);
    }

    #[test]
    fn referenced_js_for_parentless_page_falls_back_to_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(root.join("app.js"), vec![0u8; 123]).unwrap();
        let html = r#"<script src="app.js"></script>"#;
        let got = referenced_js_bytes(&root, std::path::Path::new(""), html);
        std::mem::forget(tmp);
        assert_eq!(got, 123);
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = PerformanceGate;
        assert_eq!(g.name(), "performance");
        assert!(g.explain().contains("budget"));
        let _copy: PerformanceGate = g;
        let _clone = g;
        assert!(format!("{g:?}").contains("PerformanceGate"));
    }
}
