// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! News sitemap fix plugin.

use super::helpers::{read_meta_sidecars, xml_escape};
use crate::dates::parse_flexible_date;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use std::fs;

/// Repairs news-sitemap.xml by populating entries from front-matter
/// metadata instead of using placeholder values.
#[derive(Debug, Clone, Copy)]
pub struct NewsSitemapFixPlugin;

impl Plugin for NewsSitemapFixPlugin {
    fn name(&self) -> &'static str {
        "news-sitemap-fix"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        let path = ctx.site_dir.join("news-sitemap.xml");
        if !path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&path).with_path(&path)?;

        // If no placeholder issues, skip
        if !content.contains("Unnamed Publication")
            && !content.contains("Untitled Article")
            && !content.contains("<loc></loc>")
        {
            return Ok(());
        }

        let meta_entries =
            read_meta_sidecars(&ctx.site_dir).unwrap_or_default();

        // Get base_url from config
        let base_url = ctx
            .config
            .as_ref()
            .map(|c| c.base_url.trim_end_matches('/').to_string())
            .unwrap_or_default();

        // Build news entries from metadata
        let news_entries: Vec<String> = meta_entries
            .iter()
            .filter_map(|(rel_path, meta)| {
                build_news_entry(rel_path, meta, &base_url)
            })
            .collect();

        if news_entries.is_empty() {
            return Ok(());
        }

        // Rebuild the news sitemap
        let rebuilt = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:news="http://www.google.com/schemas/sitemap-news/0.9">
{}
</urlset>
"#,
            news_entries.join("\n")
        );

        fs::write(&path, rebuilt).with_path(&path)?;

        log::info!(
            "[news-sitemap-fix] Rebuilt news-sitemap.xml with {} entries",
            news_entries.len()
        );
        Ok(())
    }
}

/// Builds a single `<url>` entry for the news sitemap from metadata.
fn build_news_entry(
    rel_path: &str,
    meta: &std::collections::HashMap<String, String>,
    base_url: &str,
) -> Option<String> {
    let title = meta.get("title").cloned().unwrap_or_default();
    let name = meta
        .get("author")
        .or_else(|| meta.get("name"))
        .cloned()
        .unwrap_or_default();
    let language = meta
        .get("language")
        .cloned()
        .unwrap_or_else(|| "en".to_string());

    if title.is_empty() || rel_path.is_empty() {
        return None;
    }

    // Issue #586 / plan §2 item 1.4 (spec A4): shared flexible date
    // chain for <news:publication_date> — RFC 2822, long-form, and
    // ISO 8601 inputs all normalise to a W3C datetime now. This is
    // the ssg-side half of the chain that retires the upstream
    // "'day' component could not be parsed" warning spam.
    let pub_date = meta
        .get("item_pub_date")
        .map(|d| match parse_flexible_date(d) {
            Ok(dt) => dt.to_w3c_date(),
            Err(err) => {
                if !d.is_empty() {
                    log::warn!(
                        "[news-sitemap-fix] 'item_pub_date' for \
                         '{rel_path}': {err}"
                    );
                }
                d.clone()
            }
        })
        .unwrap_or_default();

    // Spec A2/B1 (plan §2 item 1.2): `<loc>` goes through the shared
    // page-URL derivation so news-sitemap, sitemap, canonical `<link>`
    // and feed `<link>` all publish the same directory-URL convention
    // (`{base}/{rel_path}/`, never `…/index.html`).
    let loc = crate::urls::derive_page_url(
        base_url,
        &format!("{rel_path}/index.html"),
    );

    let keywords = meta
        .get("keywords")
        .or_else(|| meta.get("tags"))
        .cloned()
        .unwrap_or_default();
    let extras = if keywords.is_empty() {
        String::new()
    } else {
        format!(
            "\n    <news:keywords>{}</news:keywords>",
            xml_escape(&keywords)
        )
    };

    Some(format!(
        r"<url>
  <loc>{loc}</loc>
  <news:news>
    <news:publication>
      <news:name>{name}</news:name>
      <news:language>{language}</news:language>
    </news:publication>
    <news:publication_date>{pub_date}</news:publication_date>
    <news:title>{title}</news:title>{extras}
  </news:news>
</url>"
    ))
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::plugin::PluginContext;
    use anyhow::Result;
    use std::collections::HashMap;
    use std::path::Path;
    use tempfile::tempdir;

    fn write_meta_sidecar(
        dir: &Path,
        slug: &str,
        meta: &HashMap<String, String>,
    ) {
        let page_dir = dir.join(slug);
        fs::create_dir_all(&page_dir).expect("create page dir");
        let meta_path = page_dir.join("index.meta.json");
        let json = serde_json::to_string(meta).expect("serialize meta");
        fs::write(&meta_path, json).expect("write meta");
    }

    fn make_atom_ctx(site_dir: &Path) -> PluginContext {
        crate::test_support::init_logger();
        let config = crate::cmd::SsgConfig {
            base_url: "https://example.com".to_string(),
            site_name: "Test Site".to_string(),
            site_title: "Test Site".to_string(),
            site_description: "A test site".to_string(),
            language: "en".to_string(),
            content_dir: std::path::PathBuf::from("content"),
            output_dir: std::path::PathBuf::from("build"),
            template_dir: std::path::PathBuf::from("templates"),
            theme: None,
            serve_dir: None,
            i18n: None,
            cdn_prefix: None,
            og_image: None,
            image: crate::cmd::ImageConfig::default(),
            edge_headers: crate::cmd::EdgeHeadersConfig::default(),
            agents: None,
            transitions: false,
            security: crate::cmd::SecurityConfig::default(),
            no_taxonomy_pages: false,
        };
        PluginContext::with_config(
            Path::new("content"),
            Path::new("build"),
            site_dir,
            Path::new("templates"),
            config,
        )
    }

    fn test_ctx(site_dir: &Path) -> PluginContext {
        crate::test_support::init_logger();
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            site_dir,
            Path::new("templates"),
        )
    }

    #[test]
    fn test_news_sitemap_with_keywords() -> Result<()> {
        let tmp = tempdir().unwrap();

        let news_path = tmp.path().join("news-sitemap.xml");
        fs::write(
            &news_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:news="http://www.google.com/schemas/sitemap-news/0.9">
<url>
  <loc></loc>
  <news:news>
    <news:publication>
      <news:name>Unnamed Publication</news:name>
      <news:language>en</news:language>
    </news:publication>
    <news:title>Untitled Article</news:title>
  </news:news>
</url>
</urlset>"#,
        )
        .unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Breaking News".to_string());
        let _ = meta.insert("author".to_string(), "Reporter".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert(
            "keywords".to_string(),
            "rust, programming, web".to_string(),
        );
        let _ = meta.insert("language".to_string(), "fr".to_string());
        write_meta_sidecar(tmp.path(), "breaking", &meta);

        let ctx = make_atom_ctx(tmp.path());
        NewsSitemapFixPlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&news_path).unwrap();
        assert!(
            result.contains(
                "<news:keywords>rust, programming, web</news:keywords>"
            ),
            "Should inject keywords: {result}"
        );
        assert!(
            result.contains("<news:name>Reporter</news:name>"),
            "Should use author name: {result}"
        );
        assert!(
            result.contains("<news:language>fr</news:language>"),
            "Should use custom language: {result}"
        );
        assert!(
            !result.contains("Unnamed Publication"),
            "Should not have placeholder: {result}"
        );
        assert!(
            !result.contains("Untitled Article"),
            "Should not have placeholder: {result}"
        );
        Ok(())
    }

    #[test]
    fn test_news_sitemap_with_tags_fallback() -> Result<()> {
        let tmp = tempdir().unwrap();

        let news_path = tmp.path().join("news-sitemap.xml");
        fs::write(
            &news_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:news="http://www.google.com/schemas/sitemap-news/0.9">
<url>
  <loc></loc>
  <news:news>
    <news:title>Untitled Article</news:title>
  </news:news>
</url>
</urlset>"#,
        )
        .unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Tagged Post".to_string());
        let _ = meta.insert("author".to_string(), "Writer".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Mon, 01 Sep 2025 12:00:00 +0000".to_string(),
        );
        let _ = meta.insert("tags".to_string(), "tech, science".to_string());
        write_meta_sidecar(tmp.path(), "tagged", &meta);

        let ctx = make_atom_ctx(tmp.path());
        NewsSitemapFixPlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&news_path).unwrap();
        assert!(
            result.contains("<news:keywords>tech, science</news:keywords>"),
            "Should fall back to tags for keywords: {result}"
        );
        Ok(())
    }

    #[test]
    fn test_news_sitemap_skips_when_no_placeholders() -> Result<()> {
        let tmp = tempdir().unwrap();

        let news_path = tmp.path().join("news-sitemap.xml");
        let original = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
<url>
  <loc>https://example.com/good</loc>
  <news:news>
    <news:title>Good Article</news:title>
  </news:news>
</url>
</urlset>"#;
        fs::write(&news_path, original).unwrap();

        let ctx = test_ctx(tmp.path());
        NewsSitemapFixPlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&news_path).unwrap();
        assert_eq!(
            result, original,
            "Should not modify well-formed news sitemap"
        );
        Ok(())
    }

    #[test]
    fn test_build_news_entry_returns_none_for_empty_title() {
        let meta = HashMap::new();
        assert!(
            build_news_entry("slug", &meta, "https://example.com").is_none(),
            "empty title should produce None"
        );
    }

    #[test]
    fn test_build_news_entry_returns_none_for_empty_path() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Hello".to_string());
        assert!(
            build_news_entry("", &meta, "https://example.com").is_none(),
            "empty rel_path should produce None"
        );
    }

    #[test]
    fn test_build_news_entry_valid() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "My Article".to_string());
        let _ = meta.insert("author".to_string(), "Author".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let entry =
            build_news_entry("my-article", &meta, "https://example.com")
                .expect("valid metadata should produce an entry");
        // Directory-URL convention shared with canonical/feed/sitemap
        // (plan §2 item 1.2, `urls::derive_page_url`).
        assert!(entry.contains("<loc>https://example.com/my-article/</loc>"));
        assert!(entry.contains("<news:name>Author</news:name>"));
        assert!(entry.contains("<news:title>My Article</news:title>"));
        assert!(entry.contains("<news:language>en</news:language>"));
    }

    // -----------------------------------------------------------------
    // Flexible date chain (issue #586 / plan §2 item 1.4, spec A4)
    // -----------------------------------------------------------------

    #[test]
    fn test_build_news_entry_rfc2822_date_is_w3c() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "RFC".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let entry = build_news_entry("rfc", &meta, "https://example.com")
            .expect("valid entry");
        assert!(
            entry.contains(
                "<news:publication_date>2026-04-11T06:06:06+00:00\
                 </news:publication_date>"
            ),
            "RFC 2822 input should produce a W3C datetime: {entry}"
        );
    }

    #[test]
    fn test_build_news_entry_iso_date_is_w3c() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "ISO".to_string());
        let _ =
            meta.insert("item_pub_date".to_string(), "2026-07-01".to_string());
        let entry = build_news_entry("iso", &meta, "https://example.com")
            .expect("valid entry");
        assert!(
            entry.contains(
                "<news:publication_date>2026-07-01T00:00:00+00:00\
                 </news:publication_date>"
            ),
            "ISO input should produce a W3C datetime: {entry}"
        );
    }

    #[test]
    fn test_build_news_entry_long_form_date_is_w3c() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Long".to_string());
        let _ = meta
            .insert("item_pub_date".to_string(), "July 1, 2026".to_string());
        let entry = build_news_entry("long", &meta, "https://example.com")
            .expect("valid entry");
        assert!(
            entry.contains(
                "<news:publication_date>2026-07-01T00:00:00+00:00\
                 </news:publication_date>"
            ),
            "long-form input should produce a W3C datetime: {entry}"
        );
    }

    #[test]
    fn test_build_news_entry_unparseable_date_passes_through() {
        crate::test_support::init_logger();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Bad".to_string());
        let _ =
            meta.insert("item_pub_date".to_string(), "not-a-date".to_string());
        let entry = build_news_entry("bad", &meta, "https://example.com")
            .expect("valid entry");
        assert!(
            entry.contains(
                "<news:publication_date>not-a-date</news:publication_date>"
            ),
            "unparseable input keeps previous passthrough behaviour: {entry}"
        );
    }

    #[test]
    fn test_build_news_entry_without_base_url() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Post".to_string());
        let _ = meta.insert("name".to_string(), "Writer".to_string());
        let entry = build_news_entry("post", &meta, "")
            .expect("should produce entry without base_url");
        assert!(
            entry.contains("<loc>/post/</loc>"),
            "loc should be a rooted directory URL when base_url is \
             empty: {entry}"
        );
        assert!(
            entry.contains("<news:name>Writer</news:name>"),
            "should fall back to 'name' field: {entry}"
        );
    }

    #[test]
    fn test_news_sitemap_no_file_is_noop() -> Result<()> {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        NewsSitemapFixPlugin.after_compile(&ctx).unwrap();
        assert!(!tmp.path().join("news-sitemap.xml").exists());
        Ok(())
    }

    #[test]
    fn test_news_sitemap_empty_entries_no_rebuild() -> Result<()> {
        let tmp = tempdir().unwrap();
        let news_path = tmp.path().join("news-sitemap.xml");
        // Has placeholder but no meta sidecars to rebuild from
        let original = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset><url><loc></loc><news:news><news:title>Untitled Article</news:title></news:news></url></urlset>"#;
        fs::write(&news_path, original).unwrap();

        let ctx = test_ctx(tmp.path());
        NewsSitemapFixPlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&news_path).unwrap();
        assert_eq!(
            result, original,
            "should not modify when no meta entries produce valid news entries"
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // build_news_entry: empty item_pub_date parses to nothing silently
    // -----------------------------------------------------------------

    #[test]
    fn test_build_news_entry_with_empty_pub_date() {
        crate::test_support::init_logger();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "T".to_string());
        let _ = meta.insert("item_pub_date".to_string(), String::new());
        let entry =
            build_news_entry("post", &meta, "https://example.com").unwrap();
        assert!(
            entry.contains("<news:publication_date></news:publication_date>"),
            "empty date passes through empty: {entry}"
        );
    }

    // -----------------------------------------------------------------
    // Error paths
    // -----------------------------------------------------------------

    #[test]
    fn test_after_compile_errors_on_invalid_utf8_news_sitemap() {
        let tmp = tempdir().unwrap();
        let news_path = tmp.path().join("news-sitemap.xml");
        fs::write(&news_path, [0xFF, 0xFE, 0xFD]).unwrap();
        let ctx = test_ctx(tmp.path());
        let err = NewsSitemapFixPlugin.after_compile(&ctx).unwrap_err();
        assert!(format!("{err}").contains("news-sitemap.xml"));
    }

    #[test]
    #[cfg(unix)]
    fn test_after_compile_write_failure_on_readonly_news_sitemap() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let news_path = tmp.path().join("news-sitemap.xml");
        fs::write(
            &news_path,
            "<urlset><url><news:title>Untitled Article</news:title></url></urlset>",
        )
        .unwrap();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Real Title".to_string());
        write_meta_sidecar(tmp.path(), "post", &meta);
        fs::set_permissions(&news_path, fs::Permissions::from_mode(0o444))
            .unwrap();

        let ctx = make_atom_ctx(tmp.path());
        let result = NewsSitemapFixPlugin.after_compile(&ctx);
        let _ =
            fs::set_permissions(&news_path, fs::Permissions::from_mode(0o644));
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("news-sitemap.xml"));
    }
}
