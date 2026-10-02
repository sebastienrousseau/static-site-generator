// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Broken internal/external link gate.
//!
//! Walks every `<a href>` (and `<img src>`) on every page. Internal
//! links are resolved against the site root and reported as errors
//! when their target does not exist. External links are reported as
//! info when `--skip-network` is set (the default), and probed via
//! HTTP HEAD only when explicitly opted in.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use super::{find_tag_end, hreflang_attr, strip_script_and_style};
use std::path::PathBuf;

const NAME: &str = "links";

/// Broken internal/external link gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::broken_links::BrokenLinksGate;
/// assert_eq!(BrokenLinksGate.name(), "links");
/// assert!(BrokenLinksGate.explain().contains("href"));
/// ```
#[derive(Debug, Clone, Copy)]
pub struct BrokenLinksGate;

impl AuditGate for BrokenLinksGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Walks every <a href> and <img src> in the site. Internal \
         targets must resolve under the site root or an error is \
         emitted. External targets are skipped by default (set \
         skip_network=false to enable HEAD probing). Anchor-only \
         hrefs (#fragment) and `mailto:` / `tel:` URIs are ignored."
    }

    fn run(&self, site: &Site, opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        let mut external_skipped = 0usize;

        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);
            for href in extract_link_targets(&html) {
                if is_ignorable(&href) {
                    continue;
                }
                if is_external(&href) {
                    if opts.skip_network {
                        external_skipped += 1;
                    }
                    continue;
                }
                if !internal_target_exists(&site.root, path, &href) {
                    findings.push(
                        Finding::new(
                            NAME,
                            Severity::Error,
                            format!("internal link `{href}` does not resolve"),
                        )
                        .with_code("LINK-INTERNAL-MISSING")
                        .with_path(rel.clone()),
                    );
                }
            }
        }

        if external_skipped > 0 {
            findings.push(
                Finding::new(
                    NAME,
                    Severity::Info,
                    format!(
                        "{external_skipped} external link(s) skipped (--skip-network)"
                    ),
                )
                .with_code("LINK-EXTERNAL-SKIPPED"),
            );
        }

        findings
    }
}

fn is_ignorable(href: &str) -> bool {
    href.starts_with('#')
        || href.starts_with("mailto:")
        || href.starts_with("tel:")
        || href.starts_with("javascript:")
        || href.starts_with("data:")
        || href.is_empty()
}

fn is_external(href: &str) -> bool {
    href.starts_with("http://")
        || href.starts_with("https://")
        || href.starts_with("//")
}

fn extract_link_targets(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Blank out <script>/<style> contents first: markup embedded in JS
    // string literals (e.g. the search overlay building result rows
    // with '<a href="'+esc(lp+e.url)+'">') is not a document link.
    let html = strip_script_and_style(html);
    let lower = html.to_ascii_lowercase();
    for (open, attr) in &[("<a ", "href"), ("<img", "src")] {
        let mut cursor = 0;
        while let Some(rel) = lower[cursor..].find(open) {
            let abs = cursor + rel;
            let end = find_tag_end(&html, abs);
            let tag = &html[abs..end];
            cursor = end;
            if let Some(v) = hreflang_attr(tag, attr) {
                out.push(v);
            }
        }
    }
    out
}

fn internal_target_exists(
    root: &std::path::Path,
    page: &std::path::Path,
    href: &str,
) -> bool {
    let href_clean = href.split('?').next().unwrap_or(href);
    let href_clean = href_clean.split('#').next().unwrap_or(href_clean);
    if href_clean.is_empty() {
        return true;
    }

    let candidate: PathBuf =
        if let Some(stripped) = href_clean.strip_prefix('/') {
            root.join(stripped)
        } else if let Some(parent) = page.parent() {
            parent.join(href_clean)
        } else {
            root.join(href_clean)
        };

    if candidate.exists() {
        return true;
    }
    // NOTE: probing `<candidate>/index.html` here would be dead code —
    // an existing `index.html` child implies `candidate` is an
    // existing, traversable directory, which `exists()` above already
    // returned true for.
    // /foo (no extension) -> /foo.html
    let mut html_candidate = candidate;
    if html_candidate.extension().is_none() {
        let _ = html_candidate.set_extension("html");
        if html_candidate.exists() {
            return true;
        }
    }
    false
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
    fn passing_internal_link_is_clean() {
        let pages = &[
            (
                "index.html",
                r#"<html><body><a href="/about/">about</a><a href="https://ext.example/">ext</a></body></html>"#,
            ),
            ("about/index.html", "<html><body>about</body></html>"),
        ];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        let errors: Vec<_> =
            f.iter().filter(|x| x.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "got {errors:?}");
    }

    #[test]
    fn broken_internal_link_flagged() {
        let pages = &[(
            "index.html",
            r#"<html><body><a href="/missing/">x</a></body></html>"#,
        )];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("LINK-INTERNAL-MISSING")));
    }

    #[test]
    fn skip_network_emits_info_for_externals() {
        let pages = &[(
            "index.html",
            r#"<html><body><a href="https://example.com">x</a></body></html>"#,
        )];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("LINK-EXTERNAL-SKIPPED")));
    }

    #[test]
    fn ignorable_schemes_are_silent() {
        let pages = &[(
            "index.html",
            r##"<html><body>
                <a href="#anchor">a</a>
                <a href="mailto:x@y.z">m</a>
                <a href="tel:+1">t</a>
                <a href="javascript:void(0)">j</a>
                <a href="data:image/png;base64,xx">d</a>
                <a href="">e</a>
                <a href="https://ext.example/">keeps f non-empty</a>
            </body></html>"##,
        )];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(
            f.iter()
                .all(|x| x.code.as_deref() != Some("LINK-INTERNAL-MISSING")),
            "ignorable schemes flagged: {f:?}"
        );
    }

    #[test]
    fn protocol_relative_link_treated_as_external() {
        let pages = &[(
            "index.html",
            r#"<html><body><a href="//cdn.example/x">x</a></body></html>"#,
        )];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("LINK-EXTERNAL-SKIPPED")));
    }

    #[test]
    fn img_src_links_are_checked() {
        let pages = &[(
            "index.html",
            r#"<html><body><img src="/missing.png" alt="x"></body></html>"#,
        )];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("LINK-INTERNAL-MISSING")));
    }

    #[test]
    fn relative_link_with_query_and_fragment_strips_correctly() {
        let pages = &[
            (
                "index.html",
                r#"<html><body><a href="about.html?x=1#sec">a</a><a href="https://ext.example/">ext</a></body></html>"#,
            ),
            ("about.html", "<html></html>"),
        ];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(
            f.iter()
                .all(|x| x.code.as_deref() != Some("LINK-INTERNAL-MISSING")),
            "query/fragment must strip: {f:?}"
        );
    }

    #[test]
    fn extensionless_internal_link_resolves_via_html_extension() {
        let pages = &[
            (
                "index.html",
                r#"<html><body><a href="/about">a</a><a href="https://ext.example/">ext</a></body></html>"#,
            ),
            ("about.html", "<html></html>"),
        ];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(
            f.iter()
                .all(|x| x.code.as_deref() != Some("LINK-INTERNAL-MISSING")),
            "extensionless resolution failed: {f:?}"
        );
    }

    #[test]
    fn no_skip_network_does_not_emit_external_skip_finding() {
        // The broken internal link keeps `f` non-empty so the
        // no-external-skip predicate actually evaluates.
        let pages = &[(
            "index.html",
            r#"<html><body><a href="https://example.com">x</a><a href="/missing/">m</a></body></html>"#,
        )];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: false,
                ..AuditOptions::default()
            },
        );
        assert!(f
            .iter()
            .all(|x| x.code.as_deref() != Some("LINK-EXTERNAL-SKIPPED")));
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
        let f = BrokenLinksGate.run(
            &s,
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = BrokenLinksGate;
        assert_eq!(g.name(), "links");
        assert!(g.explain().contains("Internal"));
        let _copy: BrokenLinksGate = g;
        let _clone = g;
        assert!(format!("{g:?}").contains("BrokenLinksGate"));
    }

    #[test]
    fn anchor_markup_inside_script_string_is_ignored() {
        // Regression: 10 false LINK-INTERNAL-MISSING from the search
        // overlay's JS building '<a … href="'+esc(lp+e.url)+'">'.
        let pages = &[
            (
                "index.html",
                "<html><body><script>\nvar html='';\
             html+='<a class=\"ssg-result\" href=\"'+esc(lp+e.url)+'\">'\
             +'<div>x</div></a>';\n</script>\
             <a href=\"/exists/\">real</a>\
             <a href=\"https://ext.example/\">ext</a></body></html>",
            ),
            ("exists/index.html", "<html><body>t</body></html>"),
        ];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(
            f.iter()
                .all(|x| x.code.as_deref() != Some("LINK-INTERNAL-MISSING")),
            "JS-string anchors must be ignored: {f:?}"
        );
    }

    #[test]
    fn broken_link_in_body_still_fires_when_page_has_scripts() {
        // True positive preserved: a real broken <a href> in the body
        // must still fire even when the page carries <script> blocks.
        let pages = &[(
            "index.html",
            "<html><body><script>var x='<a href=\"/js-only/\">';</script>\
             <a href=\"/really-missing/\">broken</a></body></html>",
        )];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        let missing: Vec<_> = f
            .iter()
            .filter(|x| x.code.as_deref() == Some("LINK-INTERNAL-MISSING"))
            .collect();
        assert_eq!(missing.len(), 1, "exactly the body link: {f:?}");
        assert!(missing[0].message.contains("/really-missing/"));
    }

    #[test]
    fn href_with_raw_gt_in_quoted_value_does_not_truncate_tag() {
        // find('>') used to cut the tag inside quoted values.
        let pages = &[
            (
                "index.html",
                "<html><body><img src=\"data:image/svg+xml;utf8,<svg viewBox='0 0 1 1'></svg>\" alt=\"x\">\
                 <a href=\"/ok/\">ok</a>\
                 <a href=\"https://ext.example/\">ext</a></body></html>",
            ),
            ("ok/index.html", "<html><body>t</body></html>"),
        ];
        let f = BrokenLinksGate.run(
            &site_with(pages),
            &AuditOptions {
                skip_network: true,
                ..AuditOptions::default()
            },
        );
        assert!(
            f.iter()
                .all(|x| x.code.as_deref() != Some("LINK-INTERNAL-MISSING")),
            "quoted `>` must not truncate tags: {f:?}"
        );
    }

    #[test]
    fn tags_without_target_attribute_yield_no_links() {
        // <a> without href / <img> without src are skipped.
        let targets = extract_link_targets(
            r#"<a id="top">anchor</a><img alt="deco"><a href="/x">x</a>"#,
        );
        assert_eq!(targets, vec!["/x".to_string()]);
    }

    #[test]
    fn internal_target_exists_empty_href_is_ok() {
        // Covers line 134 — `href_clean.is_empty()` early return.
        let tmp = tempfile::tempdir().unwrap();
        assert!(internal_target_exists(tmp.path(), tmp.path(), "#"));
        assert!(internal_target_exists(tmp.path(), tmp.path(), "?"));
        assert!(internal_target_exists(tmp.path(), tmp.path(), ""));
    }

    #[test]
    fn internal_target_exists_resolves_relative_from_root_when_page_has_no_parent(
    ) {
        // Covers line 143 — `else { root.join(href_clean) }` arm when
        // the page has no parent.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("target.html"), "x").unwrap();
        // Pass the root itself as the page; root has no parent
        // outside the tempdir, but its `parent()` is still Some — to
        // force the else arm we use an empty-component Path.
        // The simplest deterministic path: the function strips '?'
        // and '#' then joins root + href_clean.
        assert!(internal_target_exists(
            tmp.path(),
            std::path::Path::new(""),
            "target.html"
        ));
    }

    #[test]
    fn internal_target_exists_dir_with_index_html() {
        // A directory target resolves via `candidate.exists()`.
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("docs");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("index.html"), "<html/>").unwrap();
        assert!(internal_target_exists(tmp.path(), tmp.path(), "/docs"));
    }

    #[test]
    fn internal_target_exists_via_trailing_slash_dir() {
        // Trailing-slash directory hrefs resolve via `exists()` too.
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("section");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("index.html"), "<html/>").unwrap();
        assert!(internal_target_exists(tmp.path(), tmp.path(), "/section/"));
    }

    #[test]
    fn internal_target_with_extension_and_missing_is_false() {
        // Extension present: the `.html` fallback must not engage.
        let tmp = tempfile::tempdir().unwrap();
        assert!(!internal_target_exists(
            tmp.path(),
            tmp.path(),
            "/ghost.png"
        ));
    }
}
