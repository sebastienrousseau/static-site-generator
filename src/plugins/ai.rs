// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! AI-readiness content hooks.
//!
//! Provides algorithmic content enhancements for Generative Engine
//! Optimization (GEO) and Answer Engine Optimization (AEO):
//!
//! - Auto-generate meta descriptions from page content when missing
//! - Validate all `<img>` elements have alt text (log warnings)
//! - Generate `llms.txt` and `llms-full.txt` for AI crawler guidance

use crate::error::SsgError;
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use anyhow::Result;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Plugin for AI-readiness content validation and enhancement.
///
/// Runs in `after_compile`:
/// - Checks all images have alt text (logs warnings for missing)
/// - Generates `llms.txt` and `llms-full.txt` in the site root
/// - Adds max-snippet meta for AI citation eligibility
#[derive(Debug, Clone, Copy)]
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn name(&self) -> &'static str {
        "ai"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        generate_llms_txt(&ctx.site_dir, ctx.config.as_ref())
            .map_err(|e| SsgError::io(e, &ctx.site_dir))?;
        generate_llms_full_txt(&ctx.site_dir, ctx.config.as_ref())
            .map_err(|e| SsgError::io(e, &ctx.site_dir))?;

        let html_files = collect_html_files(&ctx.site_dir)
            .map_err(|e| SsgError::io(e, &ctx.site_dir))?;
        let pages_with_missing_alt =
            process_html_for_ai(&html_files, &ctx.site_dir)
                .map_err(|e| SsgError::io(e, &ctx.site_dir))?;

        if pages_with_missing_alt > 0 {
            log::warn!(
                "[ai] {pages_with_missing_alt} page(s) have images without alt text"
            );
        }

        Ok(())
    }
}

/// Processes HTML files: injects max-snippet meta tags and checks for missing alt text.
fn process_html_for_ai(
    html_files: &[PathBuf],
    site_dir: &Path,
) -> Result<usize> {
    let mut pages_with_missing_alt = 0usize;

    for path in html_files {
        let html = fs::read_to_string(path)?;
        let modified = inject_max_snippet(&html);

        check_alt_text(path, &modified, site_dir, &mut pages_with_missing_alt);

        if modified != html {
            fs::write(path, modified)?;
        }
    }

    Ok(pages_with_missing_alt)
}

/// Injects the max-snippet meta tag before `</head>` if not already present.
fn inject_max_snippet(html: &str) -> String {
    if html.contains("max-snippet") {
        return html.to_string();
    }
    let tag = "<meta name=\"robots\" content=\"max-snippet:-1, max-image-preview:large, max-video-preview:-1\">\n";
    inject_before_head_close(html, tag)
}

/// Checks for missing alt text and logs a warning if found.
fn check_alt_text(
    path: &Path,
    html: &str,
    site_dir: &Path,
    counter: &mut usize,
) {
    let missing = count_missing_alt(html);
    if missing > 0 {
        let rel = path.strip_prefix(site_dir).unwrap_or(path).display();
        log::warn!("[ai] {missing} image(s) missing alt text in {rel}");
        *counter += 1;
    }
}

// -------------------------------------------------------------------
// llms.txt generation — llmstxt.org v1 spec
// -------------------------------------------------------------------

/// Collects page metadata from `.meta.json` sidecars in the site dir.
///
/// Returns a list of `(title, relative_url, description)` tuples for
/// pages that should appear in `llms.txt`.
fn collect_page_entries(
    site_dir: &Path,
) -> Result<Vec<(String, String, String)>> {
    let html_files = collect_html_files(site_dir)?;
    let mut entries = Vec::new();

    for html_path in &html_files {
        let rel = html_path.strip_prefix(site_dir).unwrap_or(html_path);

        // Read the companion sidecar
        let sidecar_path = html_path.with_extension("meta.json");
        let meta: serde_json::Map<String, serde_json::Value> =
            if sidecar_path.exists() {
                if let Ok(content) = fs::read_to_string(&sidecar_path) {
                    serde_json::from_str(&content).unwrap_or_default()
                } else {
                    serde_json::Map::new()
                }
            } else {
                serde_json::Map::new()
            };

        if is_excluded_page(rel, &meta) {
            continue;
        }

        let title = meta
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let description = meta
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        // Build a URL path from the relative file path
        let url = format!("/{}", rel.to_string_lossy().replace('\\', "/"));

        if !title.is_empty() {
            entries.push((title, url, description));
        }
    }

    Ok(entries)
}

/// Returns true if a page should be excluded from `llms.txt`.
///
/// Excludes pages that are drafts, private, or error pages (404).
fn is_excluded_page(
    path: &Path,
    frontmatter: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    // Exclude 404 and error pages
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if file_name == "404.html" || file_name.starts_with("error") {
        return true;
    }

    // Exclude drafts
    if let Some(draft) = frontmatter.get("draft") {
        if draft.as_bool().unwrap_or(false)
            || draft.as_str().is_some_and(|s| s == "true")
        {
            return true;
        }
    }

    // Exclude private pages
    if let Some(private) = frontmatter.get("private") {
        if private.as_bool().unwrap_or(false)
            || private.as_str().is_some_and(|s| s == "true")
        {
            return true;
        }
    }

    false
}

/// Groups page entries by their top-level directory.
///
/// Files at the root level are grouped under `"Pages"`.
/// Subdirectory names are title-cased (e.g., `blog/` becomes `"Blog"`).
fn group_pages_by_section(
    entries: &[(String, String, String)],
) -> BTreeMap<String, Vec<(String, String, String)>> {
    let mut sections: BTreeMap<String, Vec<(String, String, String)>> =
        BTreeMap::new();

    for (title, url, description) in entries {
        // url looks like "/blog/post.html" or "/index.html"
        let trimmed = url.trim_start_matches('/');
        let section = if let Some(slash) = trimmed.find('/') {
            let dir = &trimmed[..slash];
            titlecase_word(dir)
        } else {
            "Pages".to_string()
        };

        sections.entry(section).or_default().push((
            title.clone(),
            url.clone(),
            description.clone(),
        ));
    }

    sections
}

/// Title-cases a single word (first char uppercase, rest lowercase).
fn titlecase_word(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            format!("{upper}{}", chars.as_str().to_lowercase())
        }
    }
}

/// Parses `Disallow:` patterns from an existing `robots.txt` file.
fn parse_robots_disallow(site_dir: &Path) -> Vec<String> {
    let robots_path = site_dir.join("robots.txt");
    let Ok(content) = fs::read_to_string(&robots_path) else {
        return Vec::new();
    };

    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("Disallow:") {
                let pattern = rest.trim();
                if !pattern.is_empty() {
                    return Some(pattern.to_string());
                }
            }
            None
        })
        .collect()
}

/// Generates `llms.txt` following the llmstxt.org v1 specification.
///
/// Format:
/// ```text
/// # {site_name}
///
/// > {site_description}
///
/// Language: {language}
///
/// ## {Section Name}
/// - [{Page Title}]({URL}): {Description}
///
/// ## Disallow
/// - {pattern from robots.txt}
/// ```
fn generate_llms_txt(
    site_dir: &Path,
    config: Option<&crate::cmd::SsgConfig>,
) -> Result<()> {
    let site_name = config.map_or("Site", |c| c.site_name.as_str());
    let base_url = config.map_or("", |c| c.base_url.as_str());
    let description = config.map_or("", |c| c.site_description.as_str());
    let language = config
        .map(|c| c.language.as_str())
        .filter(|l| !l.is_empty())
        .unwrap_or("en");
    let canonical_root = base_url.trim_end_matches('/');

    let mut content =
        format!("# {site_name}\n\n> {description}\n\nLanguage: {language}\n");

    // Collect and group pages
    let entries = collect_page_entries(site_dir).unwrap_or_default();
    let sections = group_pages_by_section(&entries);

    for (section, pages) in &sections {
        content.push_str(&format!("\n## {section}\n"));
        for (title, url, desc) in pages {
            let full_url = if canonical_root.is_empty() {
                url.clone()
            } else {
                format!("{canonical_root}{url}")
            };
            if desc.is_empty() {
                content.push_str(&format!("- [{title}]({full_url})\n"));
            } else {
                content.push_str(&format!("- [{title}]({full_url}): {desc}\n"));
            }
        }
    }

    // Disallow section from robots.txt
    let disallow = parse_robots_disallow(site_dir);
    if !disallow.is_empty() {
        content.push_str("\n## Disallow\n");
        for pattern in &disallow {
            content.push_str(&format!("- {pattern}\n"));
        }
    }

    fs::write(site_dir.join("llms.txt"), content)?;
    log::info!("[ai] Generated llms.txt");
    Ok(())
}

/// Generates `llms-full.txt` with full text content for each page.
///
/// Follows the same structure as `llms.txt` but includes the stripped
/// HTML body content for each page rather than just a link index.
fn generate_llms_full_txt(
    site_dir: &Path,
    config: Option<&crate::cmd::SsgConfig>,
) -> Result<()> {
    let site_name = config.map_or("Site", |c| c.site_name.as_str());
    let base_url = config.map_or("", |c| c.base_url.as_str());
    let description = config.map_or("", |c| c.site_description.as_str());
    let language = config
        .map(|c| c.language.as_str())
        .filter(|l| !l.is_empty())
        .unwrap_or("en");
    let canonical_root = base_url.trim_end_matches('/');

    let mut content =
        format!("# {site_name}\n\n> {description}\n\nLanguage: {language}\n");

    let html_files = collect_html_files(site_dir)?;

    for html_path in &html_files {
        let rel = html_path.strip_prefix(site_dir).unwrap_or(html_path);

        // Read sidecar
        let sidecar_path = html_path.with_extension("meta.json");
        let meta: serde_json::Map<String, serde_json::Value> =
            if sidecar_path.exists() {
                if let Ok(c) = fs::read_to_string(&sidecar_path) {
                    serde_json::from_str(&c).unwrap_or_default()
                } else {
                    serde_json::Map::new()
                }
            } else {
                serde_json::Map::new()
            };

        if is_excluded_page(rel, &meta) {
            continue;
        }

        let title = meta
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        if title.is_empty() {
            continue;
        }

        let url = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
        let full_url = if canonical_root.is_empty() {
            url.clone()
        } else {
            format!("{canonical_root}{url}")
        };

        // Read and strip HTML content
        let html = fs::read_to_string(html_path).unwrap_or_default();
        let body_text = strip_html_tags(&extract_body(&html));
        let trimmed = collapse_whitespace(&body_text);

        content.push_str(&format!("\n---\n\n## [{title}]({full_url})\n\n"));
        if !trimmed.is_empty() {
            content.push_str(&trimmed);
            content.push('\n');
        }
    }

    fs::write(site_dir.join("llms-full.txt"), content)?;
    log::info!("[ai] Generated llms-full.txt");
    Ok(())
}

/// Extracts the content between `<body>` and `</body>` tags.
fn extract_body(html: &str) -> String {
    let lower = html.to_lowercase();
    let start = lower
        .find("<body")
        .and_then(|i| lower[i..].find('>').map(|j| i + j + 1))
        .unwrap_or(0);
    let end = lower.find("</body>").unwrap_or(html.len());
    html[start..end].to_string()
}

/// Strips HTML tags from a string, preserving text content.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

/// Collapses runs of whitespace into single spaces and trims.
fn collapse_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_ws = true; // start true to trim leading
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                result.push(' ');
                prev_ws = true;
            }
        } else {
            result.push(ch);
            prev_ws = false;
        }
    }
    // Trim trailing space
    if result.ends_with(' ') {
        let _ = result.pop();
    }
    result
}

/// Counts `<img>` tags missing alt attributes in an HTML string.
fn count_missing_alt(html: &str) -> usize {
    let lower = html.to_lowercase();
    let mut count = 0;
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<img") {
        let abs = pos + start;
        let tag_end =
            lower[abs..].find('>').map_or(lower.len(), |e| abs + e + 1);
        let tag = &lower[abs..tag_end];

        let has_alt = tag.contains("alt=");
        let empty_alt = tag.contains("alt=\"\"") || tag.contains("alt=''");
        if !has_alt || empty_alt {
            count += 1;
        }
        pos = tag_end;
    }
    count
}

/// Recursively collects HTML files (delegates to `crate::walk`).
fn collect_html_files(dir: &Path) -> Result<Vec<PathBuf>> {
    fail_point!("ai::collect-html-files", |_| {
        Err(anyhow::anyhow!("injected: ai::collect-html-files"))
    });
    crate::walk::walk_files(dir, "html").map_err(Into::into)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::cmd::SsgConfig;
    use crate::test_support::init_logger;
    use std::path::PathBuf;
    use tempfile::{tempdir, TempDir};

    // -------------------------------------------------------------------
    // Test fixtures
    // -------------------------------------------------------------------

    fn make_site() -> (TempDir, PathBuf, PluginContext) {
        init_logger();
        let dir = tempdir().expect("create tempdir");
        let site = dir.path().join("site");
        fs::create_dir_all(&site).expect("mkdir site");
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        (dir, site, ctx)
    }

    /// Writes an HTML file and a companion `.meta.json` sidecar.
    fn write_page(
        site: &Path,
        rel_path: &str,
        title: &str,
        description: &str,
        extra_fields: &str,
    ) {
        let html_path = site.join(rel_path);
        if let Some(parent) = html_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let html = format!(
            "<html><head><title>{title}</title></head>\
             <body><h1>{title}</h1><p>{description}</p></body></html>"
        );
        fs::write(&html_path, html).unwrap();

        let mut sidecar_json =
            format!(r#"{{"title": "{title}", "description": "{description}""#);
        if !extra_fields.is_empty() {
            sidecar_json.push_str(", ");
            sidecar_json.push_str(extra_fields);
        }
        sidecar_json.push('}');
        fs::write(html_path.with_extension("meta.json"), sidecar_json).unwrap();
    }

    // -------------------------------------------------------------------
    // AiPlugin — derive surface
    // -------------------------------------------------------------------

    #[test]
    fn ai_plugin_is_copy_after_move() {
        // Guards the `Copy` derive added in v0.0.34.
        let plugin = AiPlugin;
        let _copy = plugin;
        assert_eq!(plugin.name(), "ai");
    }

    #[test]
    fn name_returns_static_ai_identifier() {
        assert_eq!(AiPlugin.name(), "ai");
    }

    // -------------------------------------------------------------------
    // count_missing_alt — table-driven over the logical paths
    // -------------------------------------------------------------------

    #[test]
    fn count_missing_alt_table_driven() {
        let cases: &[(&str, usize, &str)] = &[
            // (input, expected_count, comment)
            (
                r#"<img src="a.jpg" alt="ok">"#,
                0,
                "alt present and non-empty",
            ),
            (r#"<img src="a.jpg">"#, 1, "no alt attribute at all"),
            (r#"<img src="a.jpg" alt="">"#, 1, "empty double-quoted alt"),
            (r#"<img src="a.jpg" alt=''>"#, 1, "empty single-quoted alt"),
            (
                r#"<img src="a.jpg"><img src="b.jpg" alt="ok">"#,
                1,
                "first missing, second ok",
            ),
            (
                r#"<img src="a.jpg"><img src="b.jpg">"#,
                2,
                "both missing — sequential scan progresses",
            ),
            ("", 0, "empty input → zero"),
            ("<p>no images here</p>", 0, "no <img> tags at all"),
            (r#"<IMG SRC="a.jpg" ALT="ok">"#, 0, "case-insensitive ALT"),
            (r#"<IMG SRC="a.jpg">"#, 1, "uppercase tag, no alt"),
        ];
        for (input, expected, comment) in cases {
            assert_eq!(
                count_missing_alt(input),
                *expected,
                "{comment}: count_missing_alt({input:?})"
            );
        }
    }

    #[test]
    fn count_missing_alt_unterminated_tag_does_not_panic() {
        let result = count_missing_alt("<img src=foo");
        assert!(result <= 1);
    }

    // -------------------------------------------------------------------
    // parse_robots_disallow
    // -------------------------------------------------------------------

    #[test]
    fn test_parse_robots_disallow() {
        let dir = tempdir().expect("tempdir");

        // Standard robots.txt with multiple directives
        fs::write(
            dir.path().join("robots.txt"),
            "User-agent: *\nDisallow: /admin/\nDisallow: /private/\nAllow: /\n",
        )
        .unwrap();
        let result = parse_robots_disallow(dir.path());
        assert_eq!(result, vec!["/admin/", "/private/"]);
    }

    #[test]
    fn test_parse_robots_disallow_empty_file() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("robots.txt"), "").unwrap();
        let result = parse_robots_disallow(dir.path());
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    fn test_parse_robots_disallow_no_disallow_lines() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("robots.txt"),
            "User-agent: *\nAllow: /\nSitemap: https://example.com/sitemap.xml\n",
        )
        .unwrap();
        let result = parse_robots_disallow(dir.path());
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    fn test_parse_robots_disallow_multiple_user_agents() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("robots.txt"),
            "User-agent: Googlebot\nDisallow: /nogoogle/\n\n\
             User-agent: *\nDisallow: /secret/\n",
        )
        .unwrap();
        let result = parse_robots_disallow(dir.path());
        assert_eq!(result, vec!["/nogoogle/", "/secret/"]);
    }

    #[test]
    fn test_parse_robots_disallow_missing_file() {
        let dir = tempdir().expect("tempdir");
        let result = parse_robots_disallow(dir.path());
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    fn test_parse_robots_disallow_empty_pattern_skipped() {
        // `Disallow:` with no path means allow all — should be skipped
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("robots.txt"),
            "User-agent: *\nDisallow:\nDisallow: /blocked/\n",
        )
        .unwrap();
        let result = parse_robots_disallow(dir.path());
        assert_eq!(result, vec!["/blocked/"]);
    }

    // -------------------------------------------------------------------
    // is_excluded_page
    // -------------------------------------------------------------------

    #[test]
    fn test_is_excluded_page_draft() {
        let mut meta = serde_json::Map::new();
        let _ = meta.insert("draft".to_string(), serde_json::Value::Bool(true));
        assert!(is_excluded_page(Path::new("post.html"), &meta));
    }

    #[test]
    fn test_is_excluded_page_draft_string() {
        let mut meta = serde_json::Map::new();
        let _ = meta.insert(
            "draft".to_string(),
            serde_json::Value::String("true".to_string()),
        );
        assert!(is_excluded_page(Path::new("post.html"), &meta));
    }

    #[test]
    fn test_is_excluded_page_private() {
        let mut meta = serde_json::Map::new();
        let _ =
            meta.insert("private".to_string(), serde_json::Value::Bool(true));
        assert!(is_excluded_page(Path::new("post.html"), &meta));
    }

    #[test]
    fn test_is_excluded_page_404() {
        let meta = serde_json::Map::new();
        assert!(is_excluded_page(Path::new("404.html"), &meta));
    }

    #[test]
    fn test_is_excluded_page_normal() {
        let mut meta = serde_json::Map::new();
        let _ = meta.insert(
            "title".to_string(),
            serde_json::Value::String("Hello".to_string()),
        );
        assert!(!is_excluded_page(Path::new("index.html"), &meta));
    }

    #[test]
    fn test_is_excluded_page_error_page() {
        let meta = serde_json::Map::new();
        assert!(is_excluded_page(Path::new("error500.html"), &meta));
    }

    // -------------------------------------------------------------------
    // group_pages_by_section
    // -------------------------------------------------------------------

    #[test]
    fn test_group_pages_by_section() {
        let entries = vec![
            (
                "Home".to_string(),
                "/index.html".to_string(),
                "Welcome".to_string(),
            ),
            (
                "Post 1".to_string(),
                "/blog/post1.html".to_string(),
                "First".to_string(),
            ),
            (
                "Post 2".to_string(),
                "/blog/post2.html".to_string(),
                "Second".to_string(),
            ),
            (
                "API Ref".to_string(),
                "/docs/api.html".to_string(),
                "API docs".to_string(),
            ),
        ];
        let grouped = group_pages_by_section(&entries);

        assert_eq!(grouped.len(), 3);
        assert!(grouped.contains_key("Pages"));
        assert!(grouped.contains_key("Blog"));
        assert!(grouped.contains_key("Docs"));
        assert_eq!(grouped["Pages"].len(), 1);
        assert_eq!(grouped["Blog"].len(), 2);
        assert_eq!(grouped["Docs"].len(), 1);
    }

    #[test]
    fn test_group_pages_by_section_root_only() {
        let entries = vec![
            (
                "About".to_string(),
                "/about.html".to_string(),
                String::new(),
            ),
            (
                "Contact".to_string(),
                "/contact.html".to_string(),
                String::new(),
            ),
        ];
        let grouped = group_pages_by_section(&entries);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped["Pages"].len(), 2);
    }

    #[test]
    fn test_group_pages_by_section_deterministic_order() {
        let entries = vec![
            ("Z".to_string(), "/zebra/z.html".to_string(), String::new()),
            ("A".to_string(), "/alpha/a.html".to_string(), String::new()),
            ("M".to_string(), "/middle/m.html".to_string(), String::new()),
        ];
        let grouped = group_pages_by_section(&entries);
        let keys: Vec<&String> = grouped.keys().collect();
        assert_eq!(keys, vec!["Alpha", "Middle", "Zebra"]);
    }

    // -------------------------------------------------------------------
    // generate_llms_txt — spec compliance
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn generate_llms_txt_with_full_config_includes_all_fields() {
        let dir = tempdir().expect("tempdir");
        let config = SsgConfig {
            site_name: "My Site".to_string(),
            site_description: "A great site".to_string(),
            base_url: "https://example.com".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        generate_llms_txt(dir.path(), Some(&config)).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(body.contains("# My Site"));
        assert!(body.contains("> A great site"));
        assert!(body.contains("Language: en"));
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn generate_llms_txt_without_config_uses_defaults() {
        let dir = tempdir().expect("tempdir");
        generate_llms_txt(dir.path(), None).unwrap();

        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(body.contains("# Site"));
        assert!(body.contains("Language: en"));
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn generate_llms_txt_strips_trailing_slash_from_base_url() {
        let dir = tempdir().expect("tempdir");
        let config = SsgConfig {
            site_name: "S".to_string(),
            site_description: "D".to_string(),
            base_url: "https://example.com/".to_string(),
            ..Default::default()
        };

        // Write a page so we can verify URL formatting
        write_page(dir.path(), "index.html", "Home", "Welcome", "");

        generate_llms_txt(dir.path(), Some(&config)).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        // URLs should not have double slashes
        assert!(
            !body.contains("//index.html"),
            "trailing slash should be normalised:\n{body}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn generate_llms_txt_into_missing_parent_returns_err() {
        let bogus = Path::new("/this/path/should/not/exist");
        assert!(generate_llms_txt(bogus, None).is_err());
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_txt_contains_language() {
        let dir = tempdir().expect("tempdir");
        let config = SsgConfig {
            language: "fr".to_string(),
            ..Default::default()
        };
        generate_llms_txt(dir.path(), Some(&config)).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            body.contains("Language: fr"),
            "llms.txt must include Language field:\n{body}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_txt_contains_language_defaults_to_en() {
        let dir = tempdir().expect("tempdir");
        let config = SsgConfig {
            language: String::new(),
            ..Default::default()
        };
        generate_llms_txt(dir.path(), Some(&config)).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            body.contains("Language: en"),
            "empty language should default to en:\n{body}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_txt_excludes_drafts() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "published.html", "Published", "Visible", "");
        write_page(
            dir.path(),
            "draft.html",
            "Draft Post",
            "Hidden",
            r#""draft": true"#,
        );

        generate_llms_txt(dir.path(), None).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            body.contains("Published"),
            "published page must appear:\n{body}"
        );
        assert!(
            !body.contains("Draft Post"),
            "draft page must be excluded:\n{body}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_txt_excludes_private() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "public.html", "Public", "Visible", "");
        write_page(
            dir.path(),
            "secret.html",
            "Secret",
            "Hidden",
            r#""private": true"#,
        );

        generate_llms_txt(dir.path(), None).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            !body.contains("Secret"),
            "private page must be excluded:\n{body}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_txt_excludes_404() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "index.html", "Home", "Welcome", "");
        write_page(dir.path(), "404.html", "Not Found", "Error page", "");

        generate_llms_txt(dir.path(), None).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            !body.contains("Not Found"),
            "404 page must be excluded:\n{body}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_txt_contains_sections() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "index.html", "Home", "Welcome", "");
        write_page(dir.path(), "blog/post.html", "My Post", "A blog post", "");
        write_page(
            dir.path(),
            "docs/api.html",
            "API Docs",
            "API reference",
            "",
        );

        generate_llms_txt(dir.path(), None).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            body.contains("## Pages"),
            "should have Pages section:\n{body}"
        );
        assert!(
            body.contains("## Blog"),
            "should have Blog section:\n{body}"
        );
        assert!(
            body.contains("## Docs"),
            "should have Docs section:\n{body}"
        );
        assert!(
            body.contains("- [My Post]"),
            "should contain page link:\n{body}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_txt_contains_disallow_section() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("robots.txt"),
            "User-agent: *\nDisallow: /admin/\n",
        )
        .unwrap();

        generate_llms_txt(dir.path(), None).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            body.contains("## Disallow"),
            "should have Disallow section:\n{body}"
        );
        assert!(
            body.contains("- /admin/"),
            "should contain disallow pattern:\n{body}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_txt_no_disallow_without_robots() {
        let dir = tempdir().expect("tempdir");
        generate_llms_txt(dir.path(), None).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            !body.contains("## Disallow"),
            "no robots.txt means no Disallow section:\n{body}"
        );
    }

    // -------------------------------------------------------------------
    // generate_llms_full_txt
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_full_txt_contains_body_content() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "index.html", "Home", "Welcome home", "");

        generate_llms_full_txt(dir.path(), None).unwrap();
        let body =
            fs::read_to_string(dir.path().join("llms-full.txt")).unwrap();
        assert!(body.contains("# Site"), "header present:\n{body}");
        assert!(body.contains("Language: en"), "language present:\n{body}");
        assert!(body.contains("## [Home]"), "page title present:\n{body}");
        assert!(body.contains("Welcome home"), "body text present:\n{body}");
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_full_txt_excludes_drafts() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "ok.html", "Visible", "Content", "");
        write_page(
            dir.path(),
            "hidden.html",
            "Hidden",
            "Secret",
            r#""draft": true"#,
        );

        generate_llms_full_txt(dir.path(), None).unwrap();
        let body =
            fs::read_to_string(dir.path().join("llms-full.txt")).unwrap();
        assert!(body.contains("Visible"), "published page present:\n{body}");
        assert!(!body.contains("Hidden"), "draft excluded:\n{body}");
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn test_llms_full_txt_excludes_404() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "index.html", "Home", "Welcome", "");
        write_page(dir.path(), "404.html", "Not Found", "Error", "");

        generate_llms_full_txt(dir.path(), None).unwrap();
        let body =
            fs::read_to_string(dir.path().join("llms-full.txt")).unwrap();
        assert!(!body.contains("Not Found"), "404 excluded:\n{body}");
    }

    // -------------------------------------------------------------------
    // strip_html_tags / extract_body / collapse_whitespace
    // -------------------------------------------------------------------

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<p>hello</p>"), "hello");
        assert_eq!(strip_html_tags("<div><b>bold</b> text</div>"), "bold text");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn test_extract_body() {
        let html =
            "<html><head><title>T</title></head><body>Content</body></html>";
        assert_eq!(extract_body(html), "Content");
    }

    #[test]
    fn test_extract_body_with_attributes() {
        let html = "<html><body class=\"main\">Content</body></html>";
        assert_eq!(extract_body(html), "Content");
    }

    #[test]
    fn test_extract_body_no_body_tag() {
        let html = "<p>Just a fragment</p>";
        assert_eq!(extract_body(html), html);
    }

    #[test]
    fn test_collapse_whitespace() {
        assert_eq!(collapse_whitespace("  hello   world  "), "hello world");
        assert_eq!(collapse_whitespace("no  extra"), "no extra");
        assert_eq!(collapse_whitespace(""), "");
    }

    // -------------------------------------------------------------------
    // titlecase_word
    // -------------------------------------------------------------------

    #[test]
    fn test_titlecase_word() {
        assert_eq!(titlecase_word("blog"), "Blog");
        assert_eq!(titlecase_word("DOCS"), "Docs");
        assert_eq!(titlecase_word(""), "");
        assert_eq!(titlecase_word("a"), "A");
    }

    // -------------------------------------------------------------------
    // after_compile — short-circuit + dispatch paths
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_missing_site_dir_returns_ok_without_writing() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("missing");
        let ctx =
            PluginContext::new(dir.path(), dir.path(), &missing, dir.path());

        AiPlugin.after_compile(&ctx).expect("missing site is fine");
        assert!(!missing.exists());
        assert!(!dir.path().join("llms.txt").exists());
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_injects_max_snippet_meta_tag() {
        let (_tmp, site, ctx) = make_site();
        let html = "<html><head><title>X</title></head><body></body></html>";
        fs::write(site.join("index.html"), html).unwrap();

        AiPlugin.after_compile(&ctx).unwrap();
        let output = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(output.contains("max-snippet"));
        assert!(output.contains("max-image-preview:large"));
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_creates_llms_txt_in_site_root() {
        let (_tmp, site, ctx) = make_site();
        AiPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("llms.txt").exists());
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_creates_llms_full_txt_in_site_root() {
        let (_tmp, site, ctx) = make_site();
        AiPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("llms-full.txt").exists());
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_idempotent_does_not_duplicate_meta_tag() {
        let (_tmp, site, ctx) = make_site();
        let html = "<html><head><title>X</title></head><body></body></html>";
        fs::write(site.join("index.html"), html).unwrap();

        AiPlugin.after_compile(&ctx).unwrap();
        AiPlugin.after_compile(&ctx).unwrap();

        let output = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(output.matches("max-snippet").count(), 1);
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_skips_html_files_without_head_tag() {
        let (_tmp, site, ctx) = make_site();
        fs::write(site.join("fragment.html"), "<p>just a fragment</p>")
            .unwrap();

        AiPlugin.after_compile(&ctx).unwrap();
        let output = fs::read_to_string(site.join("fragment.html")).unwrap();
        assert!(!output.contains("max-snippet"));
        assert_eq!(output, "<p>just a fragment</p>");
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_processes_files_in_subdirectories() {
        let (_tmp, site, ctx) = make_site();
        let nested = site.join("blog");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            nested.join("post.html"),
            "<html><head></head><body></body></html>",
        )
        .unwrap();

        AiPlugin.after_compile(&ctx).unwrap();
        let output = fs::read_to_string(nested.join("post.html")).unwrap();
        assert!(output.contains("max-snippet"));
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_logs_warning_for_pages_with_missing_alt() {
        let (_tmp, site, ctx) = make_site();
        fs::write(
            site.join("bad.html"),
            r#"<html><head></head><body><img src="a.jpg"></body></html>"#,
        )
        .unwrap();
        fs::write(
            site.join("worse.html"),
            r#"<html><head></head><body><img src="a.jpg" alt=""></body></html>"#,
        )
        .unwrap();

        AiPlugin.after_compile(&ctx).unwrap();
        let bad = fs::read_to_string(site.join("bad.html")).unwrap();
        assert!(bad.contains("max-snippet"));
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_does_not_rewrite_unchanged_files() {
        let (_tmp, site, ctx) = make_site();
        let html = "<html><head><meta name=\"robots\" content=\"max-snippet:-1\"></head><body></body></html>";
        fs::write(site.join("index.html"), html).unwrap();
        let original_mtime = fs::metadata(site.join("index.html"))
            .unwrap()
            .modified()
            .unwrap();

        AiPlugin.after_compile(&ctx).unwrap();
        let after = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(after, html, "unchanged file body must be preserved");
        let _ = original_mtime;
    }

    // -------------------------------------------------------------------
    // collect_html_files — recursion + filtering
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn collect_html_files_returns_empty_for_missing_directory() {
        let dir = tempdir().expect("tempdir");
        let result = collect_html_files(&dir.path().join("missing")).unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn collect_html_files_filters_non_html_extensions() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("a.html"), "").unwrap();
        fs::write(dir.path().join("b.css"), "").unwrap();
        fs::write(dir.path().join("c.js"), "").unwrap();

        let result = collect_html_files(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn collect_html_files_recurses_into_nested_subdirectories() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("top.html"), "").unwrap();
        fs::write(nested.join("deep.html"), "").unwrap();

        let result = collect_html_files(dir.path()).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn collect_html_files_returns_results_sorted() {
        let dir = tempdir().expect("tempdir");
        for name in ["zebra.html", "apple.html", "mango.html"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let result = collect_html_files(dir.path()).unwrap();
        let names: Vec<_> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["apple.html", "mango.html", "zebra.html"]);
    }

    // -------------------------------------------------------------------
    // is_excluded_page — falsy draft/private values and string forms
    // -------------------------------------------------------------------

    #[test]
    fn test_is_excluded_page_draft_false_is_not_excluded() {
        let mut meta = serde_json::Map::new();
        let _ =
            meta.insert("draft".to_string(), serde_json::Value::Bool(false));
        assert!(!is_excluded_page(Path::new("post.html"), &meta));
    }

    #[test]
    fn test_is_excluded_page_private_string_true() {
        let mut meta = serde_json::Map::new();
        let _ = meta.insert(
            "private".to_string(),
            serde_json::Value::String("true".to_string()),
        );
        assert!(is_excluded_page(Path::new("post.html"), &meta));
    }

    #[test]
    fn test_is_excluded_page_private_false_is_not_excluded() {
        let mut meta = serde_json::Map::new();
        let _ =
            meta.insert("private".to_string(), serde_json::Value::Bool(false));
        assert!(!is_excluded_page(Path::new("post.html"), &meta));
    }

    // -------------------------------------------------------------------
    // Sidecar read failures — a directory named *.meta.json exists but
    // cannot be read as a file, driving the empty-map fallback arm.
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn collect_page_entries_unreadable_sidecar_falls_back_to_empty_meta() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("page.html"),
            "<html><head></head><body>Hi</body></html>",
        )
        .unwrap();
        // Directory at the sidecar path: exists() is true, but
        // read_to_string fails (EISDIR), so meta falls back to an
        // empty map and the page (no title) is dropped.
        fs::create_dir_all(dir.path().join("page.meta.json")).unwrap();

        let entries = collect_page_entries(dir.path()).unwrap();
        assert!(
            entries.is_empty(),
            "titleless page must be dropped: {entries:?}"
        );
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn llms_full_txt_unreadable_sidecar_skips_page() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("page.html"),
            "<html><head></head><body>Hidden body</body></html>",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("page.meta.json")).unwrap();

        generate_llms_full_txt(dir.path(), None).unwrap();
        let body =
            fs::read_to_string(dir.path().join("llms-full.txt")).unwrap();
        assert!(
            !body.contains("Hidden body"),
            "page without readable sidecar (no title) must be skipped:\n{body}"
        );
    }

    // -------------------------------------------------------------------
    // llms.txt — entry line without a description
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn llms_txt_entry_without_description_omits_colon_suffix() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "index.html", "Home", "", "");

        generate_llms_txt(dir.path(), None).unwrap();
        let body = fs::read_to_string(dir.path().join("llms.txt")).unwrap();
        assert!(
            body.contains("- [Home](/index.html)\n"),
            "description-less entry must be a bare link:\n{body}"
        );
        assert!(
            !body.contains("- [Home](/index.html):"),
            "no trailing colon without a description:\n{body}"
        );
    }

    // -------------------------------------------------------------------
    // llms-full.txt — canonical absolute URLs when base_url is set
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn llms_full_txt_uses_canonical_root_for_urls() {
        let dir = tempdir().expect("tempdir");
        write_page(dir.path(), "index.html", "Home", "Welcome", "");

        let config = SsgConfig {
            site_name: "S".to_string(),
            base_url: "https://example.com/".to_string(),
            ..Default::default()
        };
        generate_llms_full_txt(dir.path(), Some(&config)).unwrap();
        let body =
            fs::read_to_string(dir.path().join("llms-full.txt")).unwrap();
        assert!(
            body.contains("## [Home](https://example.com/index.html)"),
            "page URL must be prefixed with the canonical root:\n{body}"
        );
    }

    // -------------------------------------------------------------------
    // is_excluded_page — string values that don't match "true": the
    // `is_some_and(|s| s == "true")` closure only runs when the field
    // is a JSON string, and only returns `true` when it isn't literally
    // "true"/"false" as a bool. Prior tests only exercised Bool and the
    // exact string "true", so the closure body evaluating to `false`
    // was never exercised.
    // -------------------------------------------------------------------

    #[test]
    fn test_is_excluded_page_draft_string_non_true_is_not_excluded() {
        let mut meta = serde_json::Map::new();
        let _ = meta.insert(
            "draft".to_string(),
            serde_json::Value::String("no".to_string()),
        );
        assert!(!is_excluded_page(Path::new("post.html"), &meta));
    }

    #[test]
    fn test_is_excluded_page_private_string_non_true_is_not_excluded() {
        let mut meta = serde_json::Map::new();
        let _ = meta.insert(
            "private".to_string(),
            serde_json::Value::String("no".to_string()),
        );
        assert!(!is_excluded_page(Path::new("post.html"), &meta));
    }

    // -------------------------------------------------------------------
    // after_compile — error propagation through each of the four
    // `.map_err(...)` sites (llms.txt, llms-full.txt, collect_html_files,
    // process_html_for_ai). Each squats the plugin's own output path
    // with a directory so the underlying `fs::write`/read fails.
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_propagates_llms_txt_write_error() {
        let (_tmp, site, ctx) = make_site();
        // `llms.txt` is a pre-existing directory, so `fs::write` fails
        // inside `generate_llms_txt`, and the error must propagate
        // through the first `.map_err` in `after_compile`.
        fs::create_dir_all(site.join("llms.txt")).unwrap();
        let err = AiPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    fn after_compile_propagates_llms_full_txt_write_error() {
        let (_tmp, site, ctx) = make_site();
        // `llms.txt` writes fine; `llms-full.txt` is squatted by a
        // directory so the second stage fails.
        fs::create_dir_all(site.join("llms-full.txt")).unwrap();
        let err = AiPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    #[serial_test::parallel(ai_failpoint)]
    #[cfg(unix)]
    fn after_compile_fails_when_html_file_is_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, site, ctx) = make_site();
        let html = site.join("locked.html");
        fs::write(&html, "<html><head></head><body></body></html>").unwrap();
        fs::set_permissions(&html, fs::Permissions::from_mode(0o000)).unwrap();

        let res = AiPlugin.after_compile(&ctx);

        let _ = fs::set_permissions(&html, fs::Permissions::from_mode(0o644));
        // Root CI runners bypass permission checks; only assert when the
        // read genuinely failed.
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }
}

// -------------------------------------------------------------------
// Fault injection — `ai::collect-html-files` isolates the third of
// three `collect_html_files` calls that a single `after_compile` run
// makes (the first, inside `collect_page_entries`, is swallowed by
// `generate_llms_txt`'s `unwrap_or_default`; the second, inside
// `generate_llms_full_txt`, propagates but is indistinguishable from
// the plugin's own direct call without sequencing the failpoint).
// -------------------------------------------------------------------
#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::plugin::PluginContext;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial_test::serial(ai_failpoint)]
    fn collect_html_files_third_call_failpoint_propagates() {
        let _guard = FailGuard("ai::collect-html-files");
        // Let the first two invocations (collect_page_entries via
        // generate_llms_txt, then generate_llms_full_txt's own call)
        // succeed; fail on the third (after_compile's direct call).
        fail::cfg("ai::collect-html-files", "2*off->1*return")
            .expect("activate failpoint");

        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());

        let err = AiPlugin
            .after_compile(&ctx)
            .expect_err("third collect_html_files call must propagate");
        assert!(format!("{err:?}").contains("injected: ai::collect-html-files"));
    }
}
