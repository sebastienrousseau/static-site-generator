// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Metadata + Open Graph + Twitter card gate.
//!
//! Per page:
//! - `<title>` present + non-empty.
//! - `<meta name="description">` present + non-empty.
//! - `og:title`, `og:type`, `og:image`.
//! - `twitter:card`.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use super::hreflang_attr;

const NAME: &str = "metadata";

/// Metadata + Open Graph + Twitter card gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::metadata::MetadataGate;
/// assert_eq!(MetadataGate.name(), "metadata");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct MetadataGate;

impl AuditGate for MetadataGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Asserts every page declares <title>, <meta name=description>, \
         and the Open Graph trio og:title, og:type, og:image plus \
         twitter:card. These power link-preview cards on Slack, \
         Twitter, LinkedIn, iMessage, and search-engine SERP previews."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);

            check_title(&html, &rel, &mut findings);
            if !has_named_meta(&html, "description") {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Error,
                        "<meta name=\"description\"> missing or empty",
                    )
                    .with_code("META-DESCRIPTION")
                    .with_path(rel.clone()),
                );
            }
            for required in ["og:title", "og:type", "og:image"] {
                if !has_property_meta(&html, required) {
                    findings.push(
                        Finding::new(
                            NAME,
                            Severity::Warn,
                            format!("<meta property=\"{required}\"> missing"),
                        )
                        .with_code(format!(
                            "OG-{}",
                            required
                                .split(':')
                                .next_back()
                                .unwrap_or("")
                                .to_uppercase()
                        ))
                        .with_path(rel.clone()),
                    );
                }
            }
            if !has_named_meta(&html, "twitter:card") {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Warn,
                        "<meta name=\"twitter:card\"> missing",
                    )
                    .with_code("TWITTER-CARD")
                    .with_path(rel.clone()),
                );
            }
        }
        findings
    }
}

fn check_title(html: &str, rel: &str, findings: &mut Vec<Finding>) {
    let lower = html.to_lowercase();
    let Some(start) = lower.find("<title") else {
        findings.push(
            Finding::new(NAME, Severity::Error, "Missing <title>")
                .with_code("META-TITLE")
                .with_path(rel.to_string()),
        );
        return;
    };
    let end = lower[start..].find("</title>");
    let Some(end) = end else {
        findings.push(
            Finding::new(NAME, Severity::Error, "<title> not closed")
                .with_code("META-TITLE")
                .with_path(rel.to_string()),
        );
        return;
    };
    let block = &lower[start..start + end];
    let gt = block.find('>').unwrap_or(0);
    let text = block[gt + 1..].trim();
    if text.is_empty() {
        findings.push(
            Finding::new(NAME, Severity::Error, "<title> is empty")
                .with_code("META-TITLE")
                .with_path(rel.to_string()),
        );
    }
}

fn has_named_meta(html: &str, name: &str) -> bool {
    has_meta_with(html, "name", name)
}

fn has_property_meta(html: &str, name: &str) -> bool {
    has_meta_with(html, "property", name)
}

fn has_meta_with(html: &str, key: &str, value: &str) -> bool {
    let lower = html.to_lowercase();
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<meta") {
        let abs = cursor + rel;
        let end = lower[abs..].find('>').map_or(lower.len(), |e| abs + e + 1);
        let tag = &html[abs..end];
        cursor = end;
        let Some(actual_key) = hreflang_attr(tag, key) else {
            continue;
        };
        if !actual_key.eq_ignore_ascii_case(value) {
            continue;
        }
        let Some(content) = hreflang_attr(tag, "content") else {
            continue;
        };
        if !content.trim().is_empty() {
            return true;
        }
    }
    false
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
    fn complete_metadata_is_clean() {
        let html = r#"<html><head>
            <title>x</title>
            <meta name="description" content="a">
            <meta property="og:title" content="x">
            <meta property="og:type" content="website">
            <meta property="og:image" content="/a.png">
            <meta name="twitter:card" content="summary">
        </head><body></body></html>"#;
        let f = MetadataGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn missing_og_trio_flagged() {
        let html = r#"<html><head><title>x</title></head><body></body></html>"#;
        let f = MetadataGate.run(&site(html), &AuditOptions::default());
        let codes: Vec<_> =
            f.iter().filter_map(|x| x.code.as_deref()).collect();
        assert!(codes.contains(&"OG-TITLE"));
        assert!(codes.contains(&"OG-TYPE"));
        assert!(codes.contains(&"OG-IMAGE"));
        assert!(codes.contains(&"META-DESCRIPTION"));
    }

    #[test]
    fn missing_title_flagged() {
        let html = r#"<html><head></head><body></body></html>"#;
        let f = MetadataGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("META-TITLE")));
    }

    #[test]
    fn unclosed_title_flagged() {
        let html = r#"<html><head><title>oops</head><body></body></html>"#;
        let f = MetadataGate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("META-TITLE")),
            "got {f:?}"
        );
    }

    #[test]
    fn empty_title_flagged() {
        let html =
            r#"<html><head><title>   </title></head><body></body></html>"#;
        let f = MetadataGate.run(&site(html), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("META-TITLE")));
    }

    #[test]
    fn missing_twitter_card_warns() {
        let html = r#"<html><head>
            <title>x</title>
            <meta name="description" content="d">
            <meta property="og:title" content="x">
            <meta property="og:type" content="website">
            <meta property="og:image" content="/a.png">
        </head><body></body></html>"#;
        let f = MetadataGate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("TWITTER-CARD")),
            "got {f:?}"
        );
        assert!(f.iter().any(|x| matches!(x.severity, Severity::Warn)));
    }

    #[test]
    fn empty_description_content_flagged() {
        let html = r#"<html><head>
            <title>x</title>
            <meta name="description" content="">
            <meta property="og:title" content="x">
            <meta property="og:type" content="website">
            <meta property="og:image" content="/a.png">
            <meta name="twitter:card" content="summary">
        </head><body></body></html>"#;
        let f = MetadataGate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter()
                .any(|x| x.code.as_deref() == Some("META-DESCRIPTION")),
            "got {f:?}"
        );
    }

    #[test]
    fn meta_missing_content_attr_flagged() {
        let html = r#"<html><head>
            <title>x</title>
            <meta name="description">
            <meta property="og:title" content="x">
            <meta property="og:type" content="website">
            <meta property="og:image" content="/a.png">
            <meta name="twitter:card" content="summary">
        </head><body></body></html>"#;
        let f = MetadataGate.run(&site(html), &AuditOptions::default());
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("META-DESCRIPTION")));
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = MetadataGate;
        assert_eq!(g.name(), "metadata");
        assert!(g.explain().to_lowercase().contains("open graph"));
        let _copy: MetadataGate = g;
        let _clone = g;
        let dbg = format!("{g:?}");
        assert!(dbg.contains("MetadataGate"));
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
        let _ = MetadataGate.run(&s, &AuditOptions::default());
        std::mem::forget(tmp);
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
        let f = MetadataGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }
}
