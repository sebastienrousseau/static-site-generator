// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! RSS / Atom feed structural validation gate.
//!
//! Walks the site root for `*.xml` files (typically `rss.xml`,
//! `feed.xml`, `atom.xml`), discriminates RSS 2.0 vs Atom 1.0 by the
//! root element name, and asserts the required-field set for each
//! variant.
//!
//! RSS 2.0 required fields: `<channel>` containing `<title>`,
//! `<link>`, `<description>`. At least one `<item>` is expected when
//! the feed exists.
//!
//! Atom 1.0 required fields: `<feed>` containing `<title>`, `<id>`,
//! `<updated>`. At least one `<entry>` is expected.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use crate::walk::walk_files;

const NAME: &str = "feeds";

/// RSS/Atom feed structural validation gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::feeds::FeedsGate;
/// assert_eq!(FeedsGate.name(), "feeds");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct FeedsGate;

impl AuditGate for FeedsGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Walks the site root for *.xml files, classifies each as RSS \
         2.0 or Atom 1.0 by root element, and asserts the required \
         fields per variant. RSS 2.0 demands <channel> with <title>, \
         <link>, <description>; Atom 1.0 demands <feed> with <title>, \
         <id>, <updated>. Empty feeds raise a warning."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        let xml_files = walk_files(&site.root, "xml").unwrap_or_default();
        if xml_files.is_empty() {
            return findings;
        }
        for path in &xml_files {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let rel = site.rel(path);
            let kind = classify(&text);
            match kind {
                FeedKind::Rss2 => check_rss2(&text, &rel, &mut findings),
                FeedKind::Atom1 => check_atom1(&text, &rel, &mut findings),
                FeedKind::Sitemap => {
                    // Sitemaps are validated by their own pipeline, not this gate.
                }
                FeedKind::Unknown => {
                    // Quietly ignore unknown XML — many SSGs emit
                    // OPML, GraphQL schema, etc.
                }
            }
        }
        findings
    }
}

enum FeedKind {
    Rss2,
    Atom1,
    Sitemap,
    Unknown,
}

fn classify(text: &str) -> FeedKind {
    let lower = text.to_lowercase();
    if lower.contains("<rss") {
        FeedKind::Rss2
    } else if lower.contains("<feed") && lower.contains("atom") {
        FeedKind::Atom1
    } else if lower.contains("<urlset") || lower.contains("<sitemapindex") {
        FeedKind::Sitemap
    } else {
        FeedKind::Unknown
    }
}

fn check_rss2(text: &str, rel: &str, findings: &mut Vec<Finding>) {
    let lower = text.to_lowercase();
    if !lower.contains("<channel") {
        findings.push(
            Finding::new(NAME, Severity::Error, "RSS feed missing <channel>")
                .with_code("RSS-CHANNEL")
                .with_path(rel.to_string()),
        );
        return;
    }
    for required in ["<title", "<link", "<description"] {
        if !lower.contains(required) {
            findings.push(
                Finding::new(
                    NAME,
                    Severity::Error,
                    format!("RSS <channel> missing {required}>"),
                )
                .with_code(format!(
                    "RSS-{}",
                    required.trim_start_matches('<').to_uppercase()
                ))
                .with_path(rel.to_string()),
            );
        }
    }
    if !lower.contains("<item") {
        findings.push(
            Finding::new(
                NAME,
                Severity::Warn,
                "RSS feed contains zero <item> entries",
            )
            .with_code("RSS-EMPTY")
            .with_path(rel.to_string()),
        );
    }
}

fn check_atom1(text: &str, rel: &str, findings: &mut Vec<Finding>) {
    let lower = text.to_lowercase();
    for required in ["<title", "<id", "<updated"] {
        if !lower.contains(required) {
            findings.push(
                Finding::new(
                    NAME,
                    Severity::Error,
                    format!("Atom feed missing {required}>"),
                )
                .with_code(format!(
                    "ATOM-{}",
                    required.trim_start_matches('<').to_uppercase()
                ))
                .with_path(rel.to_string()),
            );
        }
    }
    if !lower.contains("<entry") {
        findings.push(
            Finding::new(
                NAME,
                Severity::Warn,
                "Atom feed contains zero <entry> elements",
            )
            .with_code("ATOM-EMPTY")
            .with_path(rel.to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_with_files(files: &[(&str, &str)]) -> Site {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        for (name, body) in files {
            let p = root.join(name);
            // root.join(name) always has a parent directory.
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        std::mem::forget(tmp);
        Site {
            root,
            html_files: Vec::new(),
        }
    }

    #[test]
    fn valid_rss_passes() {
        let body = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>x</title><link>https://x</link><description>d</description>
  <item><title>i</title></item>
</channel></rss>"#;
        let s = site_with_files(&[("rss.xml", body)]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn rss_missing_channel_fields_flagged() {
        let body = r#"<?xml version="1.0"?><rss version="2.0"><channel></channel></rss>"#;
        let s = site_with_files(&[("rss.xml", body)]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        let codes: Vec<_> =
            f.iter().filter_map(|x| x.code.as_deref()).collect();
        assert!(codes.contains(&"RSS-TITLE"));
        assert!(codes.contains(&"RSS-LINK"));
        assert!(codes.contains(&"RSS-DESCRIPTION"));
    }

    #[test]
    fn rss_missing_channel_short_circuits() {
        let body = r#"<?xml version="1.0"?><rss version="2.0"></rss>"#;
        let s = site_with_files(&[("rss.xml", body)]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        let codes: Vec<_> =
            f.iter().filter_map(|x| x.code.as_deref()).collect();
        assert_eq!(codes, vec!["RSS-CHANNEL"]);
    }

    #[test]
    fn rss_empty_item_warns() {
        let body = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>x</title><link>https://x</link><description>d</description>
</channel></rss>"#;
        let s = site_with_files(&[("rss.xml", body)]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        assert!(
            f.iter()
                .filter_map(|x| x.code.as_deref())
                .any(|c| c == "RSS-EMPTY"),
            "got {f:?}"
        );
        assert!(
            f.iter().any(|x| matches!(x.severity, Severity::Warn)),
            "got {f:?}"
        );
    }

    #[test]
    fn valid_atom_passes() {
        let body = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>x</title><id>urn:x</id><updated>2026-01-01</updated>
  <entry><title>e</title></entry>
</feed>"#;
        let s = site_with_files(&[("atom.xml", body)]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn atom_missing_fields_flagged() {
        let body = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom"></feed>"#;
        let s = site_with_files(&[("atom.xml", body)]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        let codes: Vec<_> =
            f.iter().filter_map(|x| x.code.as_deref()).collect();
        assert!(codes.contains(&"ATOM-TITLE"));
        assert!(codes.contains(&"ATOM-ID"));
        assert!(codes.contains(&"ATOM-UPDATED"));
        assert!(codes.contains(&"ATOM-EMPTY"));
    }

    #[test]
    fn sitemap_xml_ignored() {
        let body = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://x</loc></url>
</urlset>"#;
        let s = site_with_files(&[("sitemap.xml", body)]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn unknown_xml_ignored() {
        let body = r#"<?xml version="1.0"?><opml version="2.0"></opml>"#;
        let s = site_with_files(&[("opml.xml", body)]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn no_xml_files_returns_empty() {
        let s = site_with_files(&[("index.html", "<html></html>")]);
        let f = FeedsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn unreadable_xml_skipped() {
        // Create a directory with .xml extension so read_to_string errors.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("malformed.xml")).unwrap();
        let s = Site {
            root: root.clone(),
            html_files: Vec::new(),
        };
        // Should not panic — the gate `continue`s on read error.
        let _ = FeedsGate.run(&s, &AuditOptions::default());
        std::mem::forget(tmp);
    }

    #[test]
    fn non_utf8_xml_file_is_skipped() {
        // Invalid UTF-8 makes read_to_string fail on a real file that
        // walk_files does return, driving the per-file `continue`.
        let s = site_with_files(&[("good.xml", "<urlset></urlset>")]);
        std::fs::write(s.root.join("bad.xml"), [0xFFu8, 0xFE, 0x00, 0x9F])
            .unwrap();
        let f = FeedsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "non-UTF8 xml must be skipped: {f:?}");
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = FeedsGate;
        assert_eq!(g.name(), "feeds");
        assert!(g.explain().contains("RSS"));
        // Debug + Clone + Copy traits
        let _: FeedsGate = g;
        let _clone = g;
        let dbg = format!("{g:?}");
        assert!(dbg.contains("FeedsGate"));
    }
}
