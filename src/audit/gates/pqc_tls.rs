// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Post-quantum-crypto TLS readiness gate (depends on E6 — issue #550).
//!
//! Inspects the deploy adapter output for `_headers` (Cloudflare /
//! Netlify) or `wrangler.toml` and asserts the TLS+HSTS posture
//! required for PQC roll-forward:
//! - TLS 1.3 declared (`min-version`, `tls-1.3` etc.).
//! - HSTS header present with `max-age >= 31536000` (one year).
//!
//! When neither file exists (the common case before E6 merges) the
//! gate emits a single *info* finding noting the skip rather than
//! failing — exactly as the issue body specifies.

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use std::fs;

const NAME: &str = "pqc_tls";
const HSTS_MIN_AGE: u64 = 31_536_000; // 1 year

/// PQC TLS readiness gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::pqc_tls::PqcTlsGate;
/// assert_eq!(PqcTlsGate.name(), "pqc_tls");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct PqcTlsGate;

impl AuditGate for PqcTlsGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Inspects _headers / wrangler.toml emitted by the deploy \
         adapter (E6) for TLS 1.3 + HSTS posture: max-age must be \
         >= 31536000 (one year). When neither file exists, the gate \
         emits an info note and skips — it upgrades to enforcement \
         once issue #550 (E6) merges and the files appear."
    }

    fn run(&self, site: &Site, _opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();

        let headers_path = site.root.join("_headers");
        let wrangler_path = site.root.join("wrangler.toml");
        let parent_wrangler =
            site.root.parent().map(|p| p.join("wrangler.toml"));

        let has_headers = headers_path.exists();
        let has_wrangler = wrangler_path.exists()
            || parent_wrangler.as_ref().is_some_and(|p| p.exists());

        if !has_headers && !has_wrangler {
            findings.push(
                Finding::new(
                    NAME,
                    Severity::Info,
                    "No _headers or wrangler.toml found; gate skipped (depends on E6 — issue #550)",
                )
                .with_code("PQC-INPUT-MISSING"),
            );
            return findings;
        }

        if let Ok(text) = fs::read_to_string(&headers_path) {
            check_text(&text, "_headers", &mut findings);
        }
        if let Ok(text) = fs::read_to_string(&wrangler_path) {
            check_text(&text, "wrangler.toml", &mut findings);
        } else if let Some(pw) = parent_wrangler {
            if let Ok(text) = fs::read_to_string(&pw) {
                check_text(&text, "wrangler.toml", &mut findings);
            }
        }

        findings
    }
}

fn check_text(text: &str, source: &str, findings: &mut Vec<Finding>) {
    let lower = text.to_lowercase();

    // HSTS presence + max-age threshold
    if !lower.contains("strict-transport-security") {
        findings.push(
            Finding::new(
                NAME,
                Severity::Error,
                format!(
                    "{source} declares no Strict-Transport-Security header"
                ),
            )
            .with_code("PQC-HSTS-MISSING")
            .with_path(source.to_string()),
        );
    } else if let Some(age) = extract_max_age(&lower) {
        if age < HSTS_MIN_AGE {
            findings.push(
                Finding::new(
                    NAME,
                    Severity::Error,
                    format!(
                        "{source} HSTS max-age={age} below the 1-year (31536000) PQC-roll-forward threshold"
                    ),
                )
                .with_code("PQC-HSTS-SHORT")
                .with_path(source.to_string()),
            );
        }
    } else {
        findings.push(
            Finding::new(
                NAME,
                Severity::Warn,
                format!("{source} HSTS header present but max-age could not be parsed"),
            )
            .with_code("PQC-HSTS-UNPARSEABLE")
            .with_path(source.to_string()),
        );
    }

    // TLS 1.3 declared anywhere.
    //
    // Note: a `min_version = "tlsv1.3"` wrangler.toml entry is already
    // caught by the `tlsv1.3` clause below (the former is a strict
    // substring of the latter), so it isn't listed as its own
    // alternative — doing so would be unreachable dead code.
    let mentions_tls13 = lower.contains("tls-1.3")
        || lower.contains("tlsv1.3")
        || lower.contains("tls13")
        // The spelling ssg's own `_headers` generator writes.
        || lower.contains("tls 1.3")
        || lower.contains("\"1.3\"");
    if !mentions_tls13 {
        findings.push(
            Finding::new(
                NAME,
                Severity::Warn,
                format!(
                    "{source} does not declare TLS 1.3 (required for PQC kex)"
                ),
            )
            .with_code("PQC-TLS13-MISSING")
            .with_path(source.to_string()),
        );
    }
}

fn extract_max_age(lower: &str) -> Option<u64> {
    let key = "max-age=";
    let idx = lower.find(key)?;
    let after = &lower[idx + key.len()..];
    let digits: String =
        after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn site_with_file(name: &str, content: &str) -> Site {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(name), content).unwrap();
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        Site {
            root,
            html_files: Vec::new(),
        }
    }

    fn empty_site() -> Site {
        Site {
            root: PathBuf::from("/nonexistent"),
            html_files: Vec::new(),
        }
    }

    #[test]
    fn absent_inputs_emit_info_skip() {
        let f = PqcTlsGate.run(&empty_site(), &AuditOptions::default());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Info);
        assert_eq!(f[0].code.as_deref(), Some("PQC-INPUT-MISSING"));
    }

    #[test]
    fn good_headers_file_passes() {
        let content = "/*\n  Strict-Transport-Security: max-age=31536000; includeSubDomains\n  TLS: TLSv1.3\n";
        let s = site_with_file("_headers", content);
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn short_hsts_max_age_flags_error() {
        let content =
            "/*\n  Strict-Transport-Security: max-age=3600\n  TLS: TLSv1.3\n";
        let s = site_with_file("_headers", content);
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("PQC-HSTS-SHORT")));
    }

    #[test]
    fn missing_hsts_header_flags_error() {
        let content = "/*\n  TLS: TLSv1.3\n";
        let s = site_with_file("_headers", content);
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("PQC-HSTS-MISSING")
                && x.severity == Severity::Error));
    }

    #[test]
    fn missing_tls_13_declaration_warns() {
        let content = "/*\n  Strict-Transport-Security: max-age=63072000\n";
        let s = site_with_file("_headers", content);
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        let tls = f
            .iter()
            .find(|x| x.code.as_deref() == Some("PQC-TLS13-MISSING"))
            .expect("TLS 1.3 finding");
        assert_eq!(tls.severity, Severity::Warn);
    }

    #[test]
    fn unparseable_max_age_warns() {
        let content = "/*\n  Strict-Transport-Security: max-age=not-a-number\n  TLS: TLSv1.3\n";
        let s = site_with_file("_headers", content);
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("PQC-HSTS-UNPARSEABLE")
                && x.severity == Severity::Warn));
    }

    #[test]
    fn wrangler_toml_is_scanned() {
        let content =
            "[headers]\nstrict-transport-security = \"max-age=63072000\"\nmin_version = \"tlsv1.3\"\n";
        let s = site_with_file("wrangler.toml", content);
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn tls13_alt_spellings_accepted() {
        // "TLS 1.3" with a space is what ssg's own generated _headers
        // writes, so leaving it out made the generator emit a file its
        // own auditor reported as missing a TLS 1.3 declaration.
        for spelling in ["tlsv1.3", "tls13", "\"1.3\"", "tls 1.3", "tls-1.3"] {
            // Short max-age yields PQC-HSTS-SHORT, keeping `f`
            // non-empty so the no-TLS13-MISSING predicate evaluates.
            let content = format!(
                "/*\n  Strict-Transport-Security: max-age=3600\n  TLS: {spelling}\n"
            );
            let s = site_with_file("_headers", &content);
            let f = PqcTlsGate.run(&s, &AuditOptions::default());
            assert!(f
                .iter()
                .any(|x| x.code.as_deref() == Some("PQC-HSTS-SHORT")));
            assert!(
                f.iter()
                    .all(|x| x.code.as_deref() != Some("PQC-TLS13-MISSING")),
                "spelling `{spelling}` did not satisfy TLS 1.3 check: {f:?}"
            );
        }
    }

    #[test]
    fn parent_dir_wrangler_toml_is_scanned() {
        // wrangler.toml lives beside (not inside) the site root — the
        // usual layout when `public/` is the deploy output.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("public");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            tmp.path().join("wrangler.toml"),
            "strict-transport-security = \"max-age=63072000\"\n\
             min_version = \"tlsv1.3\"\n",
        )
        .unwrap();
        std::mem::forget(tmp);
        let s = Site {
            root,
            html_files: Vec::new(),
        };
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "parent wrangler.toml must be scanned: {f:?}");
    }

    #[test]
    fn hsts_without_max_age_key_warns_unparseable() {
        // No `max-age=` at all: extract_max_age's find() misses.
        let content = "/*\n  Strict-Transport-Security: includeSubDomains\n  TLS: TLSv1.3\n";
        let s = site_with_file("_headers", content);
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("PQC-HSTS-UNPARSEABLE")));
    }

    #[test]
    fn root_without_parent_short_circuits_parent_wrangler_lookup() {
        // `/` has no parent, so `site.root.parent()` is `None` and
        // `parent_wrangler` is `None` — drives the `None` arm of
        // `parent_wrangler.as_ref().is_some_and(...)` inside
        // `has_wrangler`, which every other test (all rooted under a
        // tempdir, which always has a parent) never reaches.
        let s = Site {
            root: PathBuf::from("/"),
            html_files: Vec::new(),
        };
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        if !std::path::Path::new("/_headers").exists()
            && !std::path::Path::new("/wrangler.toml").exists()
        {
            assert_eq!(f.len(), 1);
            assert_eq!(f[0].code.as_deref(), Some("PQC-INPUT-MISSING"));
        }
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = PqcTlsGate;
        assert_eq!(g.name(), "pqc_tls");
        assert!(g.explain().contains("HSTS"));
        let _copy: PqcTlsGate = g;
        let _clone = g;
        assert!(format!("{g:?}").contains("PqcTlsGate"));
    }

    #[test]
    fn hsts_accepts_trailing_directives() {
        let content = "/*\n  Strict-Transport-Security: max-age=63072000; includeSubDomains; preload\n  TLS: TLSv1.3\n";
        let s = site_with_file("_headers", content);
        let f = PqcTlsGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty(), "should accept trailing directives: {f:?}");
    }
}
