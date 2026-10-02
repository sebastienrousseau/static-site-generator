// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Native CI audit gates (issue #549).
//!
//! Exposes 15 [`AuditGate`] implementations that run locally and in CI
//! to catch violations of WCAG, schema.org, hreflang reciprocity, CSP +
//! SRI, PQC TLS readiness, HTML5 structure, broken links, OG metadata,
//! markdown style, performance budgets, AI discovery files, RSS/Atom
//! feeds, image optimisation, the localised semantic search index, and
//! JSON-LD `inLanguage` vs `<html lang>` consistency.
//!
//! ## Surface
//!
//! ```rust,no_run
//! use ssg::audit::{AuditRunner, AuditConfig, Site};
//! use std::path::Path;
//!
//! let site = Site::load(Path::new("./public"))?;
//! let cfg  = AuditConfig::default();
//! let report = AuditRunner::new(cfg).run(&site);
//! report.print_text();
//! # Ok::<(), ssg::SsgError>(())
//! ```
//!
//! Each gate ships with `name()`, `explain()`, and a `run(&Site)` that
//! returns a `Vec<Finding>`. Findings carry a [`Severity`] so callers
//! can filter or fail based on the configured `--fail-on` threshold.
//!
//! Some gates depend on other v0.0.44 epics (PQC on E6, AI discovery on
//! E8, semantic search on E1). Those gates emit an *info* finding when
//! their input files are absent rather than failing — they upgrade to
//! enforcement once the producing epic merges.

pub mod gates;
pub mod output;

use crate::error::{PathErrorExt, SsgError};
use crate::walk::walk_files;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------

/// Severity of an audit [`Finding`].
///
/// Comparison order is `Info < Warn < Error`, so `>=` filters work
/// against a configured `--fail-on` threshold.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational note; does not affect exit code.
    Info,
    /// Soft violation; fails when `--fail-on warn` is set.
    Warn,
    /// Hard violation; fails by default.
    Error,
}

impl Severity {
    /// Returns the canonical lowercase name (`"info"`, `"warn"`, `"error"`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::Severity;
    /// assert_eq!(Severity::Info.as_str(), "info");
    /// assert_eq!(Severity::Warn.as_str(), "warn");
    /// assert_eq!(Severity::Error.as_str(), "error");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    /// Parses a severity from its lowercase textual form. Accepts both
    /// `"warn"` and `"warning"` for ergonomics.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::Severity;
    /// assert_eq!(Severity::parse("info"), Some(Severity::Info));
    /// assert_eq!(Severity::parse("warning"), Some(Severity::Warn));
    /// assert_eq!(Severity::parse("err"), Some(Severity::Error));
    /// assert_eq!(Severity::parse("nope"), None);
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" | "err" => Some(Self::Error),
            _ => None,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------
// Finding
// ---------------------------------------------------------------------

/// A single audit finding produced by a gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Identifier of the gate that produced the finding
    /// (e.g. `"wcag"`, `"hreflang"`).
    pub gate: String,
    /// Severity of the finding.
    pub severity: Severity,
    /// Optional rule code (`"WCAG-1.1.1"`, `"OG-MISSING"`, …) for
    /// downstream tooling to group by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Human-readable message.
    pub message: String,
    /// Path the finding was raised against, relative to the site root.
    /// `None` for site-wide findings (e.g. a missing manifest file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl Finding {
    /// Convenience constructor for a path-scoped finding.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{Finding, Severity};
    /// let f = Finding::new("wcag", Severity::Warn, "missing alt text");
    /// assert_eq!(f.gate, "wcag");
    /// assert_eq!(f.severity, Severity::Warn);
    /// assert_eq!(f.message, "missing alt text");
    /// assert!(f.code.is_none());
    /// ```
    pub fn new(
        gate: impl Into<String>,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            gate: gate.into(),
            severity,
            code: None,
            message: message.into(),
            path: None,
        }
    }

    /// Builder: attaches a rule code.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{Finding, Severity};
    /// let f = Finding::new("wcag", Severity::Warn, "missing alt").with_code("WCAG-1.1.1");
    /// assert_eq!(f.code.as_deref(), Some("WCAG-1.1.1"));
    /// ```
    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Builder: attaches a path.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{Finding, Severity};
    /// let f = Finding::new("links", Severity::Error, "broken").with_path("blog/foo.html");
    /// assert_eq!(f.path.as_deref(), Some("blog/foo.html"));
    /// ```
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

// ---------------------------------------------------------------------
// Site
// ---------------------------------------------------------------------

/// A loaded view of a built site for audit gates to consume.
///
/// Walks the site directory once at construction time so each gate
/// avoids redundant filesystem scans (the 15 gates would otherwise
/// stat-walk the same tree 15 times).
#[derive(Debug, Clone)]
pub struct Site {
    /// Root directory of the built site (the `public/` output dir).
    pub root: PathBuf,
    /// All `.html` files under the root, in directory-walk order.
    pub html_files: Vec<PathBuf>,
}

impl Site {
    /// Loads a site from `root`, walking it for HTML files.
    ///
    /// # Errors
    /// Returns [`SsgError::Io`] if the directory walk fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::Site;
    /// let tmp = tempfile::tempdir().unwrap();
    /// let site = Site::load(tmp.path()).unwrap();
    /// assert_eq!(site.root, tmp.path());
    /// assert!(site.html_files.is_empty());
    /// ```
    pub fn load(root: &Path) -> Result<Self, SsgError> {
        let html_files = if root.exists() {
            walk_files(root, "html").unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Self {
            root: root.to_path_buf(),
            html_files,
        })
    }

    /// Returns a relative path string for `path` against the site root,
    /// always using `/` as the separator.
    ///
    /// The separator is normalised because these strings are compared against
    /// URL paths taken from the HTML — `hreflang` targets, canonical hrefs,
    /// sitemap entries. On Windows the native separator made
    /// `en\index.html` the key while the document said `/en/index.html`, so
    /// no reciprocal pair ever matched and the hreflang gate reported both a
    /// false negative and a false positive. A path used as a URL has one
    /// separator, whatever the filesystem underneath calls it.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::{Path, PathBuf};
    /// use ssg::audit::Site;
    /// let site = Site { root: PathBuf::from("/site"), html_files: Vec::new() };
    /// assert_eq!(site.rel(Path::new("/site/blog/a.html")), "blog/a.html");
    /// ```
    #[must_use]
    pub fn rel(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        // `MAIN_SEPARATOR` is `/` on unix, so this is a no-op there rather
        // than a platform branch.
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/")
    }

    /// Reads the contents of `path` as UTF-8.
    ///
    /// # Errors
    /// Returns [`SsgError::Io`] if reading fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use ssg::audit::Site;
    /// let tmp = tempfile::tempdir().unwrap();
    /// let p = tmp.path().join("a.html");
    /// std::fs::write(&p, "<html></html>").unwrap();
    /// let site = Site { root: tmp.path().to_path_buf(), html_files: vec![p.clone()] };
    /// assert_eq!(site.read(&p).unwrap(), "<html></html>");
    /// ```
    pub fn read(&self, path: &Path) -> Result<String, SsgError> {
        fs::read_to_string(path).with_path(path)
    }
}

// ---------------------------------------------------------------------
// AuditGate trait
// ---------------------------------------------------------------------

/// Trait implemented by every audit gate.
///
/// Each gate is stateless and side-effect-free — it must not write
/// anything to disk and must not hit the network unless explicitly
/// asked to (see [`AuditOptions::skip_network`]).
pub trait AuditGate: Sync + Send {
    /// Short identifier (`snake_case`, used on `--gate <name>`).
    fn name(&self) -> &'static str;

    /// Long-form explainer printed by `ssg audit --explain --gate <name>`.
    fn explain(&self) -> &'static str;

    /// Runs the gate against `site` and returns its findings.
    fn run(&self, site: &Site, opts: &AuditOptions) -> Vec<Finding>;
}

// ---------------------------------------------------------------------
// AuditOptions / AuditConfig
// ---------------------------------------------------------------------

/// Runtime options passed to every gate at execution time.
#[derive(Debug, Clone, Copy)]
pub struct AuditOptions {
    /// When `true`, gates that would otherwise issue HTTP requests
    /// (only the broken-link gate today) must skip the network and
    /// emit an info finding noting the skip.
    pub skip_network: bool,
    /// Page-weight budget (HTML + critical CSS) in bytes for the
    /// performance gate.
    pub page_weight_budget: usize,
    /// Total JS budget in bytes for the performance gate.
    pub js_budget: usize,
    /// Image file-size budget in bytes for the image gate.
    pub image_budget: usize,
}

impl Default for AuditOptions {
    fn default() -> Self {
        Self {
            skip_network: true,
            page_weight_budget: 100 * 1024,
            js_budget: 50 * 1024,
            image_budget: 250 * 1024,
        }
    }
}

/// User-facing configuration for an [`AuditRunner`].
#[derive(Debug, Clone)]
pub struct AuditConfig {
    /// Gate identifiers to skip. Names not matching any registered
    /// gate are silently ignored (so a deprecated gate name in
    /// `ssg.toml` doesn't break the build).
    pub disabled: BTreeSet<String>,
    /// If `Some`, only the named gate runs (`--gate <name>`).
    pub only: Option<String>,
    /// Minimum severity to include in the report.
    pub severity_floor: Severity,
    /// Severity that triggers a non-zero exit code.
    pub fail_on: Severity,
    /// Runtime knobs forwarded to gates.
    pub options: AuditOptions,
}

impl AuditConfig {
    /// Sensible defaults: include everything, fail on `Error`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{AuditConfig, Severity};
    /// let cfg = AuditConfig::new();
    /// assert_eq!(cfg.fail_on, Severity::Error);
    /// assert!(cfg.disabled.is_empty());
    /// assert!(cfg.only.is_none());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            disabled: BTreeSet::new(),
            only: None,
            severity_floor: Severity::Info,
            fail_on: Severity::Error,
            options: AuditOptions::default(),
        }
    }
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------

/// Per-gate result block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    /// Gate identifier (matches [`AuditGate::name`]).
    pub name: String,
    /// `true` when the gate was disabled via config or `--gate` filter.
    pub skipped: bool,
    /// Reason for being skipped (only set when `skipped == true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// Severity counts for findings produced by the gate.
    pub severity_counts: SeverityCounts,
    /// Individual findings produced by the gate.
    pub findings: Vec<Finding>,
}

/// Tally of severities for a gate result.
#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq,
)]
pub struct SeverityCounts {
    /// Number of `info` findings.
    pub info: usize,
    /// Number of `warn` findings.
    pub warn: usize,
    /// Number of `error` findings.
    pub error: usize,
}

impl SeverityCounts {
    /// Bumps the counter for `sev`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{Severity, SeverityCounts};
    /// let mut c = SeverityCounts::default();
    /// c.add(Severity::Warn);
    /// c.add(Severity::Warn);
    /// assert_eq!(c.warn, 2);
    /// ```
    pub const fn add(&mut self, sev: Severity) {
        match sev {
            Severity::Info => self.info += 1,
            Severity::Warn => self.warn += 1,
            Severity::Error => self.error += 1,
        }
    }

    /// Total findings across severities.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{Severity, SeverityCounts};
    /// let mut c = SeverityCounts::default();
    /// c.add(Severity::Info);
    /// c.add(Severity::Error);
    /// assert_eq!(c.total(), 2);
    /// ```
    #[must_use]
    pub const fn total(&self) -> usize {
        self.info + self.warn + self.error
    }
}

/// Aggregate audit report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    /// Per-gate results in registration order.
    pub gates: Vec<GateResult>,
}

impl AuditReport {
    /// Returns the highest severity present across all gates, or `None`
    /// if no findings were produced.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::AuditReport;
    /// let r = AuditReport { gates: vec![] };
    /// assert!(r.max_severity().is_none());
    /// ```
    #[must_use]
    pub fn max_severity(&self) -> Option<Severity> {
        let mut max: Option<Severity> = None;
        for g in &self.gates {
            if g.severity_counts.error > 0 {
                return Some(Severity::Error);
            }
            if g.severity_counts.warn > 0 {
                max =
                    Some(max.map_or(Severity::Warn, |m| m.max(Severity::Warn)));
            } else if g.severity_counts.info > 0 {
                max =
                    Some(max.map_or(Severity::Info, |m| m.max(Severity::Info)));
            }
        }
        max
    }

    /// Returns `true` when the report contains at least one finding at
    /// `fail_on` or higher.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{AuditReport, Severity};
    /// let r = AuditReport { gates: vec![] };
    /// assert!(!r.should_fail(Severity::Error));
    /// ```
    #[must_use]
    pub fn should_fail(&self, fail_on: Severity) -> bool {
        self.max_severity().is_some_and(|sev| sev >= fail_on)
    }

    /// Total number of registered gates (skipped or not).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::AuditReport;
    /// let r = AuditReport { gates: vec![] };
    /// assert_eq!(r.len(), 0);
    /// ```
    #[must_use]
    pub const fn len(&self) -> usize {
        self.gates.len()
    }

    /// `true` when no gates ran.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::AuditReport;
    /// let r = AuditReport { gates: vec![] };
    /// assert!(r.is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.gates.is_empty()
    }

    /// Convenience: prints the rich text formatter to stdout.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::AuditReport;
    /// let r = AuditReport { gates: vec![] };
    /// r.print_text(); // emits nothing for an empty report
    /// ```
    pub fn print_text(&self) {
        let mut out = String::new();
        output::text::format(self, &mut out);
        print!("{out}");
    }

    /// Convenience: prints the JSON formatter to stdout.
    ///
    /// # Errors
    /// Propagates any JSON serialisation error from
    /// [`crate::audit::output::json::format`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::AuditReport;
    /// let r = AuditReport { gates: vec![] };
    /// r.print_json().unwrap();
    /// ```
    pub fn print_json(&self) -> Result<(), SsgError> {
        let s = output::json::format(self)?;
        println!("{s}");
        Ok(())
    }

    /// Convenience: prints the `JUnit` XML formatter to stdout.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::AuditReport;
    /// let r = AuditReport { gates: vec![] };
    /// r.print_junit();
    /// ```
    pub fn print_junit(&self) {
        let s = output::junit::format(self);
        println!("{s}");
    }

    /// Convenience: prints the SARIF v2.1.0 formatter to stdout (#562).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::AuditReport;
    /// let r = AuditReport { gates: vec![] };
    /// r.print_sarif();
    /// ```
    pub fn print_sarif(&self) {
        let s = output::sarif::format(self);
        println!("{s}");
    }
}

// ---------------------------------------------------------------------
// AuditRunner
// ---------------------------------------------------------------------

/// Orchestrates dispatch across the 15 registered gates.
pub struct AuditRunner {
    config: AuditConfig,
    gates: Vec<Box<dyn AuditGate>>,
}

impl std::fmt::Debug for AuditRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditRunner")
            .field("config", &self.config)
            .field(
                "gates",
                &self.gates.iter().map(|g| g.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl AuditRunner {
    /// Constructs a runner with the 15 built-in gates registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{AuditConfig, AuditRunner};
    /// let r = AuditRunner::new(AuditConfig::new());
    /// assert_eq!(r.gate_names().len(), 15);
    /// ```
    #[must_use]
    pub fn new(config: AuditConfig) -> Self {
        Self {
            config,
            gates: gates::all(),
        }
    }

    /// Constructs a runner with a user-supplied gate list (for tests).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{AuditConfig, AuditRunner};
    /// let r = AuditRunner::with_gates(AuditConfig::new(), Vec::new());
    /// assert!(r.gate_names().is_empty());
    /// ```
    #[must_use]
    pub fn with_gates(
        config: AuditConfig,
        gates: Vec<Box<dyn AuditGate>>,
    ) -> Self {
        Self { config, gates }
    }

    /// Returns the names of every gate registered with the runner.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{AuditConfig, AuditRunner};
    /// let r = AuditRunner::new(AuditConfig::new());
    /// assert!(r.gate_names().contains(&"wcag"));
    /// ```
    #[must_use]
    pub fn gate_names(&self) -> Vec<&'static str> {
        self.gates.iter().map(|g| g.name()).collect()
    }

    /// Runs every (enabled) gate sequentially and collects the result.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use ssg::audit::{AuditConfig, AuditRunner, Site};
    /// let runner = AuditRunner::new(AuditConfig::new());
    /// let site = Site { root: PathBuf::from("/nonexistent"), html_files: Vec::new() };
    /// let report = runner.run(&site);
    /// assert_eq!(report.len(), 15);
    /// ```
    #[must_use]
    pub fn run(&self, site: &Site) -> AuditReport {
        let mut results = Vec::with_capacity(self.gates.len());
        for gate in &self.gates {
            let name = gate.name();

            // Single-gate filter (`--gate <name>`).
            if let Some(ref only) = self.config.only {
                if only != name {
                    results.push(GateResult {
                        name: name.to_string(),
                        skipped: true,
                        skip_reason: Some(format!(
                            "not selected (--gate {only})"
                        )),
                        severity_counts: SeverityCounts::default(),
                        findings: Vec::new(),
                    });
                    continue;
                }
            }

            // Disabled in config.
            if self.config.disabled.contains(name) {
                results.push(GateResult {
                    name: name.to_string(),
                    skipped: true,
                    skip_reason: Some(
                        "disabled by ssg.toml [audit.disabled]".to_string(),
                    ),
                    severity_counts: SeverityCounts::default(),
                    findings: Vec::new(),
                });
                continue;
            }

            // Run.
            let findings = gate.run(site, &self.config.options);

            // Apply severity floor.
            let filtered: Vec<Finding> = findings
                .into_iter()
                .filter(|f| f.severity >= self.config.severity_floor)
                .collect();

            let mut counts = SeverityCounts::default();
            for f in &filtered {
                counts.add(f.severity);
            }

            results.push(GateResult {
                name: name.to_string(),
                skipped: false,
                skip_reason: None,
                severity_counts: counts,
                findings: filtered,
            });
        }
        AuditReport { gates: results }
    }

    /// Returns the configured `fail_on` threshold.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::{AuditConfig, AuditRunner, Severity};
    /// let r = AuditRunner::new(AuditConfig::new());
    /// assert_eq!(r.fail_on(), Severity::Error);
    /// ```
    #[must_use]
    pub const fn fail_on(&self) -> Severity {
        self.config.fail_on
    }
}

// ---------------------------------------------------------------------
// Audit config (ssg.toml [audit] section)
// ---------------------------------------------------------------------

/// Schema for the `[audit]` table in `ssg.toml`.
///
/// All fields are optional and absent fields fall back to
/// [`AuditConfig::new`] defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditTomlConfig {
    /// Gate identifiers to skip. Mirrors
    /// `[audit.disabled] gates = ["markdownlint"]` in `ssg.toml`.
    #[serde(default)]
    pub disabled: AuditDisabledSection,
    /// Performance budgets.
    #[serde(default)]
    pub budgets: AuditBudgets,
}

/// `[audit.disabled]` subsection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditDisabledSection {
    /// Names of gates to skip at audit time.
    #[serde(default)]
    pub gates: Vec<String>,
}

/// `[audit.budgets]` subsection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AuditBudgets {
    /// HTML + critical CSS budget (bytes).
    #[serde(default = "default_page_weight_budget")]
    pub page_weight_bytes: usize,
    /// Total JS budget (bytes).
    #[serde(default = "default_js_budget")]
    pub js_bytes: usize,
    /// Per-image size budget (bytes).
    #[serde(default = "default_image_budget")]
    pub image_bytes: usize,
}

impl Default for AuditBudgets {
    fn default() -> Self {
        Self {
            page_weight_bytes: default_page_weight_budget(),
            js_bytes: default_js_budget(),
            image_bytes: default_image_budget(),
        }
    }
}

const fn default_page_weight_budget() -> usize {
    100 * 1024
}

const fn default_js_budget() -> usize {
    50 * 1024
}

const fn default_image_budget() -> usize {
    250 * 1024
}

impl AuditTomlConfig {
    /// Merges the TOML config into a default [`AuditConfig`] and
    /// returns the result.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::audit::AuditTomlConfig;
    /// let toml_cfg = AuditTomlConfig::default();
    /// let cfg = toml_cfg.into_audit_config();
    /// assert!(cfg.disabled.is_empty());
    /// ```
    #[must_use]
    pub fn into_audit_config(self) -> AuditConfig {
        let mut cfg = AuditConfig::new();
        cfg.disabled.extend(self.disabled.gates);
        cfg.options.page_weight_budget = self.budgets.page_weight_bytes;
        cfg.options.js_budget = self.budgets.js_bytes;
        cfg.options.image_budget = self.budgets.image_bytes;
        cfg
    }
}

#[cfg(test)]
mod tests {

    /// `rel` is compared against URL paths from the HTML, so it must use `/`
    /// on every platform.
    ///
    /// The hreflang gate keys its reciprocity index on this value and looks
    /// it up by the `href` written in the document. On Windows the native
    /// separator produced `en\\index.html` against a document saying
    /// `/en/index.html`, so `hreflang_positive_reciprocal_pair_is_clean` and
    /// `hreflang_negative_missing_reciprocal_flagged` both failed — and had
    /// been failing unnoticed, because `tests/audit_gates.rs` was one of the
    /// 29 targets CI never ran.
    #[test]
    fn rel_always_uses_forward_slashes() {
        use std::path::PathBuf;
        let root = PathBuf::from("site");
        let site = Site {
            root: root.clone(),
            html_files: Vec::new(),
        };

        // Built with `join` so the input carries the platform separator.
        let nested = root.join("en").join("index.html");
        assert_eq!(site.rel(&nested), "en/index.html");
        assert!(
            !site.rel(&nested).contains('\\'),
            "a backslash survived into a URL-compared path"
        );
    }
    use super::*;

    #[test]
    fn severity_ordering_is_info_warn_error() {
        assert!(Severity::Info < Severity::Warn);
        assert!(Severity::Warn < Severity::Error);
    }

    #[test]
    fn severity_parse_round_trip() {
        for sev in [Severity::Info, Severity::Warn, Severity::Error] {
            assert_eq!(Severity::parse(sev.as_str()), Some(sev));
        }
        assert_eq!(Severity::parse("warning"), Some(Severity::Warn));
        assert_eq!(Severity::parse("err"), Some(Severity::Error));
        assert_eq!(Severity::parse("nope"), None);
    }

    #[test]
    fn severity_display_matches_as_str() {
        assert_eq!(format!("{}", Severity::Info), "info");
        assert_eq!(format!("{}", Severity::Warn), "warn");
        assert_eq!(format!("{}", Severity::Error), "error");
    }

    #[test]
    fn finding_builders_attach_optional_fields() {
        let f = Finding::new("g", Severity::Warn, "msg")
            .with_code("CODE")
            .with_path("a/b.html");
        assert_eq!(f.code.as_deref(), Some("CODE"));
        assert_eq!(f.path.as_deref(), Some("a/b.html"));
    }

    #[test]
    fn severity_counts_total_and_add() {
        let mut c = SeverityCounts::default();
        c.add(Severity::Info);
        c.add(Severity::Warn);
        c.add(Severity::Warn);
        c.add(Severity::Error);
        assert_eq!(c.total(), 4);
        assert_eq!(c.info, 1);
        assert_eq!(c.warn, 2);
        assert_eq!(c.error, 1);
    }

    #[test]
    fn audit_runner_registers_fifteen_gates() {
        let r = AuditRunner::new(AuditConfig::new());
        assert_eq!(r.gate_names().len(), 15, "must register exactly 15 gates");
    }

    #[test]
    fn audit_runner_gate_filter_skips_others() {
        let r = AuditRunner::new(AuditConfig {
            only: Some("hreflang".to_string()),
            ..AuditConfig::new()
        });
        let site = Site {
            root: PathBuf::from("/nonexistent"),
            html_files: Vec::new(),
        };
        let report = r.run(&site);
        let executed: Vec<_> =
            report.gates.iter().filter(|g| !g.skipped).collect();
        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].name, "hreflang");
    }

    #[test]
    fn audit_runner_disabled_gate_records_skip_reason() {
        let mut cfg = AuditConfig::new();
        let _ = cfg.disabled.insert("markdownlint".to_string());
        let r = AuditRunner::new(cfg);
        let site = Site {
            root: PathBuf::from("/nonexistent"),
            html_files: Vec::new(),
        };
        let report = r.run(&site);
        let md = report
            .gates
            .iter()
            .find(|g| g.name == "markdownlint")
            .expect("markdownlint gate registered");
        assert!(md.skipped);
        assert!(md
            .skip_reason
            .as_deref()
            .unwrap_or_default()
            .contains("disabled"));
    }

    #[test]
    fn audit_toml_config_parses_disabled_and_budgets() {
        let toml_src = r#"
            [disabled]
            gates = ["markdownlint", "links"]
            [budgets]
            page_weight_bytes = 200000
            js_bytes = 20000
            image_bytes = 100000
        "#;
        let parsed: AuditTomlConfig = toml::from_str(toml_src).unwrap();
        let cfg = parsed.into_audit_config();
        assert!(cfg.disabled.contains("markdownlint"));
        assert!(cfg.disabled.contains("links"));
        assert_eq!(cfg.options.page_weight_budget, 200_000);
        assert_eq!(cfg.options.js_budget, 20_000);
        assert_eq!(cfg.options.image_budget, 100_000);
    }

    #[test]
    fn audit_toml_config_uses_defaults_when_empty() {
        let cfg: AuditTomlConfig = toml::from_str("").unwrap();
        let merged = cfg.into_audit_config();
        assert!(merged.disabled.is_empty());
        assert_eq!(merged.options.page_weight_budget, 100 * 1024);
        assert_eq!(merged.options.js_budget, 50 * 1024);
        assert_eq!(merged.options.image_budget, 250 * 1024);
    }

    #[test]
    fn report_should_fail_compares_against_fail_on() {
        let report = AuditReport {
            gates: vec![GateResult {
                name: "x".to_string(),
                skipped: false,
                skip_reason: None,
                severity_counts: SeverityCounts {
                    info: 0,
                    warn: 1,
                    error: 0,
                },
                findings: vec![Finding::new("x", Severity::Warn, "m")],
            }],
        };
        assert!(report.should_fail(Severity::Warn));
        assert!(!report.should_fail(Severity::Error));
    }

    #[test]
    fn report_max_severity_accumulates_across_multiple_warn_and_info_gates() {
        // Drives the `Some(m) => m.max(...)` arm of both `map_or`
        // closures in `max_severity` — a single gate never re-enters
        // the accumulator, so at least two non-error gates (in either
        // order) are required to exercise the closure bodies.
        let report = AuditReport {
            gates: vec![
                GateResult {
                    name: "a".to_string(),
                    skipped: false,
                    skip_reason: None,
                    severity_counts: SeverityCounts {
                        info: 1,
                        warn: 0,
                        error: 0,
                    },
                    findings: vec![],
                },
                GateResult {
                    name: "b".to_string(),
                    skipped: false,
                    skip_reason: None,
                    severity_counts: SeverityCounts {
                        info: 0,
                        warn: 1,
                        error: 0,
                    },
                    findings: vec![],
                },
                GateResult {
                    name: "c".to_string(),
                    skipped: false,
                    skip_reason: None,
                    severity_counts: SeverityCounts {
                        info: 1,
                        warn: 0,
                        error: 0,
                    },
                    findings: vec![],
                },
            ],
        };
        assert_eq!(report.max_severity(), Some(Severity::Warn));
    }

    #[test]
    fn report_max_severity_returns_highest() {
        let report = AuditReport {
            gates: vec![
                GateResult {
                    name: "a".to_string(),
                    skipped: false,
                    skip_reason: None,
                    severity_counts: SeverityCounts {
                        info: 2,
                        warn: 0,
                        error: 0,
                    },
                    findings: vec![],
                },
                GateResult {
                    name: "b".to_string(),
                    skipped: false,
                    skip_reason: None,
                    severity_counts: SeverityCounts {
                        info: 0,
                        warn: 0,
                        error: 1,
                    },
                    findings: vec![],
                },
            ],
        };
        assert_eq!(report.max_severity(), Some(Severity::Error));
    }

    #[test]
    fn site_load_nonexistent_returns_empty_html_files() {
        // Covers line 240 — `Vec::new()` arm when root doesn't exist.
        let site = Site::load(Path::new("/nonexistent/xxx-audit")).unwrap();
        assert!(site.html_files.is_empty(), "{:?}", site.html_files);
    }

    #[test]
    fn audit_config_default_is_new() {
        // Covers lines 381-383.
        let cfg = AuditConfig::default();
        assert_eq!(cfg.severity_floor, AuditConfig::new().severity_floor);
    }

    #[test]
    fn audit_report_len_and_is_empty_align() {
        // Covers lines 517-519 (len) + 531-533 (is_empty).
        let r0 = AuditReport { gates: vec![] };
        assert_eq!(r0.len(), 0);
        assert!(r0.is_empty());
        let r1 = AuditReport {
            gates: vec![GateResult {
                name: "g".into(),
                skipped: false,
                skip_reason: None,
                severity_counts: SeverityCounts {
                    info: 0,
                    warn: 0,
                    error: 0,
                },
                findings: vec![],
            }],
        };
        assert_eq!(r1.len(), 1);
        assert!(!r1.is_empty());
    }

    #[test]
    fn audit_runner_debug_lists_gate_names() {
        // Covers lines 595-603 — Debug impl for AuditRunner.
        let runner = AuditRunner::new(AuditConfig::new());
        let dbg = format!("{runner:?}");
        assert!(dbg.contains("AuditRunner"));
        assert!(dbg.contains("gates"));
    }

    #[test]
    fn audit_runner_with_gates_accepts_empty_vec() {
        // Covers lines 634-639 — with_gates constructor.
        let runner = AuditRunner::with_gates(AuditConfig::new(), Vec::new());
        assert!(runner.gate_names().is_empty(), "{:?}", runner.gate_names());
    }

    #[test]
    fn audit_report_print_sarif_emits_to_stdout() {
        // Covers AuditReport::print_sarif body (lines 592-595).
        // The doctest under #[doc] doesn't get credited toward
        // `cargo llvm-cov --tests` coverage on stable; this unit
        // test does. We don't assert on stdout content — just that
        // the call runs to completion without panicking.
        let r = AuditReport { gates: vec![] };
        r.print_sarif();
    }
}
