// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! RSS aggregate plugin.

use super::helpers::{extract_xml_value, read_meta_sidecars, xml_escape};
use crate::dates::{parse_flexible_date, DateFormat};
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use std::fs;

/// Aggregates per-page RSS items into the root `rss.xml` feed.
#[derive(Debug, Clone, Copy)]
pub struct RssAggregatePlugin;

/// Builds a list of `(sort_key, xml_item)` pairs from metadata entries.
fn collect_articles(
    meta_entries: &[(String, std::collections::HashMap<String, String>)],
    base_url: &str,
) -> Vec<(String, String)> {
    let mut articles: Vec<(String, String)> = Vec::new();
    for (rel_path, meta) in meta_entries {
        if rel_path.is_empty() {
            continue;
        }

        let title = meta.get("title").cloned().unwrap_or_default();
        let description = meta.get("description").cloned().unwrap_or_default();
        let pub_date = meta.get("item_pub_date").cloned().unwrap_or_default();
        let author = meta.get("author").cloned().unwrap_or_default();
        let banner = meta.get("banner").or_else(|| meta.get("image")).cloned();
        let category = meta.get("category").cloned();
        let tags = meta.get("tags").cloned();

        if title.is_empty() {
            continue;
        }

        let link = if base_url.is_empty() {
            format!("{rel_path}/")
        } else {
            format!("{base_url}/{rel_path}/")
        };

        // Issue #586 / plan §2 item 1.4 (spec A4): shared flexible
        // date chain (RFC 2822 → long form → ISO 8601). RFC 2822
        // inputs pass through verbatim so existing feed output stays
        // byte-identical; long-form/ISO inputs are normalised into a
        // valid RFC 2822 <pubDate> instead of leaking raw strings.
        let (sort_key, pub_date) = match parse_flexible_date(&pub_date) {
            Ok(dt) => {
                let rfc2822 = if dt.format == DateFormat::Rfc2822 {
                    pub_date.clone()
                } else {
                    dt.to_rfc2822()
                };
                (dt.to_rfc3339(), rfc2822)
            }
            Err(err) => {
                if !pub_date.is_empty() {
                    log::warn!(
                        "[rss-aggregate] 'item_pub_date' for '{rel_path}': {err}"
                    );
                }
                (pub_date.clone(), pub_date)
            }
        };

        let escaped_desc = xml_escape(&description);

        // Build optional elements
        let mut extras = String::new();

        // Enclosure for banner/image (P2 fix)
        if let Some(ref img) = banner {
            let img_url = if img.starts_with("http") {
                img.clone()
            } else if !base_url.is_empty() {
                format!("{base_url}/{}", img.trim_start_matches('/'))
            } else {
                img.clone()
            };
            let mime = if img_url.ends_with(".webp") {
                "image/webp"
            } else if img_url.ends_with(".png") {
                "image/png"
            } else {
                "image/jpeg"
            };
            extras.push_str(&format!(
                "\n      <enclosure url=\"{img_url}\" type=\"{mime}\" length=\"0\"/>"
            ));
        }

        // Category elements (P2 fix)
        if let Some(ref cat) = category {
            extras.push_str(&format!(
                "\n      <category>{}</category>",
                xml_escape(cat)
            ));
        }
        if let Some(ref t) = tags {
            for tag in t.split(',') {
                let tag = tag.trim();
                if !tag.is_empty() {
                    extras.push_str(&format!(
                        "\n      <category>{}</category>",
                        xml_escape(tag)
                    ));
                }
            }
        }

        let item = format!(
            r#"    <item>
      <title>{title}</title>
      <link>{link}</link>
      <description>{escaped_desc}</description>
      <guid isPermaLink="true">{link}</guid>
      <pubDate>{pub_date}</pubDate>
      <author>{author}</author>{extras}
    </item>"#
        );

        articles.push((sort_key, item));
    }
    articles
}

/// Formats the final RSS XML channel document.
fn build_rss_channel(
    channel_title: &str,
    channel_link: &str,
    channel_desc: &str,
    base_url: &str,
    language: &str,
    last_build_date: &str,
    copyright: &str,
    items_xml: &str,
) -> String {
    let mut channel_extras = String::new();
    if !language.is_empty() {
        channel_extras
            .push_str(&format!("\n    <language>{language}</language>"));
    }
    if !last_build_date.is_empty() {
        channel_extras.push_str(&format!(
            "\n    <lastBuildDate>{last_build_date}</lastBuildDate>"
        ));
    }
    if !copyright.is_empty() {
        channel_extras.push_str(&format!(
            "\n    <copyright>{}</copyright>",
            xml_escape(copyright)
        ));
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>{channel_title}</title>
    <link>{channel_link}</link>
    <description>{channel_desc}</description>
    <atom:link href="{base_url}/rss.xml" rel="self" type="application/rss+xml"/>{channel_extras}
{items_xml}
  </channel>
</rss>
"#
    )
}

impl Plugin for RssAggregatePlugin {
    fn name(&self) -> &'static str {
        "rss-aggregate"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        let rss_path = ctx.site_dir.join("rss.xml");
        if !rss_path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&rss_path).with_path(&rss_path)?;

        if content.matches("<item>").count() > 1 {
            return Ok(());
        }

        let meta_entries =
            read_meta_sidecars(&ctx.site_dir).unwrap_or_default();

        let base_url = ctx
            .config
            .as_ref()
            .map(|c| c.base_url.trim_end_matches('/').to_string())
            .unwrap_or_default();

        let language = extract_language(ctx);
        let copyright = extract_copyright(&meta_entries);

        let mut articles = collect_articles(&meta_entries, &base_url);
        // Sort by date descending, then by the rendered item itself
        // (unique per article — it embeds the item's own URL) as a
        // deterministic tiebreaker. `read_meta_sidecars` walks the
        // filesystem tree, whose entry order is OS-dependent (ext4 vs
        // APFS) — without a tiebreaker, articles sharing a date
        // (common in synthetic fixtures) retain that non-deterministic
        // order through the stable sort, failing the cross-OS
        // determinism gate.
        articles.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        articles.truncate(50);

        if articles.is_empty() {
            return Ok(());
        }

        let last_build_date = extract_last_build_date(&articles);

        let items_xml: String = articles
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let channel_title = extract_xml_value(&content, "title")
            .unwrap_or_else(|| "Untitled".to_string());
        let channel_link = extract_xml_value(&content, "link")
            .unwrap_or_else(|| base_url.clone());
        let channel_desc =
            extract_xml_value(&content, "description").unwrap_or_default();

        let rebuilt = build_rss_channel(
            &channel_title,
            &channel_link,
            &channel_desc,
            &base_url,
            &language,
            &last_build_date,
            &copyright,
            &items_xml,
        );

        fs::write(&rss_path, rebuilt).with_path(&rss_path)?;

        log::info!(
            "[rss-aggregate] Rebuilt rss.xml with {} article items",
            articles.len()
        );
        Ok(())
    }
}

/// Extracts the language setting from the plugin context.
fn extract_language(ctx: &PluginContext) -> String {
    ctx.config
        .as_ref()
        .and_then(|c| {
            if c.site_name.is_empty() {
                None
            } else {
                Some("en".to_string())
            }
        })
        .unwrap_or_else(|| "en".to_string())
}

/// Extracts the copyright string from meta entries.
fn extract_copyright(
    meta_entries: &[(String, std::collections::HashMap<String, String>)],
) -> String {
    meta_entries
        .iter()
        .find_map(|(_, m)| m.get("copyright").cloned())
        .unwrap_or_default()
}

/// Extracts the last build date from the most recent article.
fn extract_last_build_date(articles: &[(String, String)]) -> String {
    articles
        .first()
        .and_then(|(_, xml)| {
            xml.find("<pubDate>").and_then(|s| {
                let after = &xml[s + 9..];
                after.find("</pubDate>").map(|e| after[..e].to_string())
            })
        })
        .unwrap_or_default()
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
    fn test_rss_aggregate_single_item_trigger() -> Result<()> {
        let tmp = tempdir().unwrap();
        let rss_path = tmp.path().join("rss.xml");
        fs::write(
            &rss_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>My Site</title>
    <link>https://example.com</link>
    <description>A test site</description>
    <item>
      <title>Feed itself</title>
      <link>https://example.com/rss.xml</link>
    </item>
  </channel>
</rss>"#,
        )
        .unwrap();

        let ctx = test_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();
        Ok(())
    }

    #[test]
    fn test_rss_aggregate_with_full_metadata() -> Result<()> {
        let tmp = tempdir().unwrap();

        let rss_path = tmp.path().join("rss.xml");
        fs::write(
            &rss_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Blog</title>
    <link>https://example.com</link>
    <description>A test blog</description>
    <item>
      <title>Placeholder</title>
    </item>
  </channel>
</rss>"#,
        )
        .unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Article One".to_string());
        let _ = meta.insert(
            "description".to_string(),
            "First article desc".to_string(),
        );
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Alice".to_string());
        let _ = meta
            .insert("banner".to_string(), "/images/banner.webp".to_string());
        let _ = meta.insert("category".to_string(), "Technology".to_string());
        let _ = meta.insert("tags".to_string(), "rust, web".to_string());
        let _ = meta.insert(
            "copyright".to_string(),
            "Copyright 2026 Alice".to_string(),
        );
        write_meta_sidecar(tmp.path(), "article-one", &meta);

        let ctx = make_atom_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&rss_path).unwrap();

        assert!(
            result.contains(
                "<enclosure url=\"https://example.com/images/banner.webp\""
            ),
            "Should have enclosure with base_url prefix: {result}"
        );
        assert!(
            result.contains("type=\"image/webp\""),
            "Should detect webp MIME type: {result}"
        );
        assert!(
            result.contains("<category>Technology</category>"),
            "Should have category element: {result}"
        );
        assert!(
            result.contains("<category>rust</category>"),
            "Should have tag category 'rust': {result}"
        );
        assert!(
            result.contains("<category>web</category>"),
            "Should have tag category 'web': {result}"
        );
        assert!(
            result.contains("<language>en</language>"),
            "Should have language element: {result}"
        );
        assert!(
            result.contains("<lastBuildDate>"),
            "Should have lastBuildDate: {result}"
        );
        assert!(
            result.contains("<copyright>Copyright 2026 Alice</copyright>"),
            "Should have copyright: {result}"
        );

        Ok(())
    }

    #[test]
    fn test_rss_aggregate_banner_with_image_field() -> Result<()> {
        let tmp = tempdir().unwrap();

        let rss_path = tmp.path().join("rss.xml");
        fs::write(
            &rss_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>T</title><link>https://example.com</link><description>D</description><item><title>X</title></item></channel></rss>"#,
        ).unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Image Test".to_string());
        let _ =
            meta.insert("description".to_string(), "Testing image".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Mon, 01 Sep 2025 12:00:00 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Bob".to_string());
        let _ = meta.insert(
            "image".to_string(),
            "https://cdn.example.com/photo.png".to_string(),
        );
        write_meta_sidecar(tmp.path(), "img-test", &meta);

        let ctx = make_atom_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&rss_path).unwrap();
        assert!(
            result.contains("url=\"https://cdn.example.com/photo.png\""),
            "Should use absolute image URL as-is: {result}"
        );
        assert!(
            result.contains("type=\"image/png\""),
            "Should detect png MIME type: {result}"
        );
        Ok(())
    }

    #[test]
    fn test_rss_aggregate_jpeg_mime() -> Result<()> {
        let tmp = tempdir().unwrap();

        let rss_path = tmp.path().join("rss.xml");
        fs::write(
            &rss_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>T</title><link>https://example.com</link><description>D</description><item><title>X</title></item></channel></rss>"#,
        ).unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "JPEG Test".to_string());
        let _ = meta.insert("description".to_string(), "desc".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Mon, 01 Sep 2025 12:00:00 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Carol".to_string());
        let _ = meta.insert("banner".to_string(), "/img/photo.jpg".to_string());
        write_meta_sidecar(tmp.path(), "jpeg-test", &meta);

        let ctx = make_atom_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&rss_path).unwrap();
        assert!(
            result.contains("type=\"image/jpeg\""),
            "Should default to image/jpeg for .jpg: {result}"
        );
        Ok(())
    }

    #[test]
    fn test_rss_aggregate_skips_multi_item() -> Result<()> {
        let tmp = tempdir().unwrap();

        let rss_path = tmp.path().join("rss.xml");
        let original = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>T</title><link>x</link><description>D</description>
<item><title>A</title></item>
<item><title>B</title></item>
</channel></rss>"#;
        fs::write(&rss_path, original).unwrap();

        let ctx = test_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&rss_path).unwrap();
        assert_eq!(result, original, "Should not modify feed with >1 items");
        Ok(())
    }

    #[test]
    fn test_collect_articles_empty_entries() {
        let articles = collect_articles(&[], "https://example.com");
        assert!(
            articles.is_empty(),
            "no meta entries should produce no articles"
        );
    }

    #[test]
    fn test_collect_articles_skips_empty_title() {
        let mut meta = HashMap::new();
        let _ =
            meta.insert("description".to_string(), "no title here".to_string());
        let entries = vec![("page".to_string(), meta)];
        let articles = collect_articles(&entries, "https://example.com");
        assert!(
            articles.is_empty(),
            "entries without title should be skipped"
        );
    }

    #[test]
    fn test_collect_articles_skips_empty_path() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Has Title".to_string());
        let entries = vec![(String::new(), meta)];
        let articles = collect_articles(&entries, "https://example.com");
        assert!(
            articles.is_empty(),
            "entries with empty path should be skipped"
        );
    }

    #[test]
    fn test_collect_articles_multiple_entries_sorted() {
        let mut meta1 = HashMap::new();
        let _ = meta1.insert("title".to_string(), "Older".to_string());
        let _ = meta1.insert("description".to_string(), "old".to_string());
        let _ = meta1.insert(
            "item_pub_date".to_string(),
            "Mon, 01 Jan 2024 00:00:00 +0000".to_string(),
        );
        let _ = meta1.insert("author".to_string(), "A".to_string());

        let mut meta2 = HashMap::new();
        let _ = meta2.insert("title".to_string(), "Newer".to_string());
        let _ = meta2.insert("description".to_string(), "new".to_string());
        let _ = meta2.insert(
            "item_pub_date".to_string(),
            "Wed, 01 Jan 2025 00:00:00 +0000".to_string(),
        );
        let _ = meta2.insert("author".to_string(), "B".to_string());

        let entries = vec![
            ("old-post".to_string(), meta1),
            ("new-post".to_string(), meta2),
        ];
        let mut articles = collect_articles(&entries, "https://example.com");
        assert_eq!(articles.len(), 2);

        // Sort descending like the plugin does
        articles.sort_by(|a, b| b.0.cmp(&a.0));
        assert!(
            articles[0].1.contains("<title>Newer</title>"),
            "newest article should sort first"
        );
    }

    #[test]
    fn test_collect_articles_xml_escapes_description() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Escape Test".to_string());
        let _ = meta.insert(
            "description".to_string(),
            "Use <b>bold</b> & \"quotes\"".to_string(),
        );
        let _ = meta.insert("author".to_string(), "X".to_string());
        let entries = vec![("esc".to_string(), meta)];
        let articles = collect_articles(&entries, "");
        assert_eq!(articles.len(), 1);
        let xml = &articles[0].1;
        assert!(
            xml.contains("&lt;b&gt;bold&lt;/b&gt;"),
            "angle brackets should be escaped: {xml}"
        );
        assert!(xml.contains("&amp;"), "ampersands should be escaped: {xml}");
    }

    // -----------------------------------------------------------------
    // Flexible date chain (issue #586 / plan §2 item 1.4, spec A4)
    // -----------------------------------------------------------------

    #[test]
    fn test_collect_articles_rfc2822_date_passes_through_verbatim() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "RFC".to_string());
        // Deliberately wrong weekday (2026-04-11 is a Saturday):
        // verbatim passthrough keeps the feed byte-identical.
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let entries = vec![("rfc".to_string(), meta)];
        let articles = collect_articles(&entries, "https://example.com");
        let item = &articles[0].1;
        assert!(
            item.contains("<pubDate>Thu, 11 Apr 2026 06:06:06 +0000</pubDate>"),
            "RFC 2822 input must pass through unchanged: {item}"
        );
        assert_eq!(articles[0].0, "2026-04-11T06:06:06+00:00");
    }

    #[test]
    fn test_collect_articles_iso_date_becomes_rfc2822_pubdate() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "ISO".to_string());
        let _ =
            meta.insert("item_pub_date".to_string(), "2026-07-01".to_string());
        let entries = vec![("iso".to_string(), meta)];
        let articles = collect_articles(&entries, "https://example.com");
        let item = &articles[0].1;
        assert!(
            item.contains("<pubDate>Wed, 01 Jul 2026 00:00:00 +0000</pubDate>"),
            "ISO input should be normalised to RFC 2822: {item}"
        );
        assert_eq!(articles[0].0, "2026-07-01T00:00:00+00:00");
    }

    #[test]
    fn test_collect_articles_long_form_date_becomes_rfc2822_pubdate() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Long".to_string());
        let _ = meta
            .insert("item_pub_date".to_string(), "July 1, 2026".to_string());
        let entries = vec![("long".to_string(), meta)];
        let articles = collect_articles(&entries, "");
        let item = &articles[0].1;
        assert!(
            item.contains("<pubDate>Wed, 01 Jul 2026 00:00:00 +0000</pubDate>"),
            "long-form input should be normalised to RFC 2822: {item}"
        );
    }

    #[test]
    fn test_collect_articles_unparseable_date_passes_through() {
        crate::test_support::init_logger();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Bad".to_string());
        let _ =
            meta.insert("item_pub_date".to_string(), "not-a-date".to_string());
        let entries = vec![("bad".to_string(), meta)];
        let articles = collect_articles(&entries, "");
        let item = &articles[0].1;
        assert!(
            item.contains("<pubDate>not-a-date</pubDate>"),
            "unparseable input keeps previous passthrough behaviour: {item}"
        );
        assert_eq!(articles[0].0, "not-a-date");
    }

    #[test]
    fn test_build_rss_channel_minimal() {
        let result = build_rss_channel(
            "Title",
            "https://x.example",
            "Desc",
            "https://x.example",
            "",
            "",
            "",
            "",
        );
        assert!(result.contains("<title>Title</title>"));
        assert!(result.contains("<link>https://x.example</link>"));
        assert!(result.contains("<description>Desc</description>"));
        assert!(
            !result.contains("<language>"),
            "no language when empty string supplied"
        );
        assert!(
            !result.contains("<lastBuildDate>"),
            "no lastBuildDate when empty string supplied"
        );
    }

    #[test]
    fn test_build_rss_channel_with_all_extras() {
        let result = build_rss_channel(
            "T",
            "L",
            "D",
            "https://x.example",
            "en",
            "Mon, 01 Jan 2024 00:00:00 +0000",
            "Copyright 2024 X",
            "<item><title>A</title></item>",
        );
        assert!(result.contains("<language>en</language>"));
        assert!(result.contains(
            "<lastBuildDate>Mon, 01 Jan 2024 00:00:00 +0000</lastBuildDate>"
        ));
        assert!(result.contains("<copyright>Copyright 2024 X</copyright>"));
        assert!(result.contains("<item><title>A</title></item>"));
    }

    #[test]
    fn test_extract_last_build_date_from_articles() {
        let articles = vec![
            ("2025".to_string(), "<item><pubDate>Mon, 01 Sep 2025 12:00:00 +0000</pubDate></item>".to_string()),
            ("2024".to_string(), "<item><pubDate>Mon, 01 Jan 2024 00:00:00 +0000</pubDate></item>".to_string()),
        ];
        let date = extract_last_build_date(&articles);
        assert_eq!(date, "Mon, 01 Sep 2025 12:00:00 +0000");
    }

    #[test]
    fn test_extract_last_build_date_empty() {
        let articles: Vec<(String, String)> = vec![];
        let date = extract_last_build_date(&articles);
        assert!(date.is_empty());
    }

    #[test]
    fn test_rss_no_file_is_noop() -> Result<()> {
        let tmp = tempdir().unwrap();
        // No rss.xml exists
        let ctx = test_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();
        assert!(!tmp.path().join("rss.xml").exists());
        Ok(())
    }

    // -----------------------------------------------------------------
    // Regression: sidecars with non-string fields (numeric word_count)
    // must not be dropped from the aggregate feed
    // -----------------------------------------------------------------

    #[test]
    fn test_rss_aggregate_keeps_page_with_numeric_sidecar_field() {
        let tmp = tempdir().unwrap();
        let rss_path = tmp.path().join("rss.xml");
        fs::write(
            &rss_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>T</title><link>https://example.com</link><description>D</description><item><title>X</title></item></channel></rss>"#,
        )
        .unwrap();

        // Hand-written sidecar with a NUMBER-valued field — the
        // pipeline emits numeric word_count, which previously failed
        // HashMap<String, String> deserialisation and silently dropped
        // the page from the feed.
        let page_dir = tmp.path().join("counted");
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(
            page_dir.join("index.meta.json"),
            r#"{"title":"Counted Post","description":"Has word_count","item_pub_date":"Thu, 11 Apr 2026 06:06:06 +0000","word_count":342}"#,
        )
        .unwrap();

        let ctx = make_atom_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&rss_path).unwrap();
        assert!(
            result.contains("<title>Counted Post</title>"),
            "page with numeric sidecar field must appear in feed: {result}"
        );
    }

    // -----------------------------------------------------------------
    // collect_articles: relative banner with no base_url
    // -----------------------------------------------------------------

    #[test]
    fn test_collect_articles_relative_banner_without_base_url() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Img".to_string());
        let _ = meta.insert("banner".to_string(), "/img/pic.png".to_string());
        let entries = vec![("img".to_string(), meta)];
        let articles = collect_articles(&entries, "");
        let item = &articles[0].1;
        assert!(
            item.contains("url=\"/img/pic.png\""),
            "relative banner is kept verbatim when base_url empty: {item}"
        );
    }

    #[test]
    fn test_collect_articles_skips_blank_tag_segments() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Tags".to_string());
        let _ = meta.insert("tags".to_string(), "rust,, web".to_string());
        let entries = vec![("tags".to_string(), meta)];
        let articles = collect_articles(&entries, "");
        let item = &articles[0].1;
        assert!(item.contains("<category>rust</category>"));
        assert!(item.contains("<category>web</category>"));
        assert_eq!(
            item.matches("<category>").count(),
            2,
            "blank tag segment must not emit an empty category: {item}"
        );
    }

    // -----------------------------------------------------------------
    // after_compile: sort path with multiple sidecar articles
    // -----------------------------------------------------------------

    #[test]
    fn test_rss_aggregate_sorts_multiple_articles_newest_first() {
        let tmp = tempdir().unwrap();
        let rss_path = tmp.path().join("rss.xml");
        fs::write(
            &rss_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>T</title><link>https://example.com</link><description>D</description><item><title>X</title></item></channel></rss>"#,
        )
        .unwrap();

        let mut older = HashMap::new();
        let _ = older.insert("title".to_string(), "Older".to_string());
        let _ = older.insert(
            "item_pub_date".to_string(),
            "Mon, 01 Jan 2024 00:00:00 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "older", &older);

        let mut newer = HashMap::new();
        let _ = newer.insert("title".to_string(), "Newer".to_string());
        let _ = newer.insert(
            "item_pub_date".to_string(),
            "Wed, 01 Jan 2025 00:00:00 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "newer", &newer);

        let ctx = make_atom_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&rss_path).unwrap();
        let newer_pos = result.find("<title>Newer</title>").unwrap();
        let older_pos = result.find("<title>Older</title>").unwrap();
        assert!(newer_pos < older_pos, "newest article must sort first");
    }

    // -----------------------------------------------------------------
    // Channel title / link fallbacks
    // -----------------------------------------------------------------

    #[test]
    fn test_rss_aggregate_falls_back_to_untitled_channel() {
        let tmp = tempdir().unwrap();
        let rss_path = tmp.path().join("rss.xml");
        // Single item, no <title>/<link>/<description> anywhere.
        fs::write(
            &rss_path,
            "<rss version=\"2.0\"><channel><item><guid>g</guid></item></channel></rss>",
        )
        .unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Post".to_string());
        write_meta_sidecar(tmp.path(), "post", &meta);

        let ctx = make_atom_ctx(tmp.path());
        RssAggregatePlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&rss_path).unwrap();
        assert!(
            result.contains("<title>Untitled</title>"),
            "missing channel title falls back to Untitled: {result}"
        );
        assert!(
            result.contains("<link>https://example.com</link>"),
            "missing channel link falls back to base_url: {result}"
        );
    }

    // -----------------------------------------------------------------
    // Error paths
    // -----------------------------------------------------------------

    #[test]
    fn test_after_compile_errors_on_invalid_utf8_rss() {
        let tmp = tempdir().unwrap();
        let rss_path = tmp.path().join("rss.xml");
        fs::write(&rss_path, [0xFF, 0xFE, 0xFD]).unwrap();
        let ctx = test_ctx(tmp.path());
        let err = RssAggregatePlugin.after_compile(&ctx).unwrap_err();
        assert!(format!("{err}").contains("rss.xml"));
    }

    #[test]
    #[cfg(unix)]
    fn test_after_compile_write_failure_on_readonly_rss() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let rss_path = tmp.path().join("rss.xml");
        fs::write(
            &rss_path,
            r#"<rss version="2.0"><channel><title>T</title><link>x</link><description>D</description><item><title>X</title></item></channel></rss>"#,
        )
        .unwrap();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Post".to_string());
        write_meta_sidecar(tmp.path(), "post", &meta);
        fs::set_permissions(&rss_path, fs::Permissions::from_mode(0o444))
            .unwrap();

        let ctx = make_atom_ctx(tmp.path());
        let result = RssAggregatePlugin.after_compile(&ctx);
        let _ =
            fs::set_permissions(&rss_path, fs::Permissions::from_mode(0o644));
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("rss.xml"));
    }

    // -----------------------------------------------------------------
    // extract_language: config with empty site_name
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_language_with_empty_site_name() {
        crate::test_support::init_logger();
        let config = crate::cmd::SsgConfig {
            base_url: String::new(),
            site_name: String::new(),
            site_title: String::new(),
            site_description: String::new(),
            language: String::new(),
            content_dir: std::path::PathBuf::from("c"),
            output_dir: std::path::PathBuf::from("b"),
            template_dir: std::path::PathBuf::from("t"),
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
        let ctx = PluginContext::with_config(
            Path::new("c"),
            Path::new("b"),
            Path::new("s"),
            Path::new("t"),
            config,
        );
        assert_eq!(extract_language(&ctx), "en");
    }
}
