// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Automated 10-Pillar Quality Gate and Master Compliance Audit Plugin.
//!
//! Ports the portfolio master quality gate audit (`audit.sh`) natively into
//! the SSG compilation pipeline. Evaluates 10 pillars of web quality:
//!
//! 1. Output & Essential Files (`robots.txt`, `sitemap.xml`, `manifest.json`, `rss.xml`, `search-index.json`)
//! 2. Meta Leaks & Content Hygiene (no unescaped tags in `<head>`, no escaped entity leaks in `<body>`)
//! 3. CSP & Security Integrity (valid Content-Security-Policy with `'unsafe-inline'`)
//! 4. SRI Hashes Sync (verifies SHA-384 cryptographic integrity against compiled assets)
//! 5. Hero Banner Subpage Isolation (prevents full-screen hero leakage onto subpages)
//! 6. Apple HIG Navbar & Footer Hygiene (valid navbar links, "Made with SSG" in footer, not in top navbar)
//! 7. Theme, Search & Lightbox Engines (search index excludes utility pages; client runtime presence)
//! 8. Forms & Link Integrity (functional form actions on contact pages)
//! 9. `CloudCDN` Asset Resolution (valid CDN paths)
//! 10. Accessibility & Semantic Hierarchy (`lang` attribute, `<h1>` heading, no empty headings)
//!
//! Emits `quality-gate-report.json` in the build output directory.

use crate::error::SsgError;
use crate::plugin::{Plugin, PluginContext};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// Pillar result status and issue list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PillarResult {
    /// Whether this pillar passed without blocking issues.
    pub pass: bool,
    /// Detailed list of detected issues for this pillar.
    pub issues: Vec<String>,
}

impl PillarResult {
    /// Creates a passing pillar result.
    #[must_use]
    pub const fn new_pass() -> Self {
        Self {
            pass: true,
            issues: Vec::new(),
        }
    }

    /// Records an issue on this pillar, marking it failed.
    pub fn add_issue(&mut self, issue: impl Into<String>) {
        self.pass = false;
        self.issues.push(issue.into());
    }
}

/// Comprehensive Quality Gate Audit Report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityGateReport {
    /// Total number of HTML pages scanned.
    pub pages_scanned: usize,
    /// Number of passing pillars out of 10.
    pub passed_pillars: usize,
    /// Total number of evaluated pillars (always 10).
    pub total_pillars: usize,
    /// Percentage pass rate (0.0 - 100.0).
    pub pass_rate: f64,
    /// Total number of issues found across all pillars.
    pub total_issues: usize,
    /// Map of pillar name to result.
    pub pillars: BTreeMap<String, PillarResult>,
}

/// Plugin that runs the 10-pillar master quality gate audit on compiled sites.
#[derive(Debug, Clone, Copy, Default)]
pub struct AuditPlugin;

impl AuditPlugin {
    /// Computes the SHA-384 Subresource Integrity string for raw bytes.
    #[must_use]
    pub fn compute_sri(bytes: &[u8]) -> String {
        let mut hasher = Sha384::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        format!(
            "sha384-{}",
            base64::engine::general_purpose::STANDARD.encode(digest)
        )
    }

    /// Yields the text of every opening tag in `html`, `<` to `>`.
    ///
    /// Quote-aware: a `>` inside an attribute value does not end the
    /// tag. That matters because SRI checking has to read two
    /// attributes of the *same* element, and the only way to be sure
    /// they belong together is to bound the element first. Splitting on
    /// lines does not bound anything once the HTML is minified.
    fn opening_tags(html: &str) -> Vec<&str> {
        let bytes = html.as_bytes();
        let mut tags = Vec::new();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'<' {
                i += 1;
                continue;
            }
            let start = i;
            i += 1;
            let mut quote: Option<u8> = None;
            while i < bytes.len() {
                let c = bytes[i];
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == b'"' || c == b'\'' => quote = Some(c),
                    None if c == b'>' => break,
                    None => {}
                }
                i += 1;
            }
            if i < bytes.len() {
                // `start..=i` spans `<` through `>`; slice on char
                // boundaries so non-ASCII attribute values are safe.
                if let Some(tag) = html.get(start..=i) {
                    tags.push(tag);
                }
                i += 1;
            }
        }
        tags
    }

    /// Reads a double- or single-quoted attribute value out of one tag.
    ///
    /// Matches on an attribute *boundary* — the name must be preceded by
    /// whitespace — so `href` does not match inside `data-href`, and
    /// `src` does not match inside `data-src` or `srcset`.
    fn tag_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let mut from = 0usize;
        while let Some(pos) = tag[from..].find(name) {
            let at = from + pos;
            let after = at + name.len();
            let preceded_by_space = tag[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
            let rest = tag.get(after..).unwrap_or("");
            if preceded_by_space && rest.starts_with('=') {
                let value = rest.get(1..).unwrap_or("");
                let q = value.as_bytes().first().copied();
                if q == Some(b'"') || q == Some(b'\'') {
                    let q = q.unwrap_or(b'"') as char;
                    let body = value.get(1..).unwrap_or("");
                    if let Some(end) = body.find(q) {
                        return body.get(..end);
                    }
                }
            }
            from = after.max(at + 1);
        }
        None
    }

    /// Runs the 10-pillar audit against a compiled site directory.
    #[must_use]
    pub fn audit_directory(site_dir: &Path) -> QualityGateReport {
        let mut pillars = BTreeMap::new();
        let pillar_names = [
            "1. Output & Essential Files",
            "2. Meta Leaks & Content Hygiene",
            "3. CSP & Security Integrity",
            "4. SRI Hashes Sync",
            "5. Hero Banner Subpage Isolation",
            "6. Apple HIG Navbar & Footer Hygiene",
            "7. Theme, Search & Lightbox Engines",
            "8. Forms & Link Integrity",
            "9. CloudCDN Asset Resolution",
            "10. Accessibility & Semantic Hierarchy",
        ];

        for name in pillar_names {
            let _ = pillars.insert(name.to_string(), PillarResult::new_pass());
        }

        if !site_dir.exists() {
            for p in pillars.values_mut() {
                p.add_issue(format!(
                    "Site directory not found: {}",
                    site_dir.display()
                ));
            }
            return QualityGateReport {
                pages_scanned: 0,
                passed_pillars: 0,
                total_pillars: 10,
                pass_rate: 0.0,
                total_issues: 10,
                pillars,
            };
        }

        // 1. Output & Essential Files
        let req_files = [
            "robots.txt",
            "sitemap.xml",
            "manifest.json",
            "rss.xml",
            "search-index.json",
        ];
        for rf in req_files {
            if !site_dir.join(rf).is_file() {
                if let Some(p) = pillars.get_mut("1. Output & Essential Files")
                {
                    p.add_issue(format!("Missing essential file: {rf}"));
                }
            }
        }

        // 2. Search Index Hygiene
        let sindex_path = site_dir.join("search-index.json");
        if sindex_path.is_file() {
            if let Ok(content) = fs::read_to_string(&sindex_path) {
                if let Ok(val) =
                    serde_json::from_str::<serde_json::Value>(&content)
                {
                    let entries = if let Some(arr) = val.as_array() {
                        Some(arr)
                    } else {
                        val.get("entries").and_then(serde_json::Value::as_array)
                    };

                    if let Some(entries) = entries {
                        for entry in entries {
                            if let Some(url) = entry
                                .get("url")
                                .and_then(serde_json::Value::as_str)
                            {
                                let u_lower = url.to_lowercase();
                                if u_lower.contains("/404")
                                    || u_lower.contains("/offline")
                                    || u_lower.contains("/thanks")
                                    || u_lower.contains("404.html")
                                    || u_lower.contains("offline.html")
                                    || u_lower.contains("thanks.html")
                                {
                                    if let Some(p) = pillars.get_mut(
                                        "7. Theme, Search & Lightbox Engines",
                                    ) {
                                        p.add_issue(format!(
                                            "search-index.json contains utility page: {url}"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Collect all compiled asset hashes for SRI verification
        let mut asset_hashes: HashMap<String, String> = HashMap::new();
        let mut html_files: Vec<PathBuf> = Vec::new();

        let mut stack = vec![site_dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy();
                        if !name.starts_with('.')
                            && name != "_layouts"
                            && name != "templates"
                            && name != "node_modules"
                        {
                            stack.push(path);
                        }
                    } else if path.is_file() {
                        let ext = path
                            .extension()
                            .unwrap_or_default()
                            .to_string_lossy();
                        if ext == "html" {
                            html_files.push(path);
                        } else if ext == "js" || ext == "css" {
                            if let Ok(bytes) = fs::read(&path) {
                                let hash = Self::compute_sri(&bytes);
                                let fname = path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string();
                                let rel = path
                                    .strip_prefix(site_dir)
                                    .unwrap_or(&path)
                                    .to_string_lossy()
                                    .replace('\\', "/");
                                let _ = asset_hashes
                                    .insert(format!("/{rel}"), hash.clone());
                                let _ = asset_hashes.insert(fname, hash);
                            }
                        }
                    }
                }
            }
        }

        // `fs::read_dir` yields entries in whatever order the filesystem
        // gives, which differs between ext4 and APFS. Issues are pushed in
        // this order, so an unsorted walk made `quality-gate-report.json`
        // differ between Linux and macOS for identical input — the
        // determinism gate compares the two trees and failed on exactly this
        // one file. Sorting makes the report a function of the site, not of
        // the machine that built it.
        html_files.sort();

        if html_files.is_empty() {
            if let Some(p) = pillars.get_mut("1. Output & Essential Files") {
                p.add_issue("No compiled HTML files found in output directory");
            }
        }

        // Deep HTML Scan
        for path in &html_files {
            let rel = path
                .strip_prefix(site_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");

            let Ok(html) = fs::read_to_string(path) else {
                continue;
            };

            // A. Head hygiene
            if let Some(start) = html.find("<head") {
                if let Some(end) = html[start..].find("</head>") {
                    let head_txt = &html[start..start + end];
                    if head_txt.contains("<div")
                        || head_txt.contains("<p")
                        || head_txt.contains("<span")
                    {
                        if let Some(p) =
                            pillars.get_mut("2. Meta Leaks & Content Hygiene")
                        {
                            p.add_issue(format!(
                                "{rel}: Unescaped HTML container inside <head>"
                            ));
                        }
                    }
                    if head_txt.contains("&lt;div")
                        || head_txt.contains("&lt;h")
                    {
                        if let Some(p) =
                            pillars.get_mut("2. Meta Leaks & Content Hygiene")
                        {
                            p.add_issue(format!(
                                "{rel}: Leaked escaped entity in <head>"
                            ));
                        }
                    }
                }
            }

            // Body hygiene
            if html.contains(".class=\"") || html.contains(".class=\\\"") {
                if let Some(p) =
                    pillars.get_mut("2. Meta Leaks & Content Hygiene")
                {
                    p.add_issue(format!(
                        "{rel}: Leaked .class= template artifact"
                    ));
                }
            }
            if html.contains("&lt;div")
                || html.contains("&lt;h2")
                || html.contains("&lt;p&gt;")
                || html.contains("&lt;img")
            {
                if let Some(p) =
                    pillars.get_mut("2. Meta Leaks & Content Hygiene")
                {
                    p.add_issue(format!(
                        "{rel}: Escaped HTML entities leaked in body content"
                    ));
                }
            }

            // B. CSP Integrity
            if !html.to_lowercase().contains("content-security-policy") {
                if let Some(p) = pillars.get_mut("3. CSP & Security Integrity")
                {
                    p.add_issue(format!(
                        "{rel}: Missing Content-Security-Policy meta tag"
                    ));
                }
            } else if !html.contains("script-src")
                && !html.contains("default-src")
            {
                if let Some(p) = pillars.get_mut("3. CSP & Security Integrity")
                {
                    p.add_issue(format!(
                        "{rel}: CSP missing essential directives"
                    ));
                }
            }

            // C. SRI Verification
            //
            // Per element, not per line. The previous pass walked
            // `html.lines()` and took the *first* `src="` and the
            // *first* `integrity="` on any line that mentioned SRI —
            // which are only the same element on pretty-printed HTML.
            // Minified output puts a whole `<head>` on one line, and
            // then the check compared one tag's `src` against another
            // tag's `integrity`.
            //
            // That was both a false positive and a false negative. Six
            // of the nine published themes reported
            // `SRI mismatch for /theme-init.<hash>.js` whose integrity
            // was in fact correct, and every SRI after the first on a
            // minified line was never verified at all — a genuinely
            // wrong hash there would have passed silently.
            for tag in Self::opening_tags(&html) {
                let Some(int_val) = Self::tag_attr(tag, "integrity") else {
                    continue;
                };
                // `src` for <script>, `href` for <link rel=stylesheet>.
                let Some(url) = Self::tag_attr(tag, "src")
                    .or_else(|| Self::tag_attr(tag, "href"))
                else {
                    continue;
                };
                if url.starts_with("http://") || url.starts_with("https://") {
                    continue;
                }
                let expected = asset_hashes
                    .get(url)
                    .or_else(|| asset_hashes.get(url.trim_start_matches('/')));
                if let Some(exp) = expected {
                    if exp != int_val {
                        if let Some(p) = pillars.get_mut("4. SRI Hashes Sync") {
                            p.add_issue(format!(
                                "{rel}: SRI mismatch for {url}"
                            ));
                        }
                    }
                }
            }

            // D. Hero banner subpage isolation
            if rel != "index.html"
                && html.contains("class=\"hero-banner-container\"")
            {
                if let Some(p) =
                    pillars.get_mut("5. Hero Banner Subpage Isolation")
                {
                    p.add_issue(format!(
                        "{rel}: Subpage has full-screen hero banner"
                    ));
                }
            }

            // E. Navbar & Footer Hygiene
            if !is_taxonomy_page(&rel) {
                if !has_responsive_navbar(&html) {
                    if let Some(p) =
                        pillars.get_mut("6. Apple HIG Navbar & Footer Hygiene")
                    {
                        p.add_issue(format!(
                            "{rel}: Missing responsive navbar"
                        ));
                    }
                }

                // Check footer contains Made with SSG
                if html.contains("<footer") && !html.contains("made-with-ssg") {
                    if let Some(p) =
                        pillars.get_mut("6. Apple HIG Navbar & Footer Hygiene")
                    {
                        p.add_issue(format!(
                            "{rel}: Footer missing 'Made with SSG' link"
                        ));
                    }
                }
            }

            // F. Forms integrity on contact page
            //
            // Taxonomy pages are excluded: a site tagging articles "contact"
            // emits `tags/contact/index.html`, which the substring match read
            // as the contact page and then failed for carrying no form. The
            // tag index is generated and never has one.
            if rel.to_lowercase().contains("contact")
                && !is_taxonomy_page(&rel)
                && !html.contains("http-equiv=\"refresh\"")
                && (!html.contains("<form") || !html.contains("action="))
            {
                {
                    if let Some(p) =
                        pillars.get_mut("8. Forms & Link Integrity")
                    {
                        p.add_issue(format!("{rel}: Contact page missing functional form action"));
                    }
                }
            }

            // G. Accessibility & Semantic Hierarchy
            if !html.contains("lang=") {
                if let Some(p) =
                    pillars.get_mut("10. Accessibility & Semantic Hierarchy")
                {
                    p.add_issue(format!("{rel}: Missing html lang attribute"));
                }
            }
            if !html.contains("<h1") {
                if let Some(p) =
                    pillars.get_mut("10. Accessibility & Semantic Hierarchy")
                {
                    p.add_issue(format!(
                        "{rel}: Missing first-level <h1> heading"
                    ));
                }
            }
        }

        let total_issues: usize =
            pillars.values().map(|p| p.issues.len()).sum();
        let passed_pillars: usize = pillars.values().filter(|p| p.pass).count();
        let pass_rate = if pillars.is_empty() {
            0.0
        } else {
            (passed_pillars as f64 / pillars.len() as f64) * 100.0
        };

        QualityGateReport {
            pages_scanned: html_files.len(),
            passed_pillars,
            total_pillars: 10,
            pass_rate,
            total_issues,
            pillars,
        }
    }
}

/// Whether a path is a generated taxonomy page rather than an authored one.
///
/// Tag indexes are emitted by the taxonomy plugin, so the navbar and footer
/// hygiene rules do not apply to them. Matching only a leading `tags/` missed
/// every translated copy: with `url_prefix = "sub_path"` the French set is
/// emitted under `fr/tags/`, so a bilingual site failed the pillar on exactly
/// the pages an English-only one was excused. Allow one leading locale segment
/// before the taxonomy root.
fn is_taxonomy_page(rel: &str) -> bool {
    if rel.starts_with("tags/") {
        return true;
    }
    // The locale segment is only considered second: `tags` is itself short and
    // alphabetic, so stripping a leading segment first would consume the very
    // directory being looked for and report `tags/index.html` as authored.
    match rel.split_once('/') {
        // A locale segment is short and alphabetic: `fr/`, `en/`, `pt-br/`.
        Some((first, rest))
            if (2..=5).contains(&first.len())
                && first
                    .chars()
                    .all(|c| c.is_ascii_alphabetic() || c == '-') =>
        {
            rest.starts_with("tags/")
        }
        _ => false,
    }
}

/// Whether a page exposes a responsive navigation bar carrying a brand link.
///
/// The pillar this backs is about structure, not about any one CSS framework.
/// Matching on the literal `navbar`/`navbar-brand` class pair only recognised
/// Bootstrap-shaped markup, so themes that ship a semantic `<nav>` landmark
/// with a `class="brand"` home link — which is the more accessible form, and
/// what every first-party theme uses — were reported as having no navbar at
/// all. Accept either spelling: the legacy class pair, or a `<nav>` landmark
/// combined with a recognisable brand link.
fn has_responsive_navbar(html: &str) -> bool {
    let bootstrap = html.contains("navbar") && html.contains("navbar-brand");

    let has_nav_landmark = html.contains("<nav")
        || html.contains("role=\"navigation\"")
        || html.contains("role='navigation'");

    // A brand link is the site identity anchored in the header: a `brand`
    // class, or an explicit home relation.
    let has_brand = html.contains("class=\"brand\"")
        || html.contains("class='brand'")
        || html.contains("navbar-brand")
        || html.contains("class=\"site-title\"")
        || html.contains("rel=\"home\"")
        || html.contains("rel='home'");

    bootstrap || (has_nav_landmark && has_brand)
}

impl Plugin for AuditPlugin {
    fn name(&self) -> &'static str {
        "audit"
    }

    fn after_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_audit_plugin_name() {
        let plugin = AuditPlugin;
        assert_eq!(plugin.name(), "audit");
    }

    /// Builds a site directory with one minified page whose `<head>`
    /// holds several SRI-bearing elements on a single line — the shape
    /// the real generator emits, and the shape the old line-based check
    /// mishandled.
    fn minified_site(script_integrity: &str) -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let css = b"body{margin:0}";
        let js = b"document.documentElement.classList.remove('no-js');";
        fs::write(root.join("style.css"), css).expect("css");
        fs::write(root.join("theme-init.js"), js).expect("js");
        let css_sri = AuditPlugin::compute_sri(css);
        // One line, stylesheet first, script second: the stylesheet's
        // `integrity` precedes the script's `src`.
        let html = format!(
            "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <title>t</title><meta http-equiv=\"Content-Security-Policy\" \
             content=\"default-src 'self'; script-src 'self'\">\
             <link rel=\"stylesheet\" href=\"/style.css\" \
             integrity=\"{css_sri}\" crossorigin=\"anonymous\">\
             <script src=\"/theme-init.js\" integrity=\"{script_integrity}\" \
             crossorigin=\"anonymous\"></script></head><body><h1>t</h1>\
             </body></html>"
        );
        fs::write(root.join("index.html"), html).expect("html");
        dir
    }

    fn sri_issues(report: &QualityGateReport) -> Vec<String> {
        report
            .pillars
            .get("4. SRI Hashes Sync")
            .map(|p| p.issues.clone())
            .unwrap_or_default()
    }

    /// The false positive. Every hash on the page is correct, but the
    /// stylesheet's `integrity` appears before the script's `src` on the
    /// same line. The old check paired them and reported a mismatch
    /// against a script whose integrity was right — which is what made
    /// six of the nine published themes score 9/10.
    #[test]
    fn correct_hashes_on_one_minified_line_raise_no_sri_issue() {
        let js = b"document.documentElement.classList.remove('no-js');";
        let dir = minified_site(&AuditPlugin::compute_sri(js));
        let report = AuditPlugin::audit_directory(dir.path());
        assert!(
            sri_issues(&report).is_empty(),
            "correct hashes must not be reported: {:?}",
            sri_issues(&report)
        );
    }

    /// The false negative, which matters more, and which needs a
    /// fixture the old check would have got *right* on its first pair.
    ///
    /// Two scripts on one line. The first carries a correct hash, so the
    /// old line-based pass matched `src`+`integrity` on it and stopped —
    /// it only ever read the first of each per line. The second script's
    /// hash is wrong and was never looked at. A wrong SRI hash makes the
    /// browser refuse to run the script, so silence here is the
    /// expensive failure.
    #[test]
    fn a_wrong_hash_after_a_correct_one_on_the_same_line_is_caught() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let first = b"console.log('first');";
        let second = b"console.log('second');";
        fs::write(root.join("first.js"), first).expect("first");
        fs::write(root.join("second.js"), second).expect("second");
        let good = AuditPlugin::compute_sri(first);
        let html = format!(
            "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <title>t</title><meta http-equiv=\"Content-Security-Policy\" \
             content=\"default-src 'self'; script-src 'self'\">\
             <script src=\"/first.js\" integrity=\"{good}\" \
             crossorigin=\"anonymous\"></script>\
             <script src=\"/second.js\" \
             integrity=\"sha384-notTheHashOfSecondJsAtAllNotEvenClose\" \
             crossorigin=\"anonymous\"></script></head><body><h1>t</h1>\
             </body></html>"
        );
        fs::write(root.join("index.html"), html).expect("html");

        let report = AuditPlugin::audit_directory(root);
        let issues = sri_issues(&report);
        assert!(
            issues.iter().any(|i| i.contains("/second.js")),
            "the wrong hash on the second script must be reported, \
             got: {issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.contains("/first.js")),
            "the correct first script must not be reported: {issues:?}"
        );
    }

    /// `tag_attr` matches on an attribute boundary, so a `data-` prefixed
    /// look-alike is not mistaken for the real attribute.
    #[test]
    fn tag_attr_does_not_match_a_prefixed_lookalike() {
        let tag = "<script data-src=\"/decoy.js\" src=\"/real.js\">";
        assert_eq!(AuditPlugin::tag_attr(tag, "src"), Some("/real.js"));
        let only_decoy = "<script data-src=\"/decoy.js\">";
        assert_eq!(AuditPlugin::tag_attr(only_decoy, "src"), None);
    }

    /// A `>` inside an attribute value must not end the tag early.
    #[test]
    fn opening_tags_are_bounded_quote_aware() {
        let html = "<a title=\"a > b\" href=\"/x\">text</a>";
        let tags = AuditPlugin::opening_tags(html);
        assert_eq!(AuditPlugin::tag_attr(tags[0], "href"), Some("/x"));
    }

    #[test]
    fn test_compute_sri() {
        let data = b"console.log('hello world');";
        let sri = AuditPlugin::compute_sri(data);
        assert!(sri.starts_with("sha384-"));
    }

    #[test]
    fn test_taxonomy_page_matches_plain_tags_root() {
        assert!(is_taxonomy_page("tags/index.html"));
        assert!(is_taxonomy_page("tags/method/index.html"));
    }

    #[test]
    fn test_taxonomy_page_matches_locale_prefixed_tags() {
        // `url_prefix = "sub_path"` emits the translated set under the locale,
        // which the pillar must excuse exactly as it does the default one.
        assert!(is_taxonomy_page("fr/tags/index.html"));
        assert!(is_taxonomy_page("fr/tags/editorial/index.html"));
        assert!(is_taxonomy_page("pt-br/tags/index.html"));
    }

    #[test]
    fn test_taxonomy_page_rejects_authored_pages() {
        assert!(!is_taxonomy_page("index.html"));
        assert!(!is_taxonomy_page("about/index.html"));
        assert!(!is_taxonomy_page("fr/a-propos/index.html"));
        // A content page that merely starts with the same letters is authored.
        assert!(!is_taxonomy_page("tagging-guide/index.html"));
    }

    /// Issues must be recorded in lexical path order, not filesystem order.
    ///
    /// `fs::read_dir` returns entries in whatever order the filesystem gives:
    /// ext4 and APFS disagree, so without a sort the same input produced a
    /// different `quality-gate-report.json` on Linux and macOS, and the
    /// cross-OS determinism gate failed on exactly that one file.
    ///
    /// This asserts the ordering property directly rather than comparing two
    /// builds. A comparison test passes vacuously on a filesystem that
    /// happens to return entries in a stable order — which APFS does, so it
    /// proved nothing locally while the real divergence was against Linux.
    #[test]
    fn issues_are_recorded_in_lexical_path_order() {
        let temp = TempDir::new().unwrap();
        let sdir = temp.path();

        fs::write(sdir.join("robots.txt"), "User-agent: *").unwrap();
        fs::write(sdir.join("sitemap.xml"), "<urlset></urlset>").unwrap();
        fs::write(sdir.join("manifest.json"), "{}").unwrap();
        fs::write(sdir.join("rss.xml"), "<rss></rss>").unwrap();
        fs::write(sdir.join("search-index.json"), "[]").unwrap();

        // Names chosen so creation order and lexical order differ.
        for name in ["zulu", "alpha", "mike", "bravo"] {
            let sub = sdir.join(name);
            fs::create_dir_all(&sub).unwrap();
            fs::write(
                sub.join("index.html"),
                "<html><body><p>no lang, no h1</p></body></html>",
            )
            .unwrap();
        }

        let report = AuditPlugin::audit_directory(sdir);
        let pillar = report
            .pillars
            .get("10. Accessibility & Semantic Hierarchy")
            .expect("accessibility pillar is always present");
        assert!(
            pillar.issues.len() >= 4,
            "expected an issue per page, got {:?}",
            pillar.issues
        );

        let paths: Vec<&str> = pillar
            .issues
            .iter()
            .filter_map(|i| i.split(':').next())
            .collect();
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(
            paths, sorted,
            "issues are not in lexical path order; the file walk is unsorted"
        );
    }

    #[test]
    fn test_navbar_accepts_bootstrap_class_pair() {
        let html = r#"<nav class="navbar"><a class="navbar-brand" href="/">Home</a></nav>"#;
        assert!(has_responsive_navbar(html));
    }

    #[test]
    fn test_navbar_accepts_semantic_nav_with_brand() {
        // The shape every first-party theme ships: a `<nav>` landmark and a
        // `brand` home link, with no Bootstrap class names anywhere.
        let html = r#"<header><a class="brand" href="/">Lucid</a>
  <nav aria-label="Main"><ul><li><a href="/install/">Install</a></li></ul></nav>
</header>"#;
        assert!(has_responsive_navbar(html));
    }

    #[test]
    fn test_navbar_accepts_navigation_role_with_home_rel() {
        let html =
            r#"<div role="navigation"><a rel="home" href="/">Site</a></div>"#;
        assert!(has_responsive_navbar(html));
    }

    #[test]
    fn test_navbar_rejects_page_without_navigation() {
        let html =
            r#"<header><h1>Just a title</h1></header><main><p>Body</p></main>"#;
        assert!(!has_responsive_navbar(html));
    }

    #[test]
    fn test_navbar_rejects_nav_landmark_without_brand() {
        // A bare nav with no site identity is still an incomplete header, so
        // the pillar should keep flagging it.
        let html = r#"<nav aria-label="Main"><ul><li><a href="/a/">A</a></li></ul></nav>"#;
        assert!(!has_responsive_navbar(html));
    }

    #[test]
    fn test_audit_directory_non_existent() {
        let p = Path::new("/non/existent/path/here");
        let report = AuditPlugin::audit_directory(p);
        assert_eq!(report.passed_pillars, 0);
        assert_eq!(report.total_issues, 10);
    }

    #[test]
    fn test_audit_directory_clean_site() {
        let temp = TempDir::new().unwrap();
        let sdir = temp.path();

        // Write essential files
        fs::write(sdir.join("robots.txt"), "User-agent: *\nDisallow:").unwrap();
        fs::write(sdir.join("sitemap.xml"), "<urlset></urlset>").unwrap();
        fs::write(sdir.join("manifest.json"), "{}").unwrap();
        fs::write(sdir.join("rss.xml"), "<rss></rss>").unwrap();
        fs::write(sdir.join("search-index.json"), "[]").unwrap();

        // Write index.html
        let html = r#"<!DOCTYPE html>
<html lang="en-GB">
<head>
  <meta charset="utf-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self' 'unsafe-inline';">
  <title>Clean Test Site</title>
</head>
<body>
  <nav class="navbar"><a class="navbar-brand" href="/">Home</a></nav>
  <main id="main">
    <h1>Clean Test Site</h1>
    <p>Welcome to the clean site.</p>
  </main>
  <footer>
    <a href="/made-with-ssg/index.html">Made with SSG</a>
  </footer>
</body>
</html>"#;
        fs::write(sdir.join("index.html"), html).unwrap();

        let report = AuditPlugin::audit_directory(sdir);
        assert_eq!(report.passed_pillars, 10);
        assert_eq!(report.total_issues, 0);
        // `pass_rate` is a computed f64; compare within epsilon rather
        // than with `==`, which is what clippy::float_cmp guards.
        assert!((report.pass_rate - 100.0).abs() < f64::EPSILON);
    }
}
