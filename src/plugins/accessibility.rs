// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Automated WCAG accessibility checker and ARIA validation plugin.
//!
//! This is a thin SSG-side wrapper around the standalone
//! [`ssg-a11y`](https://docs.rs/ssg-a11y) crate, which contains all of
//! the actual WCAG 2.2 rule-checking logic (that crate has zero
//! dependency on SSG's [`Plugin`] trait, [`PluginContext`], or
//! [`SsgError`], so it can be reused by other Rust web frameworks). This
//! module's job is purely the SSG integration: walking the compiled
//! site's HTML files, calling into `ssg_a11y::check_page` for each one,
//! and writing the two build artifacts:
//!
//! - `accessibility-report.json` — issue list per page (existing format).
//! - `wcag-compliance.json` — compliance matrix mapping each WCAG 2.2
//!   criterion to its automation status (automated / runtime-only /
//!   manual / not-applicable) plus a per-page pass/fail summary.
//!
//! See the [`ssg_a11y`] crate docs for the full list of WCAG criteria
//! checked.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use serde::Serialize;
use std::fs;

// Re-exported so `ssg::accessibility::{AccessibilityReport, ...}` keeps
// working unchanged for existing consumers — the types themselves now
// live in the standalone `ssg-a11y` crate.
pub use ssg_a11y::{
    AccessibilityIssue, AccessibilityReport, CriterionEntry, CriterionStatus,
    PageReport, WcagComplianceReport,
};

/// Plugin that checks generated HTML for WCAG compliance.
///
/// Runs in `after_compile`. Non-blocking by default (logs warnings).
#[derive(Debug, Clone, Copy)]
pub struct AccessibilityPlugin;

impl Plugin for AccessibilityPlugin {
    fn name(&self) -> &'static str {
        "accessibility"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        let html_files = ctx.get_html_files();
        let mut report = AccessibilityReport {
            pages_scanned: html_files.len(),
            total_issues: 0,
            wcag_version: "2.2".to_string(),
            pages: Vec::new(),
        };

        // Per-criterion fail set, used to populate the compliance matrix.
        let mut failed_criteria: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for path in &html_files {
            let html = fs::read_to_string(path).with_path(path)?;
            let rel = path
                .strip_prefix(&ctx.site_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let issues = ssg_a11y::check_page(&html);
            if !issues.is_empty() {
                for issue in &issues {
                    let _ = failed_criteria.insert(issue.criterion.clone());
                    log::warn!(
                        "[a11y] {} — [{}] {}",
                        rel,
                        issue.criterion,
                        issue.message
                    );
                }
                report.total_issues += issues.len();
                report.pages.push(PageReport { path: rel, issues });
            }
        }

        // Write the per-page issue report.
        let report_path = ctx.site_dir.join("accessibility-report.json");
        let json = to_pretty_json(&report, &report_path)?;
        fs::write(&report_path, json).with_path(&report_path)?;

        // Write the WCAG 2.2 compliance matrix.
        let compliance = ssg_a11y::build_compliance_report(
            html_files.len(),
            &failed_criteria,
        );
        let matrix_path = ctx.site_dir.join("wcag-compliance.json");
        let json_compliance = to_pretty_json(&compliance, &matrix_path)?;
        fs::write(&matrix_path, json_compliance).with_path(&matrix_path)?;

        if report.total_issues > 0 {
            log::warn!(
                "[a11y] {} issue(s) across {} page(s). Reports: {} + {}",
                report.total_issues,
                report.pages.len(),
                report_path.display(),
                matrix_path.display()
            );
        } else {
            log::info!(
                "[a11y] All {} page(s) passed checks. Reports: {} + {}",
                report.pages_scanned,
                report_path.display(),
                matrix_path.display()
            );
        }

        Ok(())
    }
}

/// Serialises a report artifact as pretty-printed JSON, mapping any
/// serialisation failure onto [`SsgError::Io`] keyed by the artifact
/// path it was destined for.
fn to_pretty_json<T: Serialize>(
    value: &T,
    path: &std::path::Path,
) -> Result<String, SsgError> {
    fail_point!("accessibility::to-json", |_| {
        Err(SsgError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other("injected: accessibility::to-json"),
        })
    });
    serde_json::to_string_pretty(value).map_err(|e| SsgError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    })
}

#[cfg(test)]
fn collect_html_files(
    dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, SsgError> {
    crate::walk::walk_files(dir, "html")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    fn test_ctx(site_dir: &Path) -> PluginContext {
        crate::test_support::init_logger();
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            site_dir,
            Path::new("templates"),
        )
    }

    // -------------------------------------------------------------------
    // Plugin trait surface
    // -------------------------------------------------------------------

    #[test]
    fn name_returns_static_accessibility_identifier() {
        assert_eq!(AccessibilityPlugin.name(), "accessibility");
    }

    #[test]
    fn after_compile_missing_site_dir_returns_ok_without_writing() {
        // The `!ctx.site_dir.exists()` early return.
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let ctx = test_ctx(&missing);
        AccessibilityPlugin.after_compile(&ctx).unwrap();
        assert!(!missing.join("accessibility-report.json").exists());
    }

    #[test]
    #[serial_test::parallel(accessibility_failpoint)]
    fn after_compile_clean_pages_logs_all_passed() {
        // The `else` branch logging "All N pages passed".
        // Requires a site with at least one clean page.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(
            site.join("index.html"),
            r#"<html lang="en"><head></head><body>
            <nav aria-label="Main"><a href="/">Home</a></nav>
            <main><h1>T</h1><img src="a.jpg" alt="A"></main>
            </body></html>"#,
        )
        .unwrap();

        let ctx = test_ctx(&site);
        AccessibilityPlugin.after_compile(&ctx).unwrap();
        // Report should exist and show zero issues.
        let report: AccessibilityReport = serde_json::from_str(
            &fs::read_to_string(site.join("accessibility-report.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(report.total_issues, 0);
    }

    // -------------------------------------------------------------------
    // collect_html_files — depth guard + non-html filter
    // -------------------------------------------------------------------

    #[test]
    fn collect_html_files_filters_non_html_extensions() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.html"), "").unwrap();
        fs::write(dir.path().join("b.css"), "").unwrap();
        let result = collect_html_files(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn collect_html_files_skips_non_directories_in_stack() {
        // Covered by the normal tempdir walk.
        let dir = tempdir().unwrap();
        let result = collect_html_files(&dir.path().join("missing")).unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    #[serial_test::parallel(accessibility_failpoint)]
    fn test_plugin_writes_report() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(
            site.join("index.html"),
            r#"<html><head></head><body><main><img src="x.jpg"></main></body></html>"#,
        )
        .unwrap();

        let ctx = test_ctx(&site);
        AccessibilityPlugin.after_compile(&ctx).unwrap();

        let report_path = site.join("accessibility-report.json");
        assert!(report_path.exists());

        let content = fs::read_to_string(&report_path).unwrap();
        let report: AccessibilityReport =
            serde_json::from_str(&content).unwrap();
        assert_eq!(report.pages_scanned, 1);
        assert!(report.total_issues > 0);
        assert_eq!(report.wcag_version, "2.2");
    }

    #[test]
    #[serial_test::parallel(accessibility_failpoint)]
    fn test_compliance_matrix_emitted() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(
            site.join("index.html"),
            r#"<html lang="en"><head></head><body><main>
                <h1>OK</h1>
                <a href="/contact">Contact</a>
            </main></body></html>"#,
        )
        .unwrap();

        let ctx = test_ctx(&site);
        AccessibilityPlugin.after_compile(&ctx).unwrap();

        let matrix_path = site.join("wcag-compliance.json");
        assert!(matrix_path.exists());

        let content = fs::read_to_string(&matrix_path).unwrap();
        let matrix: WcagComplianceReport =
            serde_json::from_str(&content).unwrap();
        assert_eq!(matrix.wcag_version, "2.2");
        assert_eq!(matrix.pages_scanned, 1);
        // The matrix carries every WCAG 2.2 row we listed in
        // build_compliance_report, including the three additions.
        let names: Vec<&str> = matrix
            .criteria
            .iter()
            .map(|c| c.criterion.as_str())
            .collect();
        assert!(names.contains(&"2.4.13"));
        assert!(names.contains(&"2.5.8"));
        assert!(names.contains(&"3.2.6"));
    }

    #[test]
    #[serial_test::parallel(accessibility_failpoint)]
    fn after_compile_write_failure_returns_io_error() {
        let dir = tempdir().unwrap();

        // Create a file where it expects the site directory to be.
        let file_path = dir.path().join("site");
        fs::write(&file_path, "").unwrap();

        let ctx = test_ctx(&file_path);
        let res = AccessibilityPlugin.after_compile(&ctx);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == &file_path.join("accessibility-report.json"))
        );
    }

    // ── to_pretty_json ──────────────────────────────────────────────

    #[test]
    #[serial_test::parallel(accessibility_failpoint)]
    fn to_pretty_json_maps_serde_failure_to_io_error() {
        // JSON object keys must be strings — a tuple-keyed map makes
        // `serde_json::to_string_pretty` fail, driving the error arm.
        let bad: std::collections::BTreeMap<(u8, u8), u8> =
            std::iter::once(((1, 2), 3)).collect();
        let err = to_pretty_json(&bad, Path::new("artifact.json"))
            .expect_err("non-string map keys must fail serialisation");
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == Path::new("artifact.json"))
        );
    }

    #[test]
    #[serial_test::parallel(accessibility_failpoint)]
    fn after_compile_matrix_write_failure_returns_io_error() {
        // The issue report writes fine, but `wcag-compliance.json`
        // already exists as a *directory* so the second write fails.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("wcag-compliance.json")).unwrap();

        let ctx = test_ctx(dir.path());
        let err = AccessibilityPlugin.after_compile(&ctx).unwrap_err();
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == &dir.path().join("wcag-compliance.json"))
        );
        assert!(
            dir.path().join("accessibility-report.json").exists(),
            "issue report must have been written before the failure"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::parallel(accessibility_failpoint)]
    fn after_compile_unreadable_page_returns_io_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let page = dir.path().join("index.html");
        fs::write(&page, "<html lang=\"en\"><body></body></html>").unwrap();
        fs::set_permissions(&page, fs::Permissions::from_mode(0o000)).unwrap();

        let ctx = test_ctx(dir.path());
        let res = AccessibilityPlugin.after_compile(&ctx);

        // Restore permissions before asserting so cleanup always works.
        fs::set_permissions(&page, fs::Permissions::from_mode(0o644)).unwrap();

        let err = res.expect_err("unreadable page must abort the scan");
        assert!(matches!(err, SsgError::Io { ref path, .. } if path == &page));
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard<'a>(&'a str);

    impl Drop for FailGuard<'_> {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    fn ctx_for(dir: &Path) -> PluginContext {
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            dir,
            Path::new("templates"),
        )
    }

    #[test]
    #[serial_test::serial(accessibility_failpoint)]
    fn report_serialisation_failure_aborts_after_compile() {
        let _guard = FailGuard("accessibility::to-json");
        fail::cfg("accessibility::to-json", "return").unwrap();

        let dir = tempdir().unwrap();
        let err = AccessibilityPlugin
            .after_compile(&ctx_for(dir.path()))
            .expect_err("first serialisation must fail");
        assert!(err.to_string().contains("accessibility-report.json"));
    }

    #[test]
    #[serial_test::serial(accessibility_failpoint)]
    fn matrix_serialisation_failure_aborts_after_compile() {
        // First call (issue report) succeeds, second (matrix) fails.
        let _guard = FailGuard("accessibility::to-json");
        fail::cfg("accessibility::to-json", "1*off->1*return").unwrap();

        let dir = tempdir().unwrap();
        let err = AccessibilityPlugin
            .after_compile(&ctx_for(dir.path()))
            .expect_err("second serialisation must fail");
        assert!(err.to_string().contains("wcag-compliance.json"));
    }
}
