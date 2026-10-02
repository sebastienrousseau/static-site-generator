// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! JSON-LD (Schema.org) semantic audit gate.
//!
//! Delegates to [`crate::seo::jsonld::validate_jsonld`] so this gate
//! and the build-time `JsonLdPlugin` agree on the same required-field
//! matrix per `@type`.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use crate::seo::validate_jsonld;

const NAME: &str = "jsonld";

/// JSON-LD Schema.org gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::jsonld::JsonLdGate;
/// assert_eq!(JsonLdGate.name(), "jsonld");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct JsonLdGate;

impl AuditGate for JsonLdGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Extracts every <script type=\"application/ld+json\"> block on \
         each page, asserts it parses as JSON, and validates the \
         required fields for its declared @type (Article, WebPage, \
         BreadcrumbList, FAQPage, LocalBusiness, Organization). \
         Unparseable JSON or missing-required-field findings are \
         emitted at error severity; unknown types are pass-through."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);
            for err in validate_jsonld(&html) {
                findings.push(
                    Finding::new(
                        NAME,
                        Severity::Error,
                        format!(
                            "[{}] missing/invalid `{}` — {}",
                            err.schema_type, err.field, err.reason
                        ),
                    )
                    .with_code(format!("JSONLD-{}", err.schema_type))
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

    #[test]
    fn passing_jsonld_produces_no_findings() {
        let html = r#"<html><head><script type="application/ld+json">
            {"@context":"https://schema.org","@type":"Organization","name":"Acme","url":"https://acme.test"}
        </script></head><body></body></html>"#;
        let f = JsonLdGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn unparseable_jsonld_is_flagged() {
        let html = r#"<html><head><script type="application/ld+json">{ not json }</script></head><body></body></html>"#;
        let f = JsonLdGate.run(&site(html), &AuditOptions::default());
        assert!(
            f.iter().any(|x| matches!(x.severity, Severity::Error)),
            "expected an error finding, got {f:?}"
        );
    }

    #[test]
    fn empty_site_produces_no_findings() {
        let s = Site {
            root: PathBuf::from("/nonexistent"),
            html_files: Vec::new(),
        };
        let f = JsonLdGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = JsonLdGate;
        assert_eq!(g.name(), "jsonld");
        assert!(g.explain().contains("JSON"));
        let _copy: JsonLdGate = g;
        let _clone = g;
        assert!(format!("{g:?}").contains("JsonLdGate"));
    }

    #[test]
    fn unreadable_html_file_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("ghost.html");
        let s = Site {
            root: tmp.path().to_path_buf(),
            html_files: vec![bogus],
        };
        let f = JsonLdGate.run(&s, &AuditOptions::default());
        std::mem::forget(tmp);
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn missing_required_field_emits_jsonld_prefixed_code() {
        let html = r#"<html><head><script type="application/ld+json">
            {"@context":"https://schema.org","@type":"Article"}
        </script></head><body></body></html>"#;
        let f = JsonLdGate.run(&site(html), &AuditOptions::default());
        assert!(!f.is_empty(), "expected at least one missing-field error");
        for finding in &f {
            assert_eq!(finding.severity, Severity::Error);
            let code = finding.code.as_deref();
            assert!(
                code.is_some_and(|c| c.starts_with("JSONLD-")),
                "code should be JSONLD-prefixed, got {code:?}"
            );
        }
    }

    #[test]
    fn unknown_type_is_pass_through() {
        let html = r#"<html><head><script type="application/ld+json">
            {"@context":"https://schema.org","@type":"WidgetType","name":"x"}
        </script></head><body></body></html>"#;
        let f = JsonLdGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "unknown types are pass-through; got {f:?}");
    }

    #[test]
    fn multiple_html_files_aggregate_findings() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.html");
        let b = tmp.path().join("b.html");
        std::fs::write(
            &a,
            r#"<html><head><script type="application/ld+json">{ bad </script></head></html>"#,
        )
        .unwrap();
        std::fs::write(
            &b,
            r#"<html><head><script type="application/ld+json">also bad</script></head></html>"#,
        )
        .unwrap();
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        let s = Site {
            root,
            html_files: vec![a, b],
        };
        let f = JsonLdGate.run(&s, &AuditOptions::default());
        assert!(f.len() >= 2, "expected at least one per file, got {f:?}");
    }

    #[test]
    fn no_jsonld_blocks_produces_no_findings() {
        let html =
            "<html><head><title>plain</title></head><body></body></html>";
        let f = JsonLdGate.run(&site(html), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }
}
