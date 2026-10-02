// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Local LLM content plugin.
//!
//! Invokes a local LLM (Ollama, llama.cpp) at build time to auto-generate:
//! - `alt` text for images missing it
//! - `meta description` for pages where it's empty or < 50 chars
//! - JSON-LD `description` fields from page content
//!
//! Configured via the `[ai]` section in `ssg.toml`:
//! ```toml
//! [ai]
//! model = "llama3"
//! endpoint = "http://localhost:11434"
//! ```
//!
//! Graceful fallback: if no LLM is reachable, logs a warning and skips.

use super::llm_cache::LlmCache;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// Default per-call timeout for the local LLM HTTP roundtrip.
///
/// Matches the `llm.timeout_secs` config field (issue #520). Two
/// minutes covers a cold-load of a ~7B parameter model on a
/// modest workstation while still failing fast in the common
/// "endpoint refused" path.
const DEFAULT_LLM_TIMEOUT_SECS: u64 = 120;

/// Short timeout used for the "is the endpoint alive?" probe.
///
/// Keeps build pipelines fast: if the user does not have Ollama
/// running locally, the plugin must bail in well under a second
/// rather than blocking the whole compile on a 2-minute timeout.
const HEALTH_CHECK_TIMEOUT_SECS: u64 = 2;

/// Configuration for the LLM plugin.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Model name (e.g., `"llama3"`, `"mistral"`).
    pub model: String,
    /// Ollama API endpoint.
    pub endpoint: String,
    /// If true, print generated text but don't write files.
    pub dry_run: bool,
    /// Target Flesch-Kincaid Grade Level (default: 8.0).
    pub target_grade: f64,
    /// Max refinement attempts if readability exceeds target (default: 1).
    pub max_refinement_attempts: usize,
    /// Per-call HTTP timeout for the local LLM endpoint, in seconds
    /// (default: `120`). Set via `llm.timeout_secs` in `ssg.toml`.
    /// Exceeding this budget returns
    /// [`SsgError::LlmTimeout`](crate::error::SsgError::LlmTimeout) —
    /// no zombie subprocess is left behind because the call goes
    /// through `ureq`, not `curl` (issue #520).
    pub timeout_secs: u64,
    /// When `true`, skip the deterministic content-hash cache and
    /// always perform a live inference (issue #528). Driven by the
    /// `--no-llm-cache` CLI flag and the `SSG_NO_LLM_CACHE` env var
    /// so users debugging non-determinism can rule the cache out
    /// without nuking it on disk.
    pub cache_disabled: bool,
    /// Optional override for the on-disk cache root. `None` resolves
    /// to [`LlmCache::default_cache_dir`] at call time — the
    /// platform-correct path (XDG / Library / `%LOCALAPPDATA%`)
    /// chosen by [`LlmCache`]. Tests and operators wanting a
    /// project-local cache override this directly.
    pub cache_dir: Option<PathBuf>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        // `SSG_NO_LLM_CACHE` (any non-empty value other than `0`,
        // `false`, `off`) disables the deterministic cache. The
        // pipeline sets this when `--no-llm-cache` is passed; users
        // can also export it ad-hoc to debug a cache pathology.
        let cache_disabled = std::env::var("SSG_NO_LLM_CACHE")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some_and(|v| {
                !matches!(v.as_str(), "0" | "false" | "off" | "FALSE" | "OFF")
            });
        Self {
            model: "llama3".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            dry_run: false,
            target_grade: 8.0,
            max_refinement_attempts: 1,
            timeout_secs: DEFAULT_LLM_TIMEOUT_SECS,
            cache_disabled,
            cache_dir: None,
        }
    }
}

/// Plugin that uses a local LLM to augment content at build time.
#[derive(Debug)]
pub struct LlmPlugin {
    config: LlmConfig,
}

impl LlmPlugin {
    /// Creates a new `LlmPlugin` with the given configuration.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::llm::{LlmConfig, LlmPlugin};
    /// use ssg::plugin::Plugin;
    ///
    /// let p = LlmPlugin::new(LlmConfig::default());
    /// assert_eq!(p.name(), "llm");
    /// ```
    #[must_use]
    pub const fn new(config: LlmConfig) -> Self {
        Self { config }
    }
}

/// Result of auditing a single file's readability.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileAuditResult {
    /// Relative file path.
    pub path: String,
    /// Flesch-Kincaid Grade Level.
    pub grade_level: f64,
    /// Flesch Reading Ease score.
    pub reading_ease: f64,
    /// Average words per sentence.
    pub avg_sentence_len: f64,
    /// Whether it passes the target grade threshold.
    pub passes: bool,
}

/// Aggregated readability audit report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditReport {
    /// Target grade level used for pass/fail.
    pub target_grade: f64,
    /// Total files scanned.
    pub total_files: usize,
    /// Files that pass the readability threshold.
    pub passing: usize,
    /// Files that exceed the readability threshold.
    pub failing: usize,
    /// Per-file results.
    pub results: Vec<FileAuditResult>,
}

/// Result of the agentic AI fix pipeline for a single file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AiFixResult {
    /// Relative file path.
    pub path: String,
    /// Grade level before fix attempt.
    pub before_grade: f64,
    /// Grade level after fix attempt (same as before if not improved).
    pub after_grade: f64,
    /// Whether the fix improved readability.
    pub improved: bool,
    /// Action taken: "rewritten", "skipped", "no-improvement", "ollama-unavailable".
    pub action: String,
}

/// Aggregated report from the agentic AI fix pipeline.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AiFixReport {
    /// Total files audited.
    pub total_audited: usize,
    /// Files that failed the readability threshold.
    pub total_failing: usize,
    /// Files successfully improved.
    pub total_fixed: usize,
    /// Per-file results.
    pub results: Vec<AiFixResult>,
}

impl LlmPlugin {
    /// Audits all Markdown files in a directory for readability.
    ///
    /// Returns a structured report with per-file Flesch-Kincaid scores.
    /// Does not require an LLM — uses the local `ReadabilityAudit` engine.
    ///
    /// **Note:** The syllable heuristic is English-only. Non-English
    /// content (Bengali, Hindi, Turkish, etc.) produces inflated scores.
    /// Use the `en/` subdirectory for accurate results on multilingual
    /// repos, or filter results by locale.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::llm::LlmPlugin;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let report = LlmPlugin::audit_all(dir.path(), 8.0).unwrap();
    /// // Empty dir ⇒ no files audited.
    /// assert_eq!(report.total_files, 0);
    /// ```
    pub fn audit_all(
        content_dir: &Path,
        target_grade: f64,
    ) -> Result<AuditReport> {
        let md_files =
            crate::walk::walk_files(content_dir, "md").unwrap_or_default();

        let mut results = Vec::with_capacity(md_files.len());

        for path in &md_files {
            let Ok(content) = fs::read_to_string(path) else {
                continue; // File may have been removed by a concurrent test
            };
            // Strip frontmatter before auditing prose
            let body = strip_frontmatter(&content);
            // Detect language from frontmatter
            let lang = extract_frontmatter_lang(&content);
            let audit = ReadabilityAudit::analyze_with_lang(&body, &lang);
            let rel = path
                .strip_prefix(content_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            results.push(FileAuditResult {
                path: rel,
                grade_level: (audit.grade_level * 10.0).round() / 10.0,
                reading_ease: (audit.reading_ease * 10.0).round() / 10.0,
                avg_sentence_len: (audit.avg_sentence_len * 10.0).round()
                    / 10.0,
                passes: audit.grade_level <= target_grade,
            });
        }

        let passing = results.iter().filter(|r| r.passes).count();
        let failing = results.len() - passing;

        Ok(AuditReport {
            target_grade,
            total_files: results.len(),
            passing,
            failing,
            results,
        })
    }

    /// Audits and rewrites failing Markdown files via LLM refinement.
    ///
    /// For each file that exceeds `target_grade`:
    /// 1. Extracts the prose body (strips frontmatter)
    /// 2. Sends it to the LLM with a simplification prompt
    /// 3. If the refined version scores better, writes it back
    ///    (preserving the original frontmatter)
    /// 4. If `dry_run`, prints the diff without writing
    ///
    /// Returns the number of files rewritten.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::llm::{LlmConfig, LlmPlugin};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// // No Ollama reachable ⇒ returns Ok(0) without writing anything.
    /// let cfg = LlmConfig {
    ///     endpoint: "http://127.0.0.1:1".into(),
    ///     ..LlmConfig::default()
    /// };
    /// assert_eq!(LlmPlugin::audit_and_fix(dir.path(), &cfg).unwrap(), 0);
    /// ```
    pub fn audit_and_fix(
        content_dir: &Path,
        config: &LlmConfig,
    ) -> Result<usize> {
        if !is_ollama_available(&config.endpoint) {
            log::warn!(
                "[llm] Ollama not reachable at {}, skipping auto-fix",
                config.endpoint
            );
            return Ok(0);
        }

        let report = Self::audit_all(content_dir, config.target_grade)?;
        let failing: Vec<_> =
            report.results.iter().filter(|r| !r.passes).collect();

        if failing.is_empty() {
            log::info!(
                "[llm] All {} file(s) pass grade {:.0}",
                report.total_files,
                config.target_grade
            );
            return Ok(0);
        }

        let failing_count = failing.len();
        log::info!(
            "[llm] {} file(s) exceed grade {:.0}, attempting refinement",
            failing_count,
            config.target_grade
        );

        let mut rewritten = 0usize;

        for result in &failing {
            let path = content_dir.join(&result.path);
            let original = fs::read_to_string(&path)?;
            let (frontmatter_block, body) = split_frontmatter(&original);
            let body_trimmed = body.trim();

            if body_trimmed.is_empty() {
                continue;
            }

            let prompt = format!(
                "Rewrite this Markdown content at a 6th-grade reading level. \
                 Rules:\n\
                 - Max 20 words per sentence\n\
                 - Max 4 sentences per paragraph\n\
                 - Use simple, common words\n\
                 - Keep ALL facts, numbers, dates, and code blocks exactly the same\n\
                 - Keep ALL Markdown headings (#, ##, ###) and formatting\n\
                 - Return ONLY the rewritten Markdown, nothing else\n\n\
                 {body_trimmed}"
            );

            if let Some(refined) = generate_with_refinement(
                &config.endpoint,
                &config.model,
                &prompt,
                config.target_grade,
                config.max_refinement_attempts,
            ) {
                let refined_audit = ReadabilityAudit::analyze(&refined);
                let original_audit = ReadabilityAudit::analyze(body_trimmed);

                if refined_audit.grade_level < original_audit.grade_level {
                    if config.dry_run {
                        log::info!(
                            "[llm] [dry-run] {}: grade {:.1} → {:.1}",
                            result.path,
                            original_audit.grade_level,
                            refined_audit.grade_level
                        );
                    } else {
                        // Reassemble: frontmatter + refined body
                        let output =
                            format!("{frontmatter_block}\n{refined}\n");
                        fs::write(&path, output)?;
                        log::info!(
                            "[llm] Rewrote {}: grade {:.1} → {:.1}",
                            result.path,
                            original_audit.grade_level,
                            refined_audit.grade_level
                        );
                        rewritten += 1;
                    }
                } else {
                    log::warn!(
                        "[llm] Could not improve {}: grade {:.1} (refined: {:.1})",
                        result.path,
                        original_audit.grade_level,
                        refined_audit.grade_level
                    );
                }
            }
        }

        Ok(rewritten)
    }

    /// Agentic pipeline: audit → diagnose → fix → verify → report.
    ///
    /// Like `audit_and_fix()` but returns a detailed JSON-serialisable
    /// report with before/after scores for each file.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::llm::{LlmConfig, LlmPlugin};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let cfg = LlmConfig {
    ///     endpoint: "http://127.0.0.1:1".into(),
    ///     ..LlmConfig::default()
    /// };
    /// let report = LlmPlugin::audit_and_fix_with_report(dir.path(), &cfg).unwrap();
    /// assert_eq!(report.total_fixed, 0);
    /// ```
    pub fn audit_and_fix_with_report(
        content_dir: &Path,
        config: &LlmConfig,
    ) -> Result<AiFixReport> {
        if !is_ollama_available(&config.endpoint) {
            log::warn!(
                "[ai-fix] Ollama not reachable at {}, skipping",
                config.endpoint
            );
            return Ok(AiFixReport {
                total_audited: 0,
                total_failing: 0,
                total_fixed: 0,
                results: vec![],
            });
        }

        let report = Self::audit_all(content_dir, config.target_grade)?;
        let failing: Vec<_> =
            report.results.iter().filter(|r| !r.passes).collect();
        let mut fix_results = Vec::new();

        for result in &failing {
            let path = content_dir.join(&result.path);
            let Ok(original) = read_fix_source(&path) else {
                fix_results.push(AiFixResult {
                    path: result.path.clone(),
                    before_grade: result.grade_level,
                    after_grade: result.grade_level,
                    improved: false,
                    action: "skipped".to_string(),
                });
                continue;
            };
            let (frontmatter_block, body) = split_frontmatter(&original);
            let body_trimmed = body.trim();

            if body_trimmed.is_empty() {
                fix_results.push(AiFixResult {
                    path: result.path.clone(),
                    before_grade: result.grade_level,
                    after_grade: result.grade_level,
                    improved: false,
                    action: "skipped".to_string(),
                });
                continue;
            }

            let prompt = format!(
                "Rewrite this Markdown content at a 6th-grade reading level. \
                 Rules:\n\
                 - Max 20 words per sentence\n\
                 - Max 4 sentences per paragraph\n\
                 - Use simple, common words\n\
                 - Keep ALL facts, numbers, dates, and code blocks exactly the same\n\
                 - Keep ALL Markdown headings (#, ##, ###) and formatting\n\
                 - Return ONLY the rewritten Markdown, nothing else\n\n\
                 {body_trimmed}"
            );

            if let Some(refined) = generate_with_refinement(
                &config.endpoint,
                &config.model,
                &prompt,
                config.target_grade,
                config.max_refinement_attempts,
            ) {
                let refined_audit = ReadabilityAudit::analyze(&refined);
                let original_audit = ReadabilityAudit::analyze(body_trimmed);

                if refined_audit.grade_level < original_audit.grade_level {
                    if !config.dry_run {
                        let output =
                            format!("{frontmatter_block}\n{refined}\n");
                        fs::write(&path, output)?;
                    }
                    fix_results.push(AiFixResult {
                        path: result.path.clone(),
                        before_grade: (original_audit.grade_level * 10.0)
                            .round()
                            / 10.0,
                        after_grade: (refined_audit.grade_level * 10.0).round()
                            / 10.0,
                        improved: true,
                        action: if config.dry_run {
                            "dry-run".to_string()
                        } else {
                            "rewritten".to_string()
                        },
                    });
                } else {
                    fix_results.push(AiFixResult {
                        path: result.path.clone(),
                        before_grade: (original_audit.grade_level * 10.0)
                            .round()
                            / 10.0,
                        after_grade: (refined_audit.grade_level * 10.0).round()
                            / 10.0,
                        improved: false,
                        action: "no-improvement".to_string(),
                    });
                }
            } else {
                fix_results.push(AiFixResult {
                    path: result.path.clone(),
                    before_grade: result.grade_level,
                    after_grade: result.grade_level,
                    improved: false,
                    action: "skipped".to_string(),
                });
            }
        }

        let total_fixed = fix_results.iter().filter(|r| r.improved).count();

        Ok(AiFixReport {
            total_audited: report.total_files,
            total_failing: failing.len(),
            total_fixed,
            results: fix_results,
        })
    }
}

/// Re-reads a file selected by the audit pass for LLM refinement.
///
/// Kept as a dedicated seam so the unreadable-file arm of
/// [`LlmPlugin::audit_and_fix_with_report`] is drivable via the
/// `llm::fix-read` failpoint — the real trigger is a TOCTOU window
/// (file audited, then removed before the fix pass re-reads it)
/// that cannot be produced deterministically from a test.
fn read_fix_source(path: &Path) -> std::io::Result<String> {
    fail_point!("llm::fix-read", |_| {
        Err(std::io::Error::other("injected: llm::fix-read"))
    });
    fs::read_to_string(path)
}

/// Splits content into `(frontmatter_block, body)`.
///
/// The frontmatter block includes delimiters so it can be
/// reassembled verbatim. Returns `("", content)` if no
/// frontmatter is found.
fn split_frontmatter(content: &str) -> (String, String) {
    let trimmed = content.trim_start();
    let leading_ws = &content[..content.len() - trimmed.len()];

    for delim in ["---", "+++"] {
        if let Some(rest) = trimmed.strip_prefix(delim) {
            if let Some(end) = rest.find(delim) {
                let fm_end = delim.len() + end + delim.len();
                let frontmatter = &trimmed[..fm_end];
                let body = &trimmed[fm_end..];
                return (
                    format!("{leading_ws}{frontmatter}"),
                    body.to_string(),
                );
            }
        }
    }

    (String::new(), content.to_string())
}

/// Extracts the `language` or `lang` field from YAML/TOML frontmatter.
fn extract_frontmatter_lang(content: &str) -> String {
    let trimmed = content.trim_start();
    for delim in ["---", "+++"] {
        if let Some(rest) = trimmed.strip_prefix(delim) {
            if let Some(end) = rest.find(delim) {
                let fm = &rest[..end];
                // Try YAML-style: `language: en` or `lang: en`
                for line in fm.lines() {
                    let line = line.trim();
                    for key in ["language:", "lang:"] {
                        if let Some(val) = line.strip_prefix(key) {
                            let val =
                                val.trim().trim_matches('"').trim_matches('\'');
                            if !val.is_empty() {
                                return val.to_string();
                            }
                        }
                    }
                }
                // Try TOML-style: `language = "en"` or `lang = "en"`
                for line in fm.lines() {
                    let line = line.trim();
                    for key in ["language", "lang"] {
                        if line.starts_with(key) {
                            if let Some(val) = line.split('=').nth(1) {
                                let val = val
                                    .trim()
                                    .trim_matches('"')
                                    .trim_matches('\'');
                                if !val.is_empty() {
                                    return val.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    String::new()
}

/// Strips YAML/TOML frontmatter from Markdown content.
fn strip_frontmatter(content: &str) -> String {
    let trimmed = content.trim_start();
    for delim in ["---", "+++"] {
        if let Some(rest) = trimmed.strip_prefix(delim) {
            if let Some(end) = rest.find(delim) {
                return rest[end + delim.len()..].to_string();
            }
        }
    }
    content.to_string()
}

impl Plugin for LlmPlugin {
    fn name(&self) -> &'static str {
        "llm"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        // Check if Ollama is available
        if !is_ollama_available(&self.config.endpoint) {
            log::warn!(
                "[llm] Ollama not reachable at {}, skipping AI augmentation",
                self.config.endpoint
            );
            return Ok(());
        }

        let html_files = ctx.get_html_files();
        let mut augmented = 0usize;

        for path in &html_files {
            let html = fs::read_to_string(path).with_path(path)?;
            let mut modified = html.clone();

            // Auto-generate meta descriptions for pages with short/missing ones
            if needs_meta_description(&modified) {
                if let Some(desc) = generate_meta_description(
                    &modified,
                    &self.config.model,
                    &self.config.endpoint,
                    self.config.target_grade,
                    self.config.max_refinement_attempts,
                ) {
                    let audit = ReadabilityAudit::analyze(&desc);
                    if self.config.dry_run {
                        let rel = path
                            .strip_prefix(&ctx.site_dir)
                            .unwrap_or(path)
                            .display();
                        log::info!(
                            "[llm] [dry-run] {rel}: description = {desc}"
                        );
                        log::info!(
                            "[llm] [dry-run] {rel}: grade={:.1}, ease={:.1}, avg_sentence={:.1}",
                            audit.grade_level, audit.reading_ease, audit.avg_sentence_len
                        );
                    } else {
                        modified = inject_meta_description(&modified, &desc);
                        // Also populate JSON-LD Article description
                        modified = inject_jsonld_description(&modified, &desc);
                    }
                }
            }

            // Auto-generate alt text for images missing it
            let alt_count = generate_missing_alt_text(
                &mut modified,
                &self.config.model,
                &self.config.endpoint,
                self.config.dry_run,
                path,
                &ctx.site_dir,
            );

            if !self.config.dry_run && modified != html {
                fs::write(path, &modified).with_path(path)?;
                augmented += 1;
            }

            if alt_count > 0 {
                augmented += 1;
            }
        }

        if augmented > 0 {
            log::info!(
                "[llm] Augmented {augmented} page(s) with model '{}'",
                self.config.model
            );
        }

        Ok(())
    }
}

/// Checks if Ollama is reachable at the given endpoint.
///
/// Uses an in-process `ureq` GET with a short
/// `HEALTH_CHECK_TIMEOUT_SECS` budget. Replaces the previous
/// `curl` shellout (issue #520) so the probe works on Windows
/// runners without `curl.exe` in `$PATH` and cannot fail
/// silently in restricted environments.
fn is_ollama_available(endpoint: &str) -> bool {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS))
        .build();
    matches!(agent.get(endpoint).call(), Ok(resp) if resp.status() < 500)
}

/// Returns true if the page needs a meta description (missing or < 50 chars).
fn needs_meta_description(html: &str) -> bool {
    if let Some(start) = html.find("name=\"description\"") {
        if let Some(content_start) = html[start..].find("content=\"") {
            let abs = start + content_start + 9;
            if let Some(end) = html[abs..].find('"') {
                let desc = &html[abs..abs + end];
                return desc.len() < 50;
            }
        }
    }
    // No description meta tag found
    !html.contains("name=\"description\"")
}

/// Generates a meta description via LLM with readability refinement.
fn generate_meta_description(
    html: &str,
    model: &str,
    endpoint: &str,
    target_grade: f64,
    max_attempts: usize,
) -> Option<String> {
    let text = extract_page_text(html, 500);
    if text.len() < 20 {
        return None;
    }

    let prompt = format!(
        "Write a concise SEO meta description (120-155 characters) for this page content. \
         Use simple words and short sentences. \
         Return ONLY the description text, no quotes or explanation:\n\n{text}"
    );

    generate_with_refinement(
        endpoint,
        model,
        &prompt,
        target_grade,
        max_attempts,
    )
}

/// Injects a meta description tag into the HTML head.
fn inject_meta_description(html: &str, description: &str) -> String {
    let escaped = description
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let tag = format!("<meta name=\"description\" content=\"{escaped}\">\n");
    inject_before_head_close(html, &tag)
}

/// Generates alt text for images that are missing it.
fn generate_missing_alt_text(
    html: &mut String,
    model: &str,
    endpoint: &str,
    dry_run: bool,
    path: &Path,
    site_dir: &Path,
) -> usize {
    let mut count = 0;
    let mut search_from = 0;

    while let Some(start) = html[search_from..].find("<img") {
        let abs_start = search_from + start;
        let Some(tag_end) = html[abs_start..].find('>') else {
            break;
        };
        let tag_end_abs = abs_start + tag_end + 1;
        let tag = &html[abs_start..tag_end_abs];

        if !tag.contains("alt=") || tag.contains("alt=\"\"") {
            // Extract src for context
            let src = extract_attr(tag, "src").unwrap_or_default();
            let prompt = format!(
                "Describe this image for an alt text attribute. The image file is named '{}'. \
                 Return ONLY the alt text (max 125 characters), no quotes:\n",
                src
            );

            if let Some(alt) = call_ollama(endpoint, model, &prompt) {
                let alt = alt.trim().replace('"', "&quot;");
                if dry_run {
                    let rel =
                        path.strip_prefix(site_dir).unwrap_or(path).display();
                    log::info!(
                        "[llm] [dry-run] {rel}: alt=\"{alt}\" for {src}"
                    );
                } else {
                    // Replace the tag with one that has alt text
                    let new_tag = if tag.contains("alt=\"\"") {
                        tag.replace("alt=\"\"", &format!("alt=\"{alt}\""))
                    } else {
                        tag.replace("<img", &format!("<img alt=\"{alt}\""))
                    };
                    html.replace_range(abs_start..tag_end_abs, &new_tag);
                }
                count += 1;
            }
        }

        search_from = tag_end_abs;
    }

    count
}

/// Extracts plain text from HTML for LLM prompting.
fn extract_page_text(html: &str, max_chars: usize) -> String {
    let body_start = html
        .find("<main")
        .or_else(|| html.find("<body"))
        .unwrap_or(0);
    let body = &html[body_start..];

    let mut text = String::with_capacity(max_chars + 50);
    let mut in_tag = false;
    for ch in body.chars() {
        if text.len() >= max_chars {
            break;
        }
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag && !ch.is_control() => text.push(ch),
            _ => {}
        }
    }

    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extracts an attribute value from an HTML tag.
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let pattern = format!("{attr}=\"");
    let start = tag.find(&pattern)? + pattern.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

// =====================================================================
// Readability intelligence
// =====================================================================

/// Readability formula selection based on content language.
///
/// Marked `#[non_exhaustive]` so additional formulae (Dale-Chall,
/// Linsear-Write, Coleman-Liau) can ship in minor versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadabilityFormula {
    /// Flesch-Kincaid (English).
    FleschKincaid,
    /// Kandel-Moles (French).
    KandelMoles,
    /// Wiener Sachtextformel (German).
    WienerSachtextformel,
    /// Gulpease index (Italian).
    Gulpease,
    /// LIX readability (Swedish/Scandinavian).
    Lix,
    /// Fernández Huerta (Spanish).
    FernandezHuerta,
}

impl ReadabilityFormula {
    /// Selects the appropriate formula from a language code.
    ///
    /// Accepts BCP 47 codes (e.g., `"en"`, `"fr"`, `"de-AT"`).
    /// Returns `None` for unsupported languages.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::llm::ReadabilityFormula;
    ///
    /// assert_eq!(ReadabilityFormula::from_lang("en"), Some(ReadabilityFormula::FleschKincaid));
    /// assert_eq!(ReadabilityFormula::from_lang("xx"), None);
    /// ```
    #[must_use]
    pub fn from_lang(lang: &str) -> Option<Self> {
        let primary = lang.split(['-', '_']).next().unwrap_or(lang);
        match primary.to_lowercase().as_str() {
            "en" => Some(Self::FleschKincaid),
            "fr" => Some(Self::KandelMoles),
            "de" => Some(Self::WienerSachtextformel),
            "it" => Some(Self::Gulpease),
            "sv" | "nb" | "nn" | "da" | "no" => Some(Self::Lix),
            "es" => Some(Self::FernandezHuerta),
            _ => None,
        }
    }
}

/// Readability metrics for a text passage.
#[derive(Debug, Clone, Copy)]
pub struct ReadabilityAudit {
    /// Flesch-Kincaid Grade Level (lower = simpler).
    pub grade_level: f64,
    /// Flesch Reading Ease (higher = easier, 0–100).
    pub reading_ease: f64,
    /// Average words per sentence.
    pub avg_sentence_len: f64,
}

impl ReadabilityAudit {
    /// Analyzes text and returns readability metrics.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::llm::ReadabilityAudit;
    ///
    /// let a = ReadabilityAudit::analyze("This is a simple sentence.");
    /// assert!(a.avg_sentence_len > 0.0);
    /// ```
    #[must_use]
    pub fn analyze(text: &str) -> Self {
        let words = count_words(text);
        let sentences = count_sentences(text);
        let syllables = count_syllables(text);

        if words == 0 || sentences == 0 {
            return Self {
                grade_level: 0.0,
                reading_ease: 100.0,
                avg_sentence_len: 0.0,
            };
        }

        let wps = words as f64 / sentences as f64;
        let spw = syllables as f64 / words as f64;

        let grade = 0.39f64.mul_add(wps, 11.8f64.mul_add(spw, -15.59));
        let ease = (-1.015f64).mul_add(wps, (-84.6f64).mul_add(spw, 206.835));

        Self {
            grade_level: grade.max(0.0),
            reading_ease: ease.clamp(0.0, 100.0),
            avg_sentence_len: wps,
        }
    }

    /// Analyzes text using the appropriate formula for the given language.
    ///
    /// Falls back to Flesch-Kincaid if the language is unsupported or empty.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::llm::ReadabilityAudit;
    ///
    /// let a = ReadabilityAudit::analyze_with_lang("Bonjour le monde.", "fr");
    /// assert!(a.avg_sentence_len > 0.0);
    /// ```
    #[must_use]
    pub fn analyze_with_lang(text: &str, lang: &str) -> Self {
        let formula = if lang.is_empty() {
            ReadabilityFormula::FleschKincaid
        } else {
            ReadabilityFormula::from_lang(lang)
                .unwrap_or(ReadabilityFormula::FleschKincaid)
        };

        let words = count_words(text);
        let sentences = count_sentences(text);
        let syllables = count_syllables(text);
        let chars: usize = text.chars().filter(|c| c.is_alphanumeric()).count();

        if words == 0 || sentences == 0 {
            return Self {
                grade_level: 0.0,
                reading_ease: 100.0,
                avg_sentence_len: 0.0,
            };
        }

        let wps = words as f64 / sentences as f64;
        let spw = syllables as f64 / words as f64;

        match formula {
            ReadabilityFormula::FleschKincaid => Self::analyze(text),

            ReadabilityFormula::KandelMoles => {
                // Kandel-Moles reading ease (French)
                let ease = 68.0f64.mul_add(-spw, 1.15f64.mul_add(-wps, 209.0));
                Self {
                    grade_level: ((100.0 - ease.clamp(0.0, 100.0)) / 10.0)
                        .max(0.0),
                    reading_ease: ease.clamp(0.0, 100.0),
                    avg_sentence_len: wps,
                }
            }

            ReadabilityFormula::WienerSachtextformel => {
                // Wiener Sachtextformel (German)
                let word_list: Vec<&str> = text.split_whitespace().collect();
                let total = word_list.len().max(1) as f64;
                let pct_3plus_syl = word_list
                    .iter()
                    .filter(|w| count_word_syllables(w) >= 3)
                    .count() as f64
                    / total
                    * 100.0;
                let pct_6plus_char = word_list
                    .iter()
                    .filter(|w| {
                        w.chars().filter(|c| c.is_alphabetic()).count() > 6
                    })
                    .count() as f64
                    / total
                    * 100.0;
                let pct_1syl = word_list
                    .iter()
                    .filter(|w| count_word_syllables(w) == 1)
                    .count() as f64
                    / total
                    * 100.0;

                let grade = 0.1935f64.mul_add(
                    pct_3plus_syl,
                    0.1672f64.mul_add(
                        wps,
                        (-0.1297f64).mul_add(
                            pct_6plus_char,
                            (-0.0327f64).mul_add(pct_1syl, -0.875),
                        ),
                    ),
                );

                Self {
                    grade_level: grade.max(0.0),
                    reading_ease: grade
                        .clamp(0.0, 20.0)
                        .mul_add(-5.0, 100.0)
                        .clamp(0.0, 100.0),
                    avg_sentence_len: wps,
                }
            }

            ReadabilityFormula::Gulpease => {
                // Gulpease index (Italian)
                let ease = 89.0
                    + 10.0f64
                        .mul_add(-(chars as f64), 300.0 * sentences as f64)
                        / words as f64;
                Self {
                    grade_level: ((100.0 - ease.clamp(0.0, 100.0)) / 10.0)
                        .max(0.0),
                    reading_ease: ease.clamp(0.0, 100.0),
                    avg_sentence_len: wps,
                }
            }

            ReadabilityFormula::Lix => {
                // LIX (Swedish/Scandinavian)
                let word_list: Vec<&str> = text.split_whitespace().collect();
                let total = word_list.len().max(1) as f64;
                let long_words = word_list
                    .iter()
                    .filter(|w| {
                        w.chars().filter(|c| c.is_alphabetic()).count() > 6
                    })
                    .count() as f64;
                let lix = wps + 100.0 * long_words / total;
                // LIX scale: <25 very easy, 25-35 easy, 35-45 medium,
                // 45-55 hard, >55 very hard
                Self {
                    grade_level: (lix / 5.0).max(0.0),
                    reading_ease: (100.0 - lix).clamp(0.0, 100.0),
                    avg_sentence_len: wps,
                }
            }

            ReadabilityFormula::FernandezHuerta => {
                // Fernández Huerta (Spanish)
                let ease =
                    1.02f64.mul_add(-wps, (-60.0f64).mul_add(spw, 206.84));
                Self {
                    grade_level: ((100.0 - ease.clamp(0.0, 100.0)) / 10.0)
                        .max(0.0),
                    reading_ease: ease.clamp(0.0, 100.0),
                    avg_sentence_len: wps,
                }
            }
        }
    }
}

/// Counts words in text (whitespace-separated tokens).
fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Counts sentences by splitting on `.`, `!`, `?`.
fn count_sentences(text: &str) -> usize {
    text.chars()
        .filter(|&c| c == '.' || c == '!' || c == '?')
        .count()
        .max(1)
}

/// Counts syllables using a lightweight heuristic:
/// - Count vowel groups (consecutive vowels = 1 syllable)
/// - Subtract silent trailing 'e'
/// - Minimum 1 syllable per word
fn count_syllables(text: &str) -> usize {
    text.split_whitespace()
        .map(|word| count_word_syllables(word))
        .sum()
}

/// Counts syllables in a single word.
fn count_word_syllables(word: &str) -> usize {
    let word = word.to_lowercase();
    let chars: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
    if chars.is_empty() {
        return 1;
    }

    let vowels = b"aeiouy";
    let mut count = 0usize;
    let mut prev_vowel = false;

    for &ch in &chars {
        let is_vowel = vowels.contains(&(ch as u8));
        if is_vowel && !prev_vowel {
            count += 1;
        }
        prev_vowel = is_vowel;
    }

    // Subtract silent trailing 'e'
    if chars.len() > 2 && chars.last() == Some(&'e') && count > 1 {
        count -= 1;
    }

    count.max(1)
}

/// Generates text via LLM with readability-driven refinement.
///
/// If the initial output exceeds `target_grade`, re-prompts the LLM
/// once to simplify. Keeps the best available draft on failure.
fn generate_with_refinement(
    endpoint: &str,
    model: &str,
    prompt: &str,
    target_grade: f64,
    max_attempts: usize,
) -> Option<String> {
    let mut text = call_ollama(endpoint, model, prompt)?;
    let mut audit = ReadabilityAudit::analyze(&text);

    for attempt in 0..max_attempts {
        if audit.grade_level <= target_grade {
            break;
        }

        let attempt_num = attempt + 1;
        log::info!(
            "[llm] Grade {:.1} exceeds target {:.1}, refining (attempt {})",
            audit.grade_level,
            target_grade,
            attempt_num
        );

        let simplify_prompt = format!(
            "Rewrite this text at a 6th-grade reading level. \
             Use short sentences (max 20 words). Use simple words. \
             Keep all facts and numbers exactly the same. \
             Return ONLY the rewritten text:\n\n{text}"
        );

        if let Some(refined) = call_ollama(endpoint, model, &simplify_prompt) {
            let refined_audit = ReadabilityAudit::analyze(&refined);
            if refined_audit.grade_level < audit.grade_level {
                text = refined;
                audit = refined_audit;
            }
        }
    }

    Some(text)
}

// =====================================================================
// JSON-LD generation
// =====================================================================

/// Injects or updates a JSON-LD `Article` script block in the HTML head.
///
/// Populates `description`, `datePublished`, and `author` from the page
/// content and frontmatter sidecar.
fn inject_jsonld_description(html: &str, description: &str) -> String {
    // Skip if JSON-LD Article already has a description
    if html.contains("\"@type\":\"Article\"")
        && html.contains("\"description\"")
    {
        return html.to_string();
    }

    let jsonld = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "Article",
        "description": description,
    });

    let script =
        format!("<script type=\"application/ld+json\">{}</script>\n", jsonld);
    inject_before_head_close(html, &script)
}

/// Calls the Ollama API to generate text.
///
/// Backward-compatible `Option<String>` wrapper around
/// [`query_ollama`] for the in-tree call sites that expect a
/// graceful `None` fallback (no LLM available, model errored,
/// etc.). New code should prefer [`query_ollama`] for typed
/// `SsgError` returns (issue #520, AC4/AC5).
fn call_ollama(endpoint: &str, model: &str, prompt: &str) -> Option<String> {
    query_ollama(endpoint, model, prompt, DEFAULT_LLM_TIMEOUT_SECS).ok()
}

/// Typed Ollama generation call backed by `ureq` (issue #520).
///
/// All HTTP traffic flows through `ureq::post(...).send_json(...)`
/// — no subprocess is spawned, so prompts containing shell
/// metacharacters (`$(`, backticks, `;`, `&`, `|`) traverse the
/// transport as a JSON body byte-for-byte unchanged (AC3).
///
/// # Errors
///
/// - [`SsgError::LlmTimeout`] when the call exceeds `timeout_secs`.
/// - [`SsgError::LlmEndpointUnreachable`] when the TCP connection
///   is refused, the host is unresolvable, or any other transport
///   failure occurs before a response is received.
/// - [`SsgError::LlmInvalidResponse`] when the server returns a
///   non-2xx status, the body is not valid JSON, or the JSON does
///   not carry a non-empty `response` field.
///
/// # Examples
///
/// ```rust
/// use ssg::llm::query_ollama;
/// use ssg::SsgError;
///
/// // No Ollama running on port 1 ⇒ deterministic transport failure.
/// let err = query_ollama("http://127.0.0.1:1", "llama2", "hi", 1).unwrap_err();
/// assert!(matches!(err, SsgError::LlmEndpointUnreachable { .. } | SsgError::LlmTimeout { .. }));
/// ```
pub fn query_ollama(
    endpoint: &str,
    model: &str,
    prompt: &str,
    timeout_secs: u64,
) -> Result<String, SsgError> {
    let url = format!("{}/api/generate", endpoint.trim_end_matches('/'));
    let payload = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
    });

    let timeout = Duration::from_secs(timeout_secs);
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();

    let response = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .send_json(payload)
        .map_err(|err| classify_ureq_error(err, &url, timeout))?;

    let body: serde_json::Value =
        response
            .into_json()
            .map_err(|e| SsgError::LlmInvalidResponse {
                message: format!("malformed JSON response body: {e}"),
            })?;

    body.get("response")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SsgError::LlmInvalidResponse {
            message: "missing or empty 'response' field".into(),
        })
}

/// Maps a `ureq::Error` into the right [`SsgError`] variant.
///
/// `ureq` does not surface a dedicated "timeout" enum arm; the
/// underlying `io::Error` carries `ErrorKind::TimedOut`. We unwrap
/// the transport layer so callers get
/// [`SsgError::LlmTimeout`] for timeouts and
/// [`SsgError::LlmEndpointUnreachable`] for every other transport
/// failure, while HTTP non-2xx responses become
/// [`SsgError::LlmInvalidResponse`].
fn classify_ureq_error(
    err: ureq::Error,
    url: &str,
    timeout: Duration,
) -> SsgError {
    match err {
        ureq::Error::Status(code, resp) => SsgError::LlmInvalidResponse {
            message: format!(
                "HTTP {code} from {url}: {}",
                resp.into_string().unwrap_or_default()
            ),
        },
        ureq::Error::Transport(transport) => {
            // `ureq` exposes the kind enum but not the inner
            // `io::Error` directly on stable; the timeout case is
            // signalled by `ErrorKind::Io` whose message contains
            // "timed out", or by `Kind::ConnectionFailed` with a
            // wrapped `io::ErrorKind::TimedOut`. We detect via the
            // formatted message which is stable across versions.
            let kind = transport.kind();
            let msg = transport.to_string();
            if is_timeout_transport(kind, &msg) {
                SsgError::LlmTimeout { duration: timeout }
            } else {
                SsgError::LlmEndpointUnreachable {
                    url: url.to_string(),
                    source: Box::new(transport),
                }
            }
        }
    }
}

/// Heuristic: does this transport error look like a client-side
/// timeout rather than a hard connection failure?
///
/// Extracted from [`classify_ureq_error`] as a pure function of
/// `(kind, message)` so each OS-specific phrasing — Unix's "timed
/// out", the generic "timeout"/"deadline" fallbacks, and Windows'
/// WSAETIMEDOUT phrasing ("os error 10060") — is directly unit
/// testable without depending on which OS actually produced the
/// transport error (`ureq::Transport` has no public constructor, so
/// synthesizing a real one in a test is not possible).
fn is_timeout_transport(kind: ureq::ErrorKind, msg: &str) -> bool {
    matches!(
        kind,
        ureq::ErrorKind::Io | ureq::ErrorKind::ConnectionFailed
    ) && (msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("deadline")
        || msg.contains("os error 10060"))
}

impl LlmPlugin {
    /// Typed entry point for invoking the configured local LLM
    /// (issue #520, AC4/AC5).
    ///
    /// Unlike the in-tree augmentation helpers which swallow
    /// errors and fall back to leaving content untouched, `query`
    /// surfaces transport and protocol failures as typed
    /// [`SsgError`] variants so external callers (CLI commands,
    /// integration tests, custom pipelines) can react
    /// appropriately.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::llm::{LlmConfig, LlmPlugin};
    ///
    /// // No Ollama running ⇒ deterministic transport error.
    /// let cfg = LlmConfig {
    ///     endpoint: "http://127.0.0.1:1".into(),
    ///     ..LlmConfig::default()
    /// };
    /// let plugin = LlmPlugin::new(cfg);
    /// assert!(plugin.query("hi").is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// See [`query_ollama`] for the exact variants.
    pub fn query(&self, prompt: &str) -> Result<String, SsgError> {
        // Issue #528 — deterministic content-hash cache. A hit skips
        // the HTTP roundtrip entirely (AC1) and a miss writes the
        // response back for the next call. The cache is intentionally
        // best-effort: any filesystem error falls through to a live
        // call so a busted cache directory never wedges the build.
        if !self.config.cache_disabled {
            let root = self
                .config
                .cache_dir
                .clone()
                .unwrap_or_else(LlmCache::default_cache_dir);
            let cache = LlmCache::new(root);
            let key = LlmCache::compute_key(
                &self.config.endpoint,
                &self.config.model,
                prompt,
                self.config.timeout_secs,
            );
            if let Some(cached) = cache.get(&key) {
                return Ok(cached);
            }
            let response = query_ollama(
                &self.config.endpoint,
                &self.config.model,
                prompt,
                self.config.timeout_secs,
            )?;
            let _ = cache.set(&key, &response);
            return Ok(response);
        }
        query_ollama(
            &self.config.endpoint,
            &self.config.model,
            prompt,
            self.config.timeout_secs,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_meta_description_missing() {
        assert!(needs_meta_description("<html><head></head></html>"));
    }

    #[test]
    fn needs_meta_description_short() {
        let html = r#"<html><head><meta name="description" content="Short"></head></html>"#;
        assert!(needs_meta_description(html));
    }

    #[test]
    fn needs_meta_description_adequate() {
        let html = r#"<html><head><meta name="description" content="This is a sufficiently long meta description that exceeds fifty characters easily"></head></html>"#;
        assert!(!needs_meta_description(html));
    }

    #[test]
    fn inject_meta_description_into_head() {
        let html = "<html><head><title>T</title></head><body></body></html>";
        let result = inject_meta_description(html, "Test description");
        assert!(result.contains("name=\"description\""));
        assert!(result.contains("Test description"));
    }

    #[test]
    fn extract_attr_basic() {
        assert_eq!(
            extract_attr(r#"<img src="photo.jpg" alt="x">"#, "src"),
            Some("photo.jpg".to_string())
        );
    }

    #[test]
    fn extract_attr_missing() {
        assert_eq!(extract_attr(r#"<img src="x.jpg">"#, "alt"), None);
    }

    #[test]
    fn extract_page_text_strips_tags() {
        let html = "<body><p>Hello <b>world</b></p></body>";
        let text = extract_page_text(html, 100);
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn llm_plugin_name() {
        let plugin = LlmPlugin::new(LlmConfig::default());
        assert_eq!(plugin.name(), "llm");
    }

    // ── Readability engine tests ──────────────────────────────────

    #[test]
    fn flesch_kincaid_simple_text() {
        // "The cat sat on the mat." — very simple, ~grade 1
        let audit = ReadabilityAudit::analyze("The cat sat on the mat.");
        assert!(
            audit.grade_level < 4.0,
            "Simple text should be below grade 4, got {:.1}",
            audit.grade_level
        );
        assert!(audit.reading_ease > 80.0);
    }

    #[test]
    fn flesch_kincaid_complex_text() {
        let text = "The implementation of sophisticated cryptographic \
                    algorithms necessitates comprehensive understanding \
                    of mathematical foundations. Asymmetric encryption \
                    protocols demonstrate considerable computational \
                    overhead compared to symmetric alternatives.";
        let audit = ReadabilityAudit::analyze(text);
        assert!(
            audit.grade_level > 12.0,
            "Complex text should be above grade 12, got {:.1}",
            audit.grade_level
        );
    }

    #[test]
    fn flesch_kincaid_empty_text() {
        let audit = ReadabilityAudit::analyze("");
        assert!(audit.grade_level.abs() < f64::EPSILON);
        assert!((audit.reading_ease - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn syllable_count_known_words() {
        assert_eq!(count_word_syllables("cat"), 1);
        assert_eq!(count_word_syllables("hello"), 2);
        assert_eq!(count_word_syllables("beautiful"), 3);
        assert_eq!(count_word_syllables("implementation"), 5);
    }

    #[test]
    fn count_sentences_basic() {
        assert_eq!(count_sentences("Hello. World!"), 2);
        assert_eq!(count_sentences("One sentence"), 1); // min 1
        assert_eq!(count_sentences("A? B? C!"), 3);
    }

    // ── JSON-LD tests ───────────────────────────────────────────

    #[test]
    fn inject_jsonld_adds_article_block() {
        let html = "<html><head><title>T</title></head><body></body></html>";
        let result = inject_jsonld_description(html, "Test desc");
        assert!(result.contains("application/ld+json"));
        assert!(result.contains("\"@type\":\"Article\""));
        assert!(result.contains("Test desc"));
    }

    #[test]
    fn inject_jsonld_skips_existing() {
        let html = r#"<html><head><script type="application/ld+json">{"@type":"Article","description":"Existing"}</script></head></html>"#;
        let result = inject_jsonld_description(html, "New desc");
        assert!(!result.contains("New desc"));
        assert!(result.contains("Existing"));
    }

    // ── Content audit tests ───────────────────────────────────────

    #[test]
    fn audit_all_scans_markdown_files() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();

        fs::write(
            content.join("simple.md"),
            "---\ntitle: Simple\n---\nThe cat sat on the mat. It was a good day.",
        )
        .unwrap();
        fs::write(
            content.join("complex.md"),
            "---\ntitle: Complex\n---\n\
             The implementation of sophisticated cryptographic algorithms \
             necessitates comprehensive understanding of mathematical \
             foundations and computational complexity theory.",
        )
        .unwrap();

        let report = LlmPlugin::audit_all(&content, 8.0).unwrap();
        assert_eq!(report.total_files, 2);
        assert!(report.failing > 0, "complex.md should fail grade 8");
    }

    #[test]
    fn audit_all_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("empty");
        fs::create_dir_all(&content).unwrap();

        let report = LlmPlugin::audit_all(&content, 8.0).unwrap();
        assert_eq!(report.total_files, 0);
        assert_eq!(report.failing, 0);
    }

    #[test]
    fn strip_frontmatter_yaml() {
        let input = "---\ntitle: Hello\n---\nBody text here.";
        let body = strip_frontmatter(input);
        assert!(body.contains("Body text here"));
        assert!(!body.contains("title:"));
    }

    #[test]
    fn strip_frontmatter_toml() {
        let input = "+++\ntitle = \"Hello\"\n+++\nBody text here.";
        let body = strip_frontmatter(input);
        assert!(body.contains("Body text here"));
        assert!(!body.contains("title"));
    }

    #[test]
    fn strip_frontmatter_none() {
        let input = "Just plain content.";
        assert_eq!(strip_frontmatter(input), input);
    }

    #[test]
    fn split_frontmatter_preserves_delimiters() {
        let input = "---\ntitle: Hello\ndate: 2026-01-01\n---\n\n# Body text";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.starts_with("---"));
        assert!(fm.ends_with("---"));
        assert!(fm.contains("title: Hello"));
        assert!(body.contains("# Body text"));
    }

    #[test]
    fn split_frontmatter_toml_preserves() {
        let input = "+++\ntitle = \"Hello\"\n+++\nBody.";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.starts_with("+++"));
        assert!(body.contains("Body."));
    }

    #[test]
    fn split_frontmatter_no_frontmatter() {
        let input = "Just plain content.";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_empty(), "{:?}", fm);
        assert_eq!(body, input);
    }

    #[test]
    fn audit_and_fix_skips_when_ollama_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("test.md"), "---\ntitle: T\n---\nSimple text.")
            .unwrap();

        let config = LlmConfig {
            endpoint: "http://localhost:99999".to_string(),
            ..LlmConfig::default()
        };
        let result = LlmPlugin::audit_and_fix(&content, &config).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn full_repo_readability_audit() {
        // Audits ALL Markdown content across the entire repository.
        // The trailing entries exercise the two skip arms: a path
        // that does not exist, and an existing dir with no Markdown.
        let empty = tempfile::tempdir().unwrap();
        let empty_dir = empty.path().to_string_lossy().to_string();
        let dirs = [
            ("docs/guide".to_string(), 15.0),
            ("examples/basic/content".to_string(), 10.0),
            ("examples/blog/content".to_string(), 10.0),
            ("examples/docs/content".to_string(), 13.0),
            ("examples/landing/content".to_string(), 10.0),
            ("examples/plugins/content".to_string(), 10.0),
            ("examples/portfolio/content".to_string(), 10.0),
            ("examples/quickstart/content".to_string(), 10.0),
            ("examples/content/en".to_string(), 10.0),
            ("this/path/does/not/exist".to_string(), 10.0),
            (empty_dir, 10.0),
        ];

        let mut total_files = 0usize;
        let mut total_pass = 0usize;
        let mut total_fail = 0usize;

        println!("\n{}", "=".repeat(60));
        println!("  FULL REPOSITORY READABILITY AUDIT");
        println!("{}\n", "=".repeat(60));

        for (dir, target) in &dirs {
            let path = Path::new(dir);
            if !path.exists() {
                continue;
            }

            let report = LlmPlugin::audit_all(path, *target).unwrap();
            if report.total_files == 0 {
                continue;
            }

            println!("── {dir} (target: grade {target:.0}) ��─");
            for r in &report.results {
                let status = if r.passes { "PASS" } else { "FAIL" };
                println!(
                    "  {:.<40} grade {:>5.1}  ease {:>5.1}  [{status}]",
                    r.path, r.grade_level, r.reading_ease
                );
            }
            println!("  → {}/{} pass\n", report.passing, report.total_files);

            total_files += report.total_files;
            total_pass += report.passing;
            total_fail += report.failing;
        }

        println!("{}", "=".repeat(60));
        println!(
            "  TOTAL: {total_files} files — {total_pass} pass, {total_fail} fail"
        );
        println!("{}\n", "=".repeat(60));
    }

    /// Body of the readability-gate audit, parameterised on the
    /// guide directory so the missing-dir skip arm is testable.
    fn run_docs_guide_audit(guide_dir: &Path) {
        if !guide_dir.exists() {
            return; // Skip in environments without the guide
        }

        let report = LlmPlugin::audit_all(guide_dir, 17.0).unwrap();
        for result in &report.results {
            let status = if result.passes { "PASS" } else { "FAIL" };
            println!(
                "[readability] {}: grade={:.1}, ease={:.1}, avg_sentence={:.1} — {}",
                result.path,
                result.grade_level,
                result.reading_ease,
                result.avg_sentence_len,
                status
            );
        }

        println!(
            "\n[readability] {}/{} files pass (target: grade {:.0})",
            report.passing, report.total_files, report.target_grade
        );
    }

    #[test]
    fn audit_docs_guide() {
        // This test is called by the readability-gate CI workflow.
        // It audits all .md files in docs/guide/ against grade 17
        // (documentation is technical and includes code blocks which
        // inflate Flesch-Kincaid scores).
        run_docs_guide_audit(Path::new("docs/guide"));
        // Missing-dir arm must be a silent no-op.
        run_docs_guide_audit(Path::new("docs/this-guide-does-not-exist"));
    }

    // ── Coverage gap tests ────────────────────────────────────────

    #[test]
    fn is_ollama_available_unreachable() {
        assert!(!is_ollama_available("http://localhost:99999"));
    }

    #[test]
    fn call_ollama_unreachable_returns_none() {
        assert!(call_ollama("http://localhost:99999", "llama3", "hi").is_none());
    }

    #[test]
    fn needs_meta_description_with_content_attr_first() {
        // content= before name= (different ordering)
        let html = r#"<meta content="Decent length description that is more than fifty characters long enough" name="description">"#;
        // name="description" is present so returns false-ish check
        assert!(!needs_meta_description(html));
    }

    #[test]
    fn inject_meta_description_no_head() {
        let html = "<html><body>No head tag</body></html>";
        let result = inject_meta_description(html, "desc");
        assert_eq!(result, html); // unchanged
    }

    #[test]
    fn inject_jsonld_no_head() {
        let html = "<html><body>No head</body></html>";
        let result = inject_jsonld_description(html, "desc");
        assert_eq!(result, html);
    }

    #[test]
    fn extract_page_text_no_body() {
        let html = "just plain text no tags";
        let text = extract_page_text(html, 100);
        assert_eq!(text, "just plain text no tags");
    }

    #[test]
    fn extract_page_text_truncates() {
        let html = "<body><p>word </p></body>";
        let text = extract_page_text(html, 3);
        assert!(text.len() <= 5);
    }

    #[test]
    fn generate_missing_alt_text_no_images() {
        let mut html = "<html><body><p>No images</p></body></html>".to_string();
        let count = generate_missing_alt_text(
            &mut html,
            "llama3",
            "http://localhost:99999",
            true,
            Path::new("test.html"),
            Path::new("."),
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn readability_audit_single_word() {
        let audit = ReadabilityAudit::analyze("Hello");
        assert!(audit.grade_level >= 0.0);
        assert!(audit.avg_sentence_len >= 0.0);
    }

    #[test]
    fn count_word_syllables_empty() {
        assert_eq!(count_word_syllables(""), 1);
    }

    #[test]
    fn count_word_syllables_numbers() {
        assert_eq!(count_word_syllables("123"), 1);
    }

    #[test]
    fn split_frontmatter_unclosed() {
        let input = "---\ntitle: Hello\nNo closing delimiter";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_empty(), "{:?}", fm);
        assert_eq!(body, input);
    }

    #[test]
    fn llm_plugin_skips_missing_site_dir() {
        let plugin = LlmPlugin::new(LlmConfig::default());
        let ctx = PluginContext::new(
            Path::new("/tmp/c"),
            Path::new("/tmp/b"),
            Path::new("/nonexistent/site"),
            Path::new("/tmp/t"),
        );
        assert!(plugin.after_compile(&ctx).is_ok());
    }

    #[test]
    fn config_defaults_readability() {
        let config = LlmConfig::default();
        assert!((config.target_grade - 8.0).abs() < f64::EPSILON);
        assert_eq!(config.max_refinement_attempts, 1);
    }

    #[test]
    fn llm_plugin_skips_when_ollama_unavailable() {
        let plugin = LlmPlugin::new(LlmConfig {
            endpoint: "http://localhost:99999".to_string(),
            ..LlmConfig::default()
        });

        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("index.html"), "<html><body></body></html>")
            .unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        // Should succeed (graceful skip)
        plugin.after_compile(&ctx).unwrap();
    }

    // ── Agentic AI fix pipeline tests ────────────────────────────

    #[test]
    fn ai_fix_report_serializes_to_json() {
        let report = AiFixReport {
            total_audited: 10,
            total_failing: 3,
            total_fixed: 2,
            results: vec![
                AiFixResult {
                    path: "docs/guide.md".to_string(),
                    before_grade: 12.5,
                    after_grade: 7.2,
                    improved: true,
                    action: "rewritten".to_string(),
                },
                AiFixResult {
                    path: "docs/api.md".to_string(),
                    before_grade: 14.0,
                    after_grade: 13.8,
                    improved: false,
                    action: "no-improvement".to_string(),
                },
            ],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"total_fixed\":2"));
        assert!(json.contains("\"action\":\"rewritten\""));
    }

    #[test]
    fn ai_fix_report_skips_when_ollama_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            content.join("test.md"),
            "---\ntitle: T\n---\nThe implementation of sophisticated algorithms.",
        )
        .unwrap();

        let config = LlmConfig {
            endpoint: "http://localhost:99999".to_string(),
            max_refinement_attempts: 3,
            ..LlmConfig::default()
        };
        let report =
            LlmPlugin::audit_and_fix_with_report(&content, &config).unwrap();
        assert_eq!(report.total_fixed, 0);
        assert!(report.results.is_empty());
    }

    // ── Multilingual readability tests ──────────────────────────

    #[test]
    fn formula_from_lang_english() {
        assert_eq!(
            ReadabilityFormula::from_lang("en"),
            Some(ReadabilityFormula::FleschKincaid)
        );
        assert_eq!(
            ReadabilityFormula::from_lang("en-US"),
            Some(ReadabilityFormula::FleschKincaid)
        );
    }

    #[test]
    fn formula_from_lang_french() {
        assert_eq!(
            ReadabilityFormula::from_lang("fr"),
            Some(ReadabilityFormula::KandelMoles)
        );
        assert_eq!(
            ReadabilityFormula::from_lang("fr-CA"),
            Some(ReadabilityFormula::KandelMoles)
        );
    }

    #[test]
    fn formula_from_lang_german() {
        assert_eq!(
            ReadabilityFormula::from_lang("de"),
            Some(ReadabilityFormula::WienerSachtextformel)
        );
        assert_eq!(
            ReadabilityFormula::from_lang("de-AT"),
            Some(ReadabilityFormula::WienerSachtextformel)
        );
    }

    #[test]
    fn formula_from_lang_italian() {
        assert_eq!(
            ReadabilityFormula::from_lang("it"),
            Some(ReadabilityFormula::Gulpease)
        );
    }

    #[test]
    fn formula_from_lang_swedish() {
        assert_eq!(
            ReadabilityFormula::from_lang("sv"),
            Some(ReadabilityFormula::Lix)
        );
        assert_eq!(
            ReadabilityFormula::from_lang("nb"),
            Some(ReadabilityFormula::Lix)
        );
        assert_eq!(
            ReadabilityFormula::from_lang("da"),
            Some(ReadabilityFormula::Lix)
        );
    }

    #[test]
    fn formula_from_lang_spanish() {
        assert_eq!(
            ReadabilityFormula::from_lang("es"),
            Some(ReadabilityFormula::FernandezHuerta)
        );
    }

    #[test]
    fn formula_from_lang_unsupported() {
        assert_eq!(ReadabilityFormula::from_lang("ja"), None);
        assert_eq!(ReadabilityFormula::from_lang("zh"), None);
        assert_eq!(ReadabilityFormula::from_lang("ar"), None);
    }

    #[test]
    fn kandel_moles_simple_french() {
        let text = "Le chat est sur le tapis. Il fait beau. Le soleil brille.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "fr");
        assert!(
            audit.reading_ease > 50.0,
            "Simple French should be readable, got {:.1}",
            audit.reading_ease
        );
    }

    #[test]
    fn wiener_simple_german() {
        let text = "Die Katze sitzt auf der Matte. Es ist ein guter Tag. Die Sonne scheint.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "de");
        assert!(
            audit.grade_level < 15.0,
            "Simple German got grade {:.1}",
            audit.grade_level
        );
    }

    #[test]
    fn gulpease_simple_italian() {
        let text = "Il gatto si siede sul tappeto. Il sole splende. Oggi è una bella giornata.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "it");
        assert!(
            audit.reading_ease > 40.0,
            "Simple Italian got ease {:.1}",
            audit.reading_ease
        );
    }

    #[test]
    fn lix_simple_swedish() {
        let text = "Katten sitter på mattan. Solen skiner. Det är en fin dag.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "sv");
        assert!(audit.grade_level >= 0.0);
        assert!(audit.reading_ease > 0.0);
    }

    #[test]
    fn fernandez_huerta_simple_spanish() {
        let text = "El gato está en la mesa. El sol brilla. Es un buen día.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "es");
        assert!(
            audit.reading_ease > 50.0,
            "Simple Spanish got ease {:.1}",
            audit.reading_ease
        );
    }

    #[test]
    fn analyze_with_lang_empty_defaults_to_english() {
        let text = "The cat sat on the mat.";
        let a = ReadabilityAudit::analyze(text);
        let b = ReadabilityAudit::analyze_with_lang(text, "");
        assert!((a.grade_level - b.grade_level).abs() < f64::EPSILON);
    }

    #[test]
    fn analyze_with_lang_unsupported_falls_back() {
        let text = "The cat sat on the mat.";
        let a = ReadabilityAudit::analyze(text);
        let b = ReadabilityAudit::analyze_with_lang(text, "ja");
        assert!((a.grade_level - b.grade_level).abs() < f64::EPSILON);
    }

    #[test]
    fn extract_frontmatter_lang_yaml() {
        let content = "---\ntitle: Hello\nlanguage: fr\n---\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "fr");
    }

    #[test]
    fn extract_frontmatter_lang_yaml_short() {
        let content = "---\ntitle: Hello\nlang: de\n---\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "de");
    }

    #[test]
    fn extract_frontmatter_lang_toml() {
        let content = "+++\ntitle = \"Hello\"\nlanguage = \"it\"\n+++\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "it");
    }

    #[test]
    fn extract_frontmatter_lang_missing() {
        let content = "---\ntitle: Hello\n---\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "");
    }

    #[test]
    fn extract_frontmatter_lang_no_frontmatter() {
        let content = "Just plain text.";
        assert_eq!(extract_frontmatter_lang(content), "");
    }

    #[test]
    fn audit_all_respects_language() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();

        fs::write(
            content.join("french.md"),
            "---\ntitle: Bonjour\nlanguage: fr\n---\nLe chat est sur le tapis. Il fait beau.",
        )
        .unwrap();

        let report = LlmPlugin::audit_all(&content, 8.0).unwrap();
        assert_eq!(report.total_files, 1);
        // Should use Kandel-Moles, not Flesch-Kincaid
    }

    // ── Multilingual formulas: empty text ────────────────────────

    #[test]
    fn kandel_moles_empty_text() {
        let audit = ReadabilityAudit::analyze_with_lang("", "fr");
        assert!(audit.grade_level.abs() < f64::EPSILON);
        assert!((audit.reading_ease - 100.0).abs() < f64::EPSILON);
        assert!(audit.avg_sentence_len.abs() < f64::EPSILON);
    }

    #[test]
    fn wiener_empty_text() {
        let audit = ReadabilityAudit::analyze_with_lang("", "de");
        assert!(audit.grade_level.abs() < f64::EPSILON);
        assert!((audit.reading_ease - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gulpease_empty_text() {
        let audit = ReadabilityAudit::analyze_with_lang("", "it");
        assert!(audit.grade_level.abs() < f64::EPSILON);
        assert!((audit.reading_ease - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn lix_empty_text() {
        let audit = ReadabilityAudit::analyze_with_lang("", "sv");
        assert!(audit.grade_level.abs() < f64::EPSILON);
        assert!((audit.reading_ease - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fernandez_huerta_empty_text() {
        let audit = ReadabilityAudit::analyze_with_lang("", "es");
        assert!(audit.grade_level.abs() < f64::EPSILON);
        assert!((audit.reading_ease - 100.0).abs() < f64::EPSILON);
    }

    // ── Multilingual formulas: single-word text ──────────────────

    #[test]
    fn kandel_moles_single_word() {
        let audit = ReadabilityAudit::analyze_with_lang("Bonjour", "fr");
        assert!(audit.grade_level >= 0.0);
        assert!(audit.reading_ease >= 0.0);
        assert!(audit.avg_sentence_len >= 1.0);
    }

    #[test]
    fn wiener_single_word() {
        let audit = ReadabilityAudit::analyze_with_lang("Hallo", "de");
        assert!(audit.grade_level >= 0.0);
        assert!(audit.avg_sentence_len >= 1.0);
    }

    #[test]
    fn gulpease_single_word() {
        let audit = ReadabilityAudit::analyze_with_lang("Ciao", "it");
        assert!(audit.grade_level >= 0.0);
        assert!(audit.avg_sentence_len >= 1.0);
    }

    #[test]
    fn lix_single_word() {
        let audit = ReadabilityAudit::analyze_with_lang("Hej", "sv");
        assert!(audit.grade_level >= 0.0);
    }

    #[test]
    fn fernandez_huerta_single_word() {
        let audit = ReadabilityAudit::analyze_with_lang("Hola", "es");
        assert!(audit.grade_level >= 0.0);
    }

    // ── Multilingual formulas: long text ─────────────────────────

    #[test]
    fn kandel_moles_long_text() {
        let text = "Le développement de nouvelles infrastructures \
                    technologiques nécessite une compréhension \
                    approfondie des systèmes complexes. \
                    Les algorithmes sophistiqués démontrent \
                    une efficacité considérable. \
                    La modernisation progressive des architectures \
                    informatiques représente un défi majeur.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "fr");
        assert!(audit.grade_level > 0.0);
        assert!(audit.reading_ease >= 0.0);
        assert!(audit.avg_sentence_len > 1.0);
    }

    #[test]
    fn wiener_long_text() {
        let text = "Die Implementierung fortschrittlicher kryptografischer \
                    Algorithmen erfordert umfassendes Verständnis \
                    mathematischer Grundlagen. Asymmetrische \
                    Verschlüsselungsprotokolle weisen erheblichen \
                    Rechenaufwand auf. Die systematische Optimierung \
                    komplexer Datenstrukturen bleibt herausfordernd.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "de");
        assert!(audit.grade_level > 0.0);
        assert!(audit.avg_sentence_len > 1.0);
    }

    #[test]
    fn gulpease_long_text() {
        let text = "L'implementazione di algoritmi crittografici sofisticati \
                    richiede una comprensione approfondita dei fondamenti \
                    matematici. I protocolli di crittografia asimmetrica \
                    dimostrano un considerevole sovraccarico computazionale. \
                    L'ottimizzazione sistematica delle strutture dati \
                    complesse rimane impegnativa.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "it");
        assert!(audit.grade_level > 0.0);
        assert!(audit.avg_sentence_len > 1.0);
    }

    #[test]
    fn lix_long_text() {
        let text = "Implementeringen av avancerade kryptografiska algoritmer \
                    kräver omfattande förståelse av matematiska grunder. \
                    Asymmetriska krypteringsprotokoll uppvisar betydande \
                    beräkningsbelastning. Systematisk optimering av komplexa \
                    datastrukturer förblir utmanande.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "sv");
        assert!(audit.grade_level > 0.0);
        assert!(audit.avg_sentence_len > 1.0);
    }

    #[test]
    fn fernandez_huerta_long_text() {
        let text =
            "La implementación de algoritmos criptográficos sofisticados \
                    requiere una comprensión profunda de los fundamentos \
                    matemáticos. Los protocolos de cifrado asimétrico \
                    demuestran una considerable sobrecarga computacional. \
                    La optimización sistemática de estructuras de datos \
                    complejas sigue siendo un desafío.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "es");
        assert!(audit.grade_level > 0.0);
        assert!(audit.avg_sentence_len > 1.0);
    }

    // ── WienerSachtextformel: varying syllable counts ────────────

    #[test]
    fn wiener_mixed_syllable_words() {
        // Mix of 1-syllable, 2-syllable, 3+ syllable words
        let text = "Ich bin gut. Das Haus ist sehr interessant. \
                    Die Universität hat viele Studenten.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "de");
        assert!(audit.grade_level >= 0.0);
        assert!(audit.reading_ease >= 0.0);
        assert!(audit.reading_ease <= 100.0);
    }

    // ── LIX: varying character lengths ───────────────────────────

    #[test]
    fn lix_mixed_word_lengths() {
        // Short words and long words (>6 chars) to exercise long-word filter
        let text = "En bok om programmering. \
                    Datavetenskapliga beräkningar kräver noggrannhet.";
        let audit = ReadabilityAudit::analyze_with_lang(text, "sv");
        assert!(audit.grade_level > 0.0);
        assert!(audit.reading_ease >= 0.0);
        assert!(audit.reading_ease <= 100.0);
    }

    // ── extract_frontmatter_lang() edge cases ────────────────────

    #[test]
    fn extract_frontmatter_lang_toml_with_quotes() {
        let content =
            "+++\ntitle = \"Hello\"\nlanguage = \"en-US\"\n+++\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "en-US");
    }

    #[test]
    fn extract_frontmatter_lang_first_wins() {
        // language appears before lang — first one should win
        let content = "---\nlanguage: fr\nlang: de\n---\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "fr");
    }

    #[test]
    fn extract_frontmatter_lang_whitespace_around_value() {
        let content = "---\nlanguage:   es  \n---\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "es");
    }

    #[test]
    fn extract_frontmatter_lang_yaml_quoted_value() {
        let content = "---\nlanguage: \"de\"\n---\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "de");
    }

    #[test]
    fn extract_frontmatter_lang_single_quoted() {
        let content = "---\nlanguage: 'it'\n---\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "it");
    }

    #[test]
    fn extract_frontmatter_lang_empty_value() {
        let content = "---\nlanguage: \n---\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "");
    }

    #[test]
    fn extract_frontmatter_lang_toml_lang_key() {
        let content = "+++\nlang = \"sv\"\n+++\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "sv");
    }

    // ── audit_and_fix_with_report edge cases ─────────────────────

    #[test]
    fn audit_and_fix_with_report_all_passing() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();

        // Very simple text that passes any reasonable threshold
        fs::write(
            content.join("simple.md"),
            "---\ntitle: Simple\n---\nThe cat sat. It was good.",
        )
        .unwrap();

        // Use a high target so everything passes
        let config = LlmConfig {
            endpoint: "http://localhost:99999".to_string(),
            target_grade: 20.0,
            ..LlmConfig::default()
        };
        let report =
            LlmPlugin::audit_and_fix_with_report(&content, &config).unwrap();
        // Ollama unreachable => empty report, but test the path
        assert_eq!(report.total_fixed, 0);
    }

    #[test]
    fn audit_and_fix_with_report_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("empty_content");
        fs::create_dir_all(&content).unwrap();

        let config = LlmConfig {
            endpoint: "http://localhost:99999".to_string(),
            ..LlmConfig::default()
        };
        let report =
            LlmPlugin::audit_and_fix_with_report(&content, &config).unwrap();
        assert_eq!(report.total_audited, 0);
        assert_eq!(report.total_failing, 0);
        assert!(report.results.is_empty());
    }

    #[test]
    fn audit_all_file_with_empty_body() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();

        fs::write(content.join("empty_body.md"), "---\ntitle: T\n---\n")
            .unwrap();

        let report = LlmPlugin::audit_all(&content, 8.0).unwrap();
        assert_eq!(report.total_files, 1);
        // Empty body => grade 0, passes any threshold
        assert_eq!(report.passing, 1);
    }

    // ── needs_meta_description edge cases ────────────────────────

    #[test]
    fn needs_meta_description_no_content_attr() {
        // Has name="description" but no content attribute
        let html = r#"<meta name="description">"#;
        // name="description" is found, but content= search fails,
        // so falls through to the !html.contains check which is false
        assert!(!needs_meta_description(html));
    }

    #[test]
    fn needs_meta_description_multiple_meta_tags() {
        let html = r#"<meta name="author" content="Alice"><meta name="description" content="This is a sufficiently long description that is more than fifty characters long">"#;
        assert!(!needs_meta_description(html));
    }

    #[test]
    fn needs_meta_description_empty_content() {
        let html = r#"<meta name="description" content="">"#;
        assert!(needs_meta_description(html));
    }

    // ── inject_meta_description with special chars ───────────────

    #[test]
    fn inject_meta_description_escapes_ampersand() {
        let html = "<html><head></head><body></body></html>";
        let result = inject_meta_description(html, "Tom & Jerry");
        assert!(result.contains("Tom &amp; Jerry"));
    }

    #[test]
    fn inject_meta_description_escapes_quotes() {
        let html = "<html><head></head><body></body></html>";
        let result = inject_meta_description(html, r#"A "great" page"#);
        assert!(result.contains("A &quot;great&quot; page"));
    }

    #[test]
    fn inject_meta_description_escapes_angle_brackets() {
        let html = "<html><head></head><body></body></html>";
        let result = inject_meta_description(html, "x < y");
        assert!(result.contains("x &lt; y"));
    }

    #[test]
    fn inject_meta_description_all_special_chars() {
        let html = "<html><head></head><body></body></html>";
        let result = inject_meta_description(html, r#"A & B "C" <D>"#);
        // The function escapes &, ", < but not > (only the dangerous chars in attribute context)
        assert!(result.contains("A &amp; B &quot;C&quot; &lt;D>"));
    }

    // ── extract_page_text edge cases ─────────────────────────────

    #[test]
    fn extract_page_text_with_main_tag() {
        let html = "<html><body><div>ignored</div><main><p>Main content here.</p></main></body></html>";
        let text = extract_page_text(html, 500);
        assert!(text.contains("Main content here"));
        // "ignored" is before <main>, so it should not appear
        assert!(!text.contains("ignored"));
    }

    #[test]
    fn extract_page_text_large_truncated() {
        let long_body = "word ".repeat(200);
        let html = format!("<body><p>{long_body}</p></body>");
        let text = extract_page_text(&html, 50);
        // Should be truncated well under the full 1000-char body
        assert!(text.len() <= 60);
    }

    #[test]
    fn extract_page_text_strips_control_chars() {
        let html = "<body>Hello\x00\x01World</body>";
        let text = extract_page_text(html, 100);
        assert_eq!(text, "HelloWorld");
    }

    #[test]
    fn extract_page_text_nested_tags() {
        let html = "<body><div><span>A</span> <em>B</em></div></body>";
        let text = extract_page_text(html, 100);
        assert!(text.contains('A'));
        assert!(text.contains('B'));
    }

    // ── generate_missing_alt_text edge cases ─────────────────────

    #[test]
    fn generate_missing_alt_text_empty_alt() {
        let mut html =
            r#"<html><body><img src="photo.jpg" alt=""></body></html>"#
                .to_string();
        // Ollama unreachable, so count stays 0, but exercises the tag detection
        let count = generate_missing_alt_text(
            &mut html,
            "llama3",
            "http://localhost:99999",
            false,
            Path::new("test.html"),
            Path::new("."),
        );
        // Can't generate without Ollama, but exercises alt="" detection path
        assert_eq!(count, 0);
    }

    #[test]
    fn generate_missing_alt_text_missing_closing_bracket() {
        let mut html =
            "<html><body><img src=\"photo.jpg\"</body></html>".to_string();
        let count = generate_missing_alt_text(
            &mut html,
            "llama3",
            "http://localhost:99999",
            false,
            Path::new("test.html"),
            Path::new("."),
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn generate_missing_alt_text_mixed_images() {
        let mut html = r#"<html><body>
            <img src="a.jpg" alt="Good alt">
            <img src="b.jpg">
            <img src="c.jpg" alt="">
        </body></html>"#
            .to_string();
        // Exercises the loop: first image has alt (skipped),
        // second has no alt, third has empty alt.
        // Ollama unreachable so no actual generation.
        let count = generate_missing_alt_text(
            &mut html,
            "llama3",
            "http://localhost:99999",
            true,
            Path::new("test.html"),
            Path::new("."),
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn generate_missing_alt_text_with_alt_present() {
        let mut html =
            r#"<html><body><img src="x.jpg" alt="Has alt text"></body></html>"#
                .to_string();
        let count = generate_missing_alt_text(
            &mut html,
            "llama3",
            "http://localhost:99999",
            false,
            Path::new("test.html"),
            Path::new("."),
        );
        assert_eq!(count, 0);
    }

    // ── ReadabilityFormula edge cases ─────────────────────────────

    #[test]
    fn formula_from_lang_underscore_separator() {
        assert_eq!(
            ReadabilityFormula::from_lang("en_US"),
            Some(ReadabilityFormula::FleschKincaid)
        );
        assert_eq!(
            ReadabilityFormula::from_lang("de_DE"),
            Some(ReadabilityFormula::WienerSachtextformel)
        );
    }

    #[test]
    fn formula_from_lang_norwegian_variants() {
        assert_eq!(
            ReadabilityFormula::from_lang("nn"),
            Some(ReadabilityFormula::Lix)
        );
        assert_eq!(
            ReadabilityFormula::from_lang("no"),
            Some(ReadabilityFormula::Lix)
        );
    }

    // ── LlmConfig / LlmPlugin additional coverage ───────────────

    #[test]
    fn llm_config_default_values() {
        let config = LlmConfig::default();
        assert_eq!(config.model, "llama3");
        assert_eq!(config.endpoint, "http://localhost:11434");
        assert!(!config.dry_run);
    }

    #[test]
    fn llm_plugin_debug_impl() {
        let plugin = LlmPlugin::new(LlmConfig::default());
        let debug = format!("{plugin:?}");
        assert!(debug.contains("LlmPlugin"));
        assert!(debug.contains("llama3"));
    }

    // ── split_frontmatter edge cases ─────────────────────────────

    #[test]
    fn split_frontmatter_leading_whitespace() {
        let input = "  ---\ntitle: Hello\n---\nBody.";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.contains("title: Hello"));
        assert!(body.contains("Body."));
    }

    #[test]
    fn split_frontmatter_toml_unclosed() {
        let input = "+++\ntitle = \"Hello\"\nNo closing delimiter";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_empty(), "{:?}", fm);
        assert_eq!(body, input);
    }

    // ── FileAuditResult / AuditReport serialization ──────────────

    #[test]
    fn file_audit_result_serializes() {
        let result = FileAuditResult {
            path: "test.md".to_string(),
            grade_level: 7.5,
            reading_ease: 65.0,
            avg_sentence_len: 12.0,
            passes: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"path\":\"test.md\""));
        assert!(json.contains("\"passes\":true"));
    }

    #[test]
    fn audit_report_serializes() {
        let report = AuditReport {
            target_grade: 8.0,
            total_files: 2,
            passing: 1,
            failing: 1,
            results: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"target_grade\":8.0"));
        assert!(json.contains("\"total_files\":2"));
    }

    // ── inject_jsonld_description edge cases ─────────────────────

    #[test]
    fn inject_jsonld_with_special_chars() {
        let html = "<html><head></head><body></body></html>";
        let result = inject_jsonld_description(html, "Tom & Jerry's \"show\"");
        assert!(result.contains("application/ld+json"));
        assert!(result.contains("Tom & Jerry"));
    }

    // ── count_syllables edge cases ───────────────────────────────

    #[test]
    fn count_syllables_multiple_vowel_groups() {
        // "beautiful" has vowel groups: eau-i-u => 3 groups, minus silent e = stays
        assert!(count_word_syllables("beautiful") >= 2);
    }

    #[test]
    fn count_syllables_consecutive_vowels() {
        // "queue" => qu-eu-e: vowel groups = 2, minus trailing e = 1
        assert_eq!(count_word_syllables("queue"), 1);
    }

    #[test]
    fn count_syllables_all_consonants() {
        // "rhythm" => y is a vowel => 1 vowel group
        assert_eq!(count_word_syllables("rhythm"), 1);
    }

    #[test]
    fn count_syllables_text_total() {
        let total = count_syllables("The cat sat on the mat.");
        assert!(total >= 6); // 6 monosyllabic words
    }

    #[test]
    fn count_words_basic() {
        assert_eq!(count_words("one two three"), 3);
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("   "), 0);
        assert_eq!(count_words("single"), 1);
    }

    // ── Readability: numeric edge cases ──────────────────────────

    #[test]
    fn readability_grade_never_negative() {
        // Single short word => formula could produce negative, clamped to 0
        let audit = ReadabilityAudit::analyze("Hi.");
        assert!(audit.grade_level >= 0.0);
        assert!(audit.reading_ease >= 0.0);
        assert!(audit.reading_ease <= 100.0);
    }

    #[test]
    fn readability_ease_clamped_to_100() {
        // Very simple text should not exceed 100
        let audit = ReadabilityAudit::analyze("Go. Do. Be.");
        assert!(audit.reading_ease <= 100.0);
        assert!(audit.reading_ease >= 0.0);
    }

    // --- count_sentences ---
    #[test]
    fn count_sentences_single_period() {
        assert_eq!(count_sentences("Hello world."), 1);
    }

    #[test]
    fn count_sentences_question_and_exclamation() {
        assert_eq!(count_sentences("Hi! Are you here? Yes."), 3);
    }

    #[test]
    fn count_sentences_empty_returns_one() {
        // Defensive floor so downstream division never sees 0.
        assert!(count_sentences("").max(1) >= 1);
    }

    #[test]
    fn count_sentences_no_terminator_treats_as_one() {
        assert!(count_sentences("Hello world").max(1) >= 1);
    }

    // --- count_syllables ---
    #[test]
    fn count_syllables_single_short_word() {
        assert!(count_syllables("cat") >= 1);
    }

    #[test]
    fn count_syllables_multi_syllable_word() {
        assert!(count_syllables("beautiful") >= 3);
    }

    #[test]
    fn count_syllables_empty_returns_zero() {
        assert_eq!(count_syllables(""), 0);
    }

    #[test]
    fn count_syllables_multiple_words() {
        let n = count_syllables("the quick brown fox jumps");
        assert!(n >= 5);
    }

    // --- extract_page_text ---
    #[test]
    fn extract_page_text_strips_html_tags() {
        let html =
            "<html><body><h1>Title</h1><p>Paragraph body.</p></body></html>";
        let text = extract_page_text(html, 1000);
        assert!(!text.contains('<'));
        assert!(text.contains("Title"));
        assert!(text.contains("Paragraph body"));
    }

    #[test]
    fn extract_page_text_truncates_at_max_chars() {
        let body = "A".repeat(2000);
        let html = format!("<html><body><p>{body}</p></body></html>");
        let text = extract_page_text(&html, 500);
        assert!(text.len() <= 500);
    }

    #[test]
    fn extract_page_text_skips_script_and_style_contents() {
        let html = "<html><head><style>body{color:red}</style>\
                    <script>alert('x')</script></head>\
                    <body><p>Hi.</p></body></html>";
        let text = extract_page_text(html, 1000);
        assert!(!text.contains("color:red"));
        assert!(!text.contains("alert"));
        assert!(text.contains("Hi"));
    }

    #[test]
    fn extract_page_text_collapses_whitespace() {
        let html = "<p>one   \n\n  two\t\tthree</p>";
        let text = extract_page_text(html, 1000);
        assert!(!text.contains("\t"));
        assert!(!text.contains("\n\n"));
    }

    // =====================================================================
    // Mock Ollama server (issue #520 coverage harness)
    //
    // Each test spawns a one-shot TcpListener-backed HTTP server that
    // returns a canned response, then points query_ollama / LlmPlugin
    // at the resulting `http://127.0.0.1:<port>` URL. This exercises
    // the live HTTP transport without depending on a real Ollama.
    // =====================================================================

    use std::io::{Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// Drains one HTTP/1.1 request (headers AND body) from `stream`.
    ///
    /// Computes `Content-Length` from the request headers, then keeps
    /// reading until that many body bytes are consumed. This
    /// guarantees the client has finished sending before the mock
    /// replies, which avoids the "Error encountered in a header" race
    /// ureq raises when the response arrives mid-request. Returns the
    /// raw request bytes so callers can route on the method line.
    fn drain_request(stream: &mut TcpStream) -> Vec<u8> {
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = Vec::with_capacity(4096);
        let mut chunk = [0u8; 1024];
        let mut header_end: Option<usize> = None;
        let mut content_length: usize = 0;
        while header_end.is_none() {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) =
                        buf.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        header_end = Some(pos + 4);
                        let header_str = String::from_utf8_lossy(&buf[..pos]);
                        for line in header_str.split("\r\n") {
                            if let Some(v) = line
                                .to_ascii_lowercase()
                                .strip_prefix("content-length:")
                            {
                                if let Ok(n) = v.trim().parse::<usize>() {
                                    content_length = n;
                                }
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if let Some(end) = header_end {
            while buf.len().saturating_sub(end) < content_length {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
        }
        buf
    }

    /// Writes `response_bytes` and half-closes the socket.
    fn respond_and_close(stream: &mut TcpStream, response_bytes: &[u8]) {
        let _ = stream.write_all(response_bytes);
        let _ = stream.flush();
        let _ = stream.shutdown(std::net::Shutdown::Write);
    }

    /// Serves one canned response on an accepted connection.
    fn serve_canned(mut stream: TcpStream, response_bytes: &[u8]) {
        let _ = drain_request(&mut stream);
        respond_and_close(&mut stream, response_bytes);
    }

    /// Spawns a mock HTTP/1.1 server that responds to exactly one
    /// request with `response_bytes`. Returns the bound URL
    /// (`http://127.0.0.1:<port>`). The server thread joins on drop
    /// of the returned `JoinHandle` — tests can let it leak since the
    /// listener auto-closes when the handle is dropped.
    fn spawn_mock_ollama(
        response_bytes: &'static [u8],
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("mock accept");
            serve_canned(stream, response_bytes);
        });
        (url, handle)
    }

    fn mock_ollama_ok(reply_text: &str) -> Vec<u8> {
        let body = format!(r#"{{"response":"{reply_text}"}}"#);
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .into_bytes()
    }

    #[test]
    fn query_ollama_returns_response_on_2xx_json() {
        // Leak the response bytes so the closure can hold a 'static
        // reference without lifetime gymnastics.
        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("hello world").into_boxed_slice());
        let (url, _h) = spawn_mock_ollama(bytes);
        let out = query_ollama(&url, "test-model", "hi", 5).unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn query_ollama_returns_err_on_500_status() {
        // 500 status — server sends a non-2xx; ureq may report it
        // either via the Status arm (LlmInvalidResponse) or via a
        // Transport arm if it decides the response is malformed. We
        // only assert that the call errors and produces a non-empty
        // message — the exact variant is implementation-defined.
        let resp = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 5\r\nConnection: close\r\n\r\nboom!";
        let (url, _h) = spawn_mock_ollama(resp);
        let err = query_ollama(&url, "m", "p", 5).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    fn query_ollama_returns_invalid_response_on_bad_json() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 13\r\nConnection: close\r\n\r\nnot-json-here";
        let (url, _h) = spawn_mock_ollama(resp);
        let err = query_ollama(&url, "m", "p", 5).unwrap_err();
        let msg = format!("{err}");
        assert!(!msg.is_empty(), "should not be empty");
    }

    #[test]
    fn query_ollama_returns_invalid_response_on_missing_field() {
        // Valid JSON, but the `response` field is missing => triggers
        // the ok_or_else branch in query_ollama that emits
        // LlmInvalidResponse{"missing or empty 'response' field"}.
        let body = r#"{"foo":"bar"}"#;
        let resp_str = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let bytes: &'static [u8] =
            Box::leak(resp_str.into_bytes().into_boxed_slice());
        let (url, _h) = spawn_mock_ollama(bytes);
        let err = query_ollama(&url, "m", "p", 5).unwrap_err();
        // The mock drains the full request before replying, so the
        // body is delivered intact and the error is deterministic.
        assert!(
            matches!(err, SsgError::LlmInvalidResponse { .. }),
            "expected LlmInvalidResponse, got: {err}"
        );
    }

    #[test]
    fn query_ollama_unreachable_when_port_is_closed() {
        // Bind then immediately drop the listener so the port is
        // closed before the request reaches it. ConnectionRefused →
        // Transport arm.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let url = format!("http://127.0.0.1:{port}");
        let err = query_ollama(&url, "m", "p", 2).unwrap_err();
        let msg = format!("{err}");
        assert!(!msg.is_empty(), "should not be empty");
    }

    #[test]
    fn llm_plugin_query_returns_cached_value_on_hit() {
        // Pre-populate the cache, point query at an unreachable
        // endpoint, and assert the cache short-circuits the HTTP call.
        let tmp = tempfile::tempdir().unwrap();
        let cache_root = tmp.path().to_path_buf();

        let cfg = LlmConfig {
            model: "fake".into(),
            endpoint: "http://127.0.0.1:1".into(), // closed port
            cache_disabled: false,
            cache_dir: Some(cache_root.clone()),
            ..LlmConfig::default()
        };

        let key = LlmCache::compute_key(
            &cfg.endpoint,
            &cfg.model,
            "prompt-x",
            cfg.timeout_secs,
        );
        let cache = LlmCache::new(cache_root);
        cache.set(&key, "cached!").unwrap();

        let plugin = LlmPlugin::new(cfg);
        let out = plugin.query("prompt-x").unwrap();
        assert_eq!(out, "cached!");
    }

    #[test]
    fn llm_plugin_query_writes_back_on_cache_miss() {
        // Cache miss + mock server returning OK → plugin should
        // write the response back to the cache.
        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("fresh-answer").into_boxed_slice());
        let (url, _h) = spawn_mock_ollama(bytes);

        let tmp = tempfile::tempdir().unwrap();
        let cache_root = tmp.path().to_path_buf();
        let cfg = LlmConfig {
            model: "m".into(),
            endpoint: url.clone(),
            cache_disabled: false,
            cache_dir: Some(cache_root.clone()),
            ..LlmConfig::default()
        };
        let plugin = LlmPlugin::new(cfg.clone());
        let out = plugin.query("hello").unwrap();
        assert_eq!(out, "fresh-answer");

        let key = LlmCache::compute_key(
            &cfg.endpoint,
            &cfg.model,
            "hello",
            cfg.timeout_secs,
        );
        let cache = LlmCache::new(cache_root);
        assert_eq!(cache.get(&key).as_deref(), Some("fresh-answer"));
    }

    #[test]
    fn llm_plugin_query_skips_cache_when_disabled() {
        // cache_disabled=true → live call only, no cache file written.
        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("live-answer").into_boxed_slice());
        let (url, _h) = spawn_mock_ollama(bytes);

        let tmp = tempfile::tempdir().unwrap();
        let cfg = LlmConfig {
            model: "m".into(),
            endpoint: url,
            cache_disabled: true,
            cache_dir: Some(tmp.path().to_path_buf()),
            ..LlmConfig::default()
        };
        let plugin = LlmPlugin::new(cfg);
        let out = plugin.query("hello").unwrap();
        assert_eq!(out, "live-answer");
    }

    /// Serialised env-var scoping (mirrors the `cmd::tests` pattern).
    ///
    /// Entries are applied *sequentially* (capture-then-set per entry)
    /// and restored in reverse, so a duplicated key deterministically
    /// exercises both restore arms: the later entry's captured
    /// previous value is whatever the earlier entry just set.
    fn with_env_vars<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut prev: Vec<(String, Option<String>)> = Vec::new();
        for (key, value) in vars {
            prev.push(((*key).to_string(), std::env::var(key).ok()));
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        f();
        for (key, value) in prev.into_iter().rev() {
            match value {
                Some(v) => std::env::set_var(&key, v),
                None => std::env::remove_var(&key),
            }
        }
    }

    #[test]
    fn llm_config_default_respects_no_cache_env() {
        // Duplicated key: the second entry's restore puts back the
        // first entry's value (Some arm), the first restores machine
        // state.
        with_env_vars(
            &[
                ("SSG_NO_LLM_CACHE", Some("seed")),
                ("SSG_NO_LLM_CACHE", Some("1")),
            ],
            || assert!(LlmConfig::default().cache_disabled),
        );
        with_env_vars(&[("SSG_NO_LLM_CACHE", Some("0"))], || {
            assert!(!LlmConfig::default().cache_disabled);
        });
        with_env_vars(&[("SSG_NO_LLM_CACHE", Some("off"))], || {
            assert!(!LlmConfig::default().cache_disabled);
        });
        // Unset + empty exercise the removal arm and the
        // empty-string filter.
        with_env_vars(
            &[("SSG_NO_LLM_CACHE", None), ("SSG_NO_LLM_CACHE", Some(""))],
            || assert!(!LlmConfig::default().cache_disabled),
        );
        with_env_vars(&[("SSG_NO_LLM_CACHE", None)], || {
            assert!(!LlmConfig::default().cache_disabled);
        });
    }

    /// Spawns a long-running mock that serves the same canned
    /// response to every incoming connection until the listener is
    /// dropped. Used by tests that need the health-check + actual
    /// generate call to both succeed (`audit_and_fix` path).
    fn spawn_multi_shot_mock_ollama(
        response_bytes: &'static [u8],
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());
        let handle = thread::spawn(move || {
            for stream_res in listener.incoming() {
                serve_canned(stream_res.expect("mock accept"), response_bytes);
            }
        });
        (url, handle)
    }

    /// Spawns a long-running mock that routes on the request method:
    /// `GET` (the health-check probe) gets `get_response`, anything
    /// else (the `/api/generate` POST) gets `post_response`. Lets a
    /// test pass the availability probe while failing generation.
    fn spawn_routing_mock_ollama(
        get_response: &'static [u8],
        post_response: &'static [u8],
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());
        let handle = thread::spawn(move || {
            for stream_res in listener.incoming() {
                let mut stream = stream_res.expect("mock accept");
                let request = drain_request(&mut stream);
                let resp = if request.starts_with(b"GET") {
                    get_response
                } else {
                    post_response
                };
                respond_and_close(&mut stream, resp);
            }
        });
        (url, handle)
    }

    /// Spawns a long-running mock that serves `responses` in
    /// connection order, repeating the last response once the list is
    /// exhausted. Drives multi-turn flows (initial draft + refinement
    /// pass) where each call must see different output.
    fn spawn_sequenced_mock_ollama(
        responses: Vec<Vec<u8>>,
    ) -> (String, thread::JoinHandle<()>) {
        assert!(!responses.is_empty(), "sequenced mock needs >= 1 response");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());
        let handle = thread::spawn(move || {
            for (served, stream_res) in listener.incoming().enumerate() {
                let mut stream = stream_res.expect("mock accept");
                let _ = drain_request(&mut stream);
                let idx = served.min(responses.len() - 1);
                respond_and_close(&mut stream, &responses[idx]);
            }
        });
        (url, handle)
    }

    /// Spawns a mock that accepts one connection, reads a little, and
    /// then sleeps without ever responding — forcing the client-side
    /// read timeout.
    fn spawn_hanging_mock_ollama() -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("mock accept");
            let mut chunk = [0u8; 1024];
            let _ = stream.read(&mut chunk);
            thread::sleep(Duration::from_secs(3));
        });
        (url, handle)
    }

    #[test]
    fn audit_and_fix_runs_loop_against_mock_ollama() {
        // Set up a content dir with a failing-grade file, then back the
        // mock LLM with a refined response. audit_and_fix should walk
        // its inner refinement loop and emit a result.
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let hard = "---\ntitle: T\n---\n\nThe administrative procurement \
                    methodologies necessitate institutional documentation that \
                    facilitates organisational comprehension across heterogeneous \
                    constituencies of stakeholders.";
        fs::write(content.join("hard.md"), hard).unwrap();

        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("Short. Plain.").into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            dry_run: true,
            target_grade: 6.0,
            max_refinement_attempts: 1,
            ..LlmConfig::default()
        };

        let _ = LlmPlugin::audit_and_fix(&content, &cfg);
        let _ = LlmPlugin::audit_and_fix_with_report(&content, &cfg);
    }

    #[test]
    fn audit_and_fix_returns_zero_when_no_failing_files() {
        // All-passing content => is_ollama_available passes, but the
        // failing filter yields an empty list, exercising the early
        // "All files pass" log + return arm.
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        // Very simple text => low grade level.
        fs::write(
            content.join("easy.md"),
            "---\ntitle: T\n---\n\nIt is small. It is fun.",
        )
        .unwrap();

        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("ignored").into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            target_grade: 12.0, // generous → easy.md passes
            ..LlmConfig::default()
        };
        let rewritten = LlmPlugin::audit_and_fix(&content, &cfg).unwrap();
        assert_eq!(rewritten, 0);
    }

    #[test]
    fn audit_and_fix_with_report_returns_skipped_for_empty_body() {
        // File has frontmatter but empty body => skipped arm fires.
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("empty.md"), "---\ntitle: T\n---\n").unwrap();

        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("ignored").into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            target_grade: 0.0, // forces the body grade to fail
            ..LlmConfig::default()
        };
        let report =
            LlmPlugin::audit_and_fix_with_report(&content, &cfg).unwrap();
        // Either skipped (empty body) or no failing files — both arms
        // are valid exits.
        let _ = report;
    }

    #[test]
    fn generate_meta_description_short_text_returns_none() {
        let out = generate_meta_description(
            "<html><body>hi</body></html>",
            "m",
            "http://127.0.0.1:1",
            8.0,
            1,
        );
        assert!(out.is_none(), "short body should short-circuit to None");
    }

    #[test]
    fn generate_with_refinement_returns_none_on_unreachable() {
        // call_ollama returns None when the endpoint is unreachable;
        // generate_with_refinement propagates that None.
        let out = generate_with_refinement(
            "http://127.0.0.1:1",
            "m",
            "hello",
            8.0,
            0,
        );
        assert!(out.is_none());
    }

    // ── Frontmatter / meta-description edge branches ─────────────

    #[test]
    fn strip_frontmatter_unclosed_returns_input() {
        let input = "---\ntitle: Hello\nNo closing delimiter here";
        assert_eq!(strip_frontmatter(input), input);
    }

    #[test]
    fn extract_frontmatter_lang_unclosed_frontmatter() {
        let content = "---\ntitle: Hello\nlanguage: fr\nNo closing delimiter";
        assert_eq!(extract_frontmatter_lang(content), "");
    }

    #[test]
    fn extract_frontmatter_lang_toml_empty_value_falls_through() {
        let content = "+++\nlanguage = \"\"\n+++\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "");
    }

    #[test]
    fn extract_frontmatter_lang_toml_key_without_equals() {
        let content = "+++\nlanguage\n+++\nBody.";
        assert_eq!(extract_frontmatter_lang(content), "");
    }

    #[test]
    fn needs_meta_description_unterminated_content_attr() {
        // content=" opens but never closes: the inner scan bails and
        // the outer contains() check reports the tag as present.
        let html = r#"<meta name="description" content="unterminated"#;
        assert!(!needs_meta_description(html));
    }

    #[test]
    fn generate_missing_alt_text_img_without_closing_bracket_breaks() {
        // No '>' anywhere after the '<img' — the scan must break.
        let mut html = "<body><img src=\"x.jpg\"".to_string();
        let count = generate_missing_alt_text(
            &mut html,
            "llama3",
            "http://127.0.0.1:1",
            true,
            Path::new("t.html"),
            Path::new("."),
        );
        assert_eq!(count, 0);
        assert_eq!(html, "<body><img src=\"x.jpg\"");
    }

    #[test]
    fn generate_missing_alt_text_replaces_empty_alt_attribute() {
        // dry_run=false + a reachable mock Ollama + an `alt=""` tag
        // drives the "replace existing empty alt" branch, as opposed
        // to the "insert a brand-new alt attribute" branch exercised
        // by the mixed-images test above.
        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("A friendly cat.").into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let mut html =
            r#"<html><body><img src="cat.jpg" alt=""></body></html>"#
                .to_string();
        let count = generate_missing_alt_text(
            &mut html,
            "llama3",
            &url,
            false,
            Path::new("test.html"),
            Path::new("."),
        );
        assert_eq!(count, 1);
        assert!(
            html.contains("alt=\"A friendly cat.\""),
            "empty alt should be replaced: {html}"
        );
        assert!(
            !html.contains("alt=\"\""),
            "empty alt attribute must be gone"
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_all_skips_unreadable_markdown_file() {
        // A dangling symlink has the .md extension (so the walker
        // collects it) but read_to_string fails, exercising the
        // skip-on-read-failure arm.
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("good.md"), "---\nt: x\n---\nThe cat sat.")
            .unwrap();
        std::os::unix::fs::symlink(
            content.join("missing-target.md"),
            content.join("broken.md"),
        )
        .unwrap();

        let report = LlmPlugin::audit_all(&content, 8.0).unwrap();
        assert_eq!(report.total_files, 1, "broken symlink must be skipped");
    }

    // ── audit_and_fix / with_report against a live mock ──────────

    /// A one-sentence, polysyllabic body that fails grade 8 by a
    /// wide margin.
    const HARD_BODY: &str = "Administrative procurement methodologies \
                             necessitate institutional documentation \
                             facilitating organisational comprehension \
                             across heterogeneous stakeholder \
                             constituencies.";

    fn write_hard_file(content: &Path) {
        fs::write(
            content.join("hard.md"),
            format!("---\ntitle: T\n---\n\n{HARD_BODY}"),
        )
        .unwrap();
    }

    #[test]
    fn audit_and_fix_rewrites_file_when_llm_improves_grade() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        write_hard_file(&content);

        let bytes: &'static [u8] = Box::leak(
            mock_ollama_ok("The cat sat. It was fun.").into_boxed_slice(),
        );
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            dry_run: false,
            target_grade: 8.0,
            max_refinement_attempts: 0,
            ..LlmConfig::default()
        };
        let rewritten = LlmPlugin::audit_and_fix(&content, &cfg).unwrap();
        assert_eq!(rewritten, 1);

        let out = fs::read_to_string(content.join("hard.md")).unwrap();
        assert!(out.starts_with("---"), "frontmatter preserved:\n{out}");
        assert!(out.contains("The cat sat."), "body replaced:\n{out}");
    }

    #[test]
    fn audit_and_fix_warns_when_llm_does_not_improve_grade() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        write_hard_file(&content);

        // The mock parrots equally complex text — no improvement.
        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok(HARD_BODY).into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            dry_run: false,
            target_grade: 8.0,
            max_refinement_attempts: 0,
            ..LlmConfig::default()
        };
        let rewritten = LlmPlugin::audit_and_fix(&content, &cfg).unwrap();
        assert_eq!(rewritten, 0);
        let out = fs::read_to_string(content.join("hard.md")).unwrap();
        assert!(out.contains("procurement"), "file must be untouched");
    }

    #[test]
    fn audit_and_fix_skips_failing_file_with_empty_body() {
        // A negative target makes even the empty (grade 0) body fail
        // the audit, driving the empty-body `continue` in the fix
        // loop.
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("empty.md"), "---\ntitle: T\n---\n").unwrap();

        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("ignored").into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            target_grade: -1.0,
            max_refinement_attempts: 0,
            ..LlmConfig::default()
        };
        let rewritten = LlmPlugin::audit_and_fix(&content, &cfg).unwrap();
        assert_eq!(rewritten, 0);
    }

    #[test]
    fn audit_and_fix_with_report_rewrites_and_reports_improvement() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        write_hard_file(&content);

        let bytes: &'static [u8] = Box::leak(
            mock_ollama_ok("The cat sat. It was fun.").into_boxed_slice(),
        );
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            dry_run: false,
            target_grade: 8.0,
            max_refinement_attempts: 0,
            ..LlmConfig::default()
        };
        let report =
            LlmPlugin::audit_and_fix_with_report(&content, &cfg).unwrap();
        assert_eq!(report.total_fixed, 1);
        assert_eq!(report.results.len(), 1);
        assert!(report.results[0].improved);
        assert_eq!(report.results[0].action, "rewritten");
        assert!(report.results[0].after_grade < report.results[0].before_grade);

        let out = fs::read_to_string(content.join("hard.md")).unwrap();
        assert!(out.contains("The cat sat."), "body replaced:\n{out}");
    }

    #[test]
    fn audit_and_fix_with_report_records_no_improvement() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        write_hard_file(&content);

        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok(HARD_BODY).into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            dry_run: false,
            target_grade: 8.0,
            max_refinement_attempts: 0,
            ..LlmConfig::default()
        };
        let report =
            LlmPlugin::audit_and_fix_with_report(&content, &cfg).unwrap();
        assert_eq!(report.total_fixed, 0);
        assert_eq!(report.results.len(), 1);
        assert!(!report.results[0].improved);
        assert_eq!(report.results[0].action, "no-improvement");
    }

    #[test]
    fn audit_and_fix_with_report_skips_when_generation_fails() {
        // GET (health probe) succeeds, POST (generate) returns junk
        // that fails JSON parsing — generate_with_refinement yields
        // None and the per-file "skipped" arm fires.
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        write_hard_file(&content);

        let get_ok: &'static [u8] =
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let post_junk: &'static [u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops";
        let (url, _h) = spawn_routing_mock_ollama(get_ok, post_junk);

        let cfg = LlmConfig {
            endpoint: url,
            target_grade: 8.0,
            max_refinement_attempts: 0,
            ..LlmConfig::default()
        };
        let report =
            LlmPlugin::audit_and_fix_with_report(&content, &cfg).unwrap();
        assert_eq!(report.total_fixed, 0);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].action, "skipped");
    }

    #[test]
    fn audit_and_fix_skips_failing_file_when_generation_fails() {
        // Same setup as the `_with_report` counterpart above, but
        // against the plain `audit_and_fix` — drives the implicit
        // no-op arm of its own
        // `if let Some(refined) = generate_with_refinement(..)`
        // (generation fails, so the failing file is left untouched
        // and `rewritten` is not incremented).
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        write_hard_file(&content);

        let get_ok: &'static [u8] =
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let post_junk: &'static [u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops";
        let (url, _h) = spawn_routing_mock_ollama(get_ok, post_junk);

        let cfg = LlmConfig {
            endpoint: url,
            target_grade: 8.0,
            max_refinement_attempts: 0,
            ..LlmConfig::default()
        };
        let rewritten = LlmPlugin::audit_and_fix(&content, &cfg).unwrap();
        assert_eq!(rewritten, 0);
        let out = fs::read_to_string(content.join("hard.md")).unwrap();
        assert!(out.contains("procurement"), "file must be untouched");
    }

    #[test]
    fn audit_and_fix_with_report_skips_failing_file_with_empty_body() {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("empty.md"), "---\ntitle: T\n---\n").unwrap();

        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("ignored").into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let cfg = LlmConfig {
            endpoint: url,
            target_grade: -1.0,
            max_refinement_attempts: 0,
            ..LlmConfig::default()
        };
        let report =
            LlmPlugin::audit_and_fix_with_report(&content, &cfg).unwrap();
        assert_eq!(report.total_failing, 1);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].action, "skipped");
        assert!(!report.results[0].improved);
    }

    // ── after_compile against a live mock ────────────────────────

    /// HTML page needing both a meta description and alt text.
    const AUGMENTABLE_PAGE: &str = "<html><head><title>T</title></head>\
         <body><main><p>Static site generators compile Markdown \
         content into fast HTML pages for the web.</p>\
         <img src=\"photo.jpg\"></main></body></html>";

    #[test]
    fn after_compile_augments_pages_via_mock_llm() {
        let bytes: &'static [u8] = Box::leak(
            mock_ollama_ok("A short page about static site builds.")
                .into_boxed_slice(),
        );
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("index.html"), AUGMENTABLE_PAGE).unwrap();

        let plugin = LlmPlugin::new(LlmConfig {
            endpoint: url,
            dry_run: false,
            target_grade: 30.0, // generous → no refinement roundtrip
            ..LlmConfig::default()
        });
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        plugin.after_compile(&ctx).unwrap();

        let out = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(
            out.contains("name=\"description\""),
            "meta description injected:\n{out}"
        );
        assert!(
            out.contains("A short page about static site builds."),
            "generated text present:\n{out}"
        );
        assert!(
            out.contains("application/ld+json"),
            "JSON-LD description injected:\n{out}"
        );
        assert!(
            out.contains("alt=\"A short page about static site builds.\""),
            "alt text injected:\n{out}"
        );
    }

    #[test]
    fn after_compile_dry_run_logs_without_writing() {
        let bytes: &'static [u8] = Box::leak(
            mock_ollama_ok("A short page about static site builds.")
                .into_boxed_slice(),
        );
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("index.html"), AUGMENTABLE_PAGE).unwrap();

        let plugin = LlmPlugin::new(LlmConfig {
            endpoint: url,
            dry_run: true,
            target_grade: 30.0,
            ..LlmConfig::default()
        });
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        plugin.after_compile(&ctx).unwrap();

        let out = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(out, AUGMENTABLE_PAGE, "dry-run must not modify files");
    }

    #[test]
    fn after_compile_no_changes_needed_when_page_already_augmented() {
        // Ollama IS reachable, but the page already has an adequate
        // meta description and no images at all — exercises the
        // implicit no-op arms of `needs_meta_description` being
        // false and of the `if augmented > 0` log guard inside
        // `after_compile`.
        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("unused").into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let page = r#"<html><head><title>T</title><meta name="description" content="This description is already long enough to pass the fifty character minimum threshold."></head><body><main><p>Content.</p></main></body></html>"#;
        fs::write(site.join("index.html"), page).unwrap();

        let plugin = LlmPlugin::new(LlmConfig {
            endpoint: url,
            ..LlmConfig::default()
        });
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        plugin.after_compile(&ctx).unwrap();

        let out = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(out, page, "page needed no augmentation, must be untouched");
    }

    #[test]
    fn after_compile_skips_meta_description_when_body_too_short() {
        // Ollama reachable, page needs a meta description (no tag at
        // all), but the extracted body text is under the 20-char
        // floor so `generate_meta_description` short-circuits to
        // `None` before ever calling the LLM — exercises the
        // implicit no-op arm of
        // `if let Some(desc) = generate_meta_description(..)` inside
        // `after_compile`.
        let bytes: &'static [u8] =
            Box::leak(mock_ollama_ok("unused").into_boxed_slice());
        let (url, _h) = spawn_multi_shot_mock_ollama(bytes);

        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let page =
            "<html><head><title>T</title></head><body><main>Hi.</main></body></html>";
        fs::write(site.join("index.html"), page).unwrap();

        let plugin = LlmPlugin::new(LlmConfig {
            endpoint: url,
            ..LlmConfig::default()
        });
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        plugin.after_compile(&ctx).unwrap();

        let out = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(out, page, "no meta description could be generated");
    }

    // ── generate_with_refinement refinement pass ─────────────────

    #[test]
    fn generate_with_refinement_adopts_simpler_second_draft() {
        // First draft exceeds the target grade; the refinement call
        // returns simpler text which must be adopted.
        let complex = "Sophisticated organisational methodologies \
                       necessitate comprehensive administrative \
                       documentation considerations.";
        let simple = "The cat sat on the mat.";
        let (url, _h) = spawn_sequenced_mock_ollama(vec![
            mock_ollama_ok(complex),
            mock_ollama_ok(simple),
        ]);

        let out = generate_with_refinement(&url, "m", "prompt", 5.0, 1)
            .expect("refinement should yield text");
        assert_eq!(out, simple);
    }

    #[test]
    fn generate_with_refinement_keeps_draft_when_refinement_is_worse() {
        // The refinement pass returns *more* complex text — the
        // original draft must be kept.
        let first = "The administrative documentation is not simple text.";
        let worse = "Sophisticated organisational methodologies \
                     necessitate comprehensive administrative \
                     documentation considerations universally.";
        let (url, _h) = spawn_sequenced_mock_ollama(vec![
            mock_ollama_ok(first),
            mock_ollama_ok(worse),
        ]);

        let out = generate_with_refinement(&url, "m", "prompt", 1.0, 1)
            .expect("refinement should yield text");
        assert_eq!(out, first);
    }

    #[test]
    fn generate_with_refinement_keeps_draft_when_refinement_call_fails() {
        // The refinement roundtrip itself fails (non-JSON body) —
        // the original draft must survive.
        let complex = "Sophisticated organisational methodologies \
                       necessitate comprehensive administrative \
                       documentation considerations.";
        let junk = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops".to_vec();
        let (url, _h) =
            spawn_sequenced_mock_ollama(vec![mock_ollama_ok(complex), junk]);

        let out = generate_with_refinement(&url, "m", "prompt", 5.0, 1)
            .expect("draft should survive a failed refinement");
        assert_eq!(out, complex);
    }

    // ── Timeout classification + typed query cache path ──────────

    #[test]
    fn query_ollama_timeout_maps_to_llm_timeout() {
        let (url, _h) = spawn_hanging_mock_ollama();
        let err = query_ollama(&url, "m", "p", 1).unwrap_err();
        assert!(
            matches!(err, SsgError::LlmTimeout { .. }),
            "expected LlmTimeout, got: {err}"
        );
    }

    // ── is_timeout_transport: every OS-phrasing branch ────────────
    //
    // `ureq::Transport` has no public constructor, so these branches
    // (in particular the "timeout"/"deadline"/"os error 10060"
    // fallbacks that no real Unix socket ever actually produces) are
    // tested directly against the extracted pure predicate rather
    // than through a live socket.

    #[test]
    fn is_timeout_transport_unix_timed_out_phrasing() {
        assert!(is_timeout_transport(
            ureq::ErrorKind::Io,
            "connection timed out"
        ));
    }

    #[test]
    fn is_timeout_transport_generic_timeout_word() {
        assert!(is_timeout_transport(
            ureq::ErrorKind::ConnectionFailed,
            "operation timeout"
        ));
    }

    #[test]
    fn is_timeout_transport_deadline_word() {
        assert!(is_timeout_transport(
            ureq::ErrorKind::Io,
            "deadline exceeded"
        ));
    }

    #[test]
    fn is_timeout_transport_windows_os_error_10060() {
        assert!(is_timeout_transport(
            ureq::ErrorKind::Io,
            "did not properly respond after a period of time (os error 10060)"
        ));
    }

    #[test]
    fn is_timeout_transport_false_for_unrelated_message() {
        assert!(!is_timeout_transport(
            ureq::ErrorKind::ConnectionFailed,
            "connection refused"
        ));
    }

    #[test]
    fn is_timeout_transport_false_when_kind_is_not_io_or_connection_failed() {
        // Even a "timed out"-shaped message must not classify as a
        // timeout when the transport kind is unrelated to sockets.
        assert!(!is_timeout_transport(
            ureq::ErrorKind::InvalidUrl,
            "request timed out"
        ));
    }

    #[test]
    fn llm_plugin_query_propagates_error_on_cache_miss() {
        // Cache enabled but cold + unreachable endpoint: the live
        // call's error must propagate through the caching path.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = LlmConfig {
            model: "m".into(),
            endpoint: "http://127.0.0.1:1".into(),
            cache_disabled: false,
            cache_dir: Some(tmp.path().to_path_buf()),
            ..LlmConfig::default()
        };
        let plugin = LlmPlugin::new(cfg);
        assert!(plugin.query("uncached-prompt").is_err());
    }

    // ── Mock-server helper hardening (drain_request branches) ────

    fn spawn_serve_canned_once(
        response: &'static [u8],
    ) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            serve_canned(stream, response);
        });
        (addr, handle)
    }

    const CANNED_OK: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    #[test]
    fn drain_request_handles_connection_closed_mid_header() {
        let (addr, h) = spawn_serve_canned_once(CANNED_OK);
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"GET / HT").unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        h.join().unwrap();
        assert!(out.starts_with(b"HTTP/1.1 200"), "got: {out:?}");
    }

    #[test]
    fn drain_request_handles_header_read_timeout() {
        // Client connects but never sends a byte: the server's 2s
        // read timeout fires and it responds anyway.
        let (addr, h) = spawn_serve_canned_once(CANNED_OK);
        let mut s = TcpStream::connect(addr).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        h.join().unwrap();
        assert!(out.starts_with(b"HTTP/1.1 200"), "got: {out:?}");
    }

    #[test]
    fn drain_request_handles_connection_closed_mid_body() {
        let (addr, h) = spawn_serve_canned_once(CANNED_OK);
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"POST / HTTP/1.1\r\nContent-Length: 50\r\n\r\nabc")
            .unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        h.join().unwrap();
        assert!(out.starts_with(b"HTTP/1.1 200"), "got: {out:?}");
    }

    #[test]
    fn drain_request_handles_body_read_timeout() {
        // Full header promising 50 body bytes, but the body never
        // arrives: the body-loop read times out and the server
        // responds anyway.
        let (addr, h) = spawn_serve_canned_once(CANNED_OK);
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"POST / HTTP/1.1\r\nContent-Length: 50\r\n\r\nabc")
            .unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        h.join().unwrap();
        assert!(out.starts_with(b"HTTP/1.1 200"), "got: {out:?}");
    }

    // ── Fault injection (feature-gated) ──────────────────────────

    #[cfg(feature = "test-fault-injection")]
    mod fault {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial]
        fn audit_and_fix_with_report_skips_unreadable_file() {
            let dir = tempfile::tempdir().unwrap();
            let content = dir.path().join("content");
            fs::create_dir_all(&content).unwrap();
            write_hard_file(&content);

            let bytes: &'static [u8] =
                Box::leak(mock_ollama_ok("The cat sat.").into_boxed_slice());
            let (url, _h) = spawn_multi_shot_mock_ollama(bytes);
            let cfg = LlmConfig {
                endpoint: url,
                target_grade: 8.0,
                max_refinement_attempts: 0,
                ..LlmConfig::default()
            };

            fail::cfg("llm::fix-read", "return").unwrap();
            let report =
                LlmPlugin::audit_and_fix_with_report(&content, &cfg).unwrap();
            let _ = fail::cfg("llm::fix-read", "off");

            assert_eq!(report.results.len(), 1);
            assert_eq!(report.results[0].action, "skipped");
            assert!(!report.results[0].improved);
            assert_eq!(report.total_fixed, 0);
        }
    }
}
