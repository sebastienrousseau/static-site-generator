// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Atom 1.0 feed plugin.

use super::helpers::{read_meta_sidecars, xml_escape};
use crate::dates::parse_flexible_date;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use std::fs;
use std::path::Path;

/// Generates an Atom 1.0 `atom.xml` feed from `.meta.json` sidecars.
///
/// Runs after `RssAggregatePlugin` in `after_compile`. Reads the same
/// sidecar files, sorts entries by date descending, and limits to 50.
#[derive(Debug, Clone, Copy)]
pub struct AtomFeedPlugin;

impl Plugin for AtomFeedPlugin {
    fn name(&self) -> &'static str {
        "atom-feed"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        let mut meta_entries =
            read_meta_sidecars(&ctx.site_dir).unwrap_or_default();

        // Fall back to build_dir/.meta for sidecars emitted by
        // TemplatePlugin::before_compile (not present in site_dir
        // when staticdatagen doesn't copy them).
        if meta_entries.is_empty() {
            let meta_dir = ctx.build_dir.join(".meta");
            if meta_dir.exists() {
                meta_entries =
                    read_meta_sidecars(&meta_dir).unwrap_or_default();
            }
        }

        // Last resort: extract entries from an existing rss.xml
        // (staticdatagen generates rss.xml natively even without sidecars).
        if meta_entries.is_empty() {
            meta_entries = extract_entries_from_rss(&ctx.site_dir);
        }

        let base_url = ctx
            .config
            .as_ref()
            .map(|c| c.base_url.trim_end_matches('/').to_string())
            .unwrap_or_default();

        let site_name = ctx
            .config
            .as_ref()
            .map(|c| c.site_name.clone())
            .unwrap_or_default();

        let feed_title = if site_name.is_empty() {
            "Untitled".to_string()
        } else {
            site_name
        };

        let mut articles = collect_atom_entries(&meta_entries, &base_url);
        // Sort by date descending, then by `id` ascending as a
        // deterministic tiebreaker. `read_meta_sidecars` walks the
        // filesystem tree, whose entry order is OS-dependent (ext4 vs
        // APFS) — without a tiebreaker, pages sharing a date (common
        // in synthetic fixtures) retain that non-deterministic order
        // through the stable sort, failing the cross-OS determinism
        // gate.
        articles
            .sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        articles.truncate(50);

        if articles.is_empty() {
            return Ok(());
        }

        let feed_xml = build_atom_feed(&feed_title, &base_url, &articles);

        let atom_path = ctx.site_dir.join("atom.xml");
        fs::write(&atom_path, &feed_xml).with_path(&atom_path)?;

        let atom_self_link = if base_url.is_empty() {
            "atom.xml".to_string()
        } else {
            format!("{base_url}/atom.xml")
        };
        inject_atom_link(&ctx.site_dir, &atom_self_link)?;

        log::info!(
            "[atom-feed] Generated atom.xml with {} entries",
            articles.len()
        );
        Ok(())
    }
}

/// Collects Atom entries from metadata sidecars.
fn collect_atom_entries(
    meta_entries: &[(String, std::collections::HashMap<String, String>)],
    base_url: &str,
) -> Vec<(String, AtomEntry)> {
    let mut articles = Vec::new();
    for (rel_path, meta) in meta_entries {
        if let Some(entry) = build_atom_entry(rel_path, meta, base_url) {
            articles.push(entry);
        }
    }
    articles
}

/// Builds a single Atom entry from metadata, or `None` if data is insufficient.
fn build_atom_entry(
    rel_path: &str,
    meta: &std::collections::HashMap<String, String>,
    base_url: &str,
) -> Option<(String, AtomEntry)> {
    if rel_path.is_empty() {
        return None;
    }

    let title = meta.get("title").cloned().unwrap_or_default();
    if title.is_empty() {
        return None;
    }

    let description = meta.get("description").cloned().unwrap_or_default();
    let pub_date = meta.get("item_pub_date").cloned().unwrap_or_default();
    let author = meta.get("author").cloned().unwrap_or_default();

    let link = if base_url.is_empty() {
        format!("{rel_path}/")
    } else {
        format!("{base_url}/{rel_path}/")
    };

    // Issue #586 / plan §2 item 1.4 (spec A4): shared flexible date
    // chain — RFC 2822, long-form, and ISO 8601 inputs all normalise
    // to the RFC 3339 shape Atom requires; unparseable values pass
    // through verbatim (previous behaviour) with a warning naming the
    // failing field.
    let rfc3339 = match parse_flexible_date(&pub_date) {
        Ok(dt) => dt.to_rfc3339(),
        Err(err) => {
            if !pub_date.is_empty() {
                log::warn!(
                    "[atom-feed] 'item_pub_date' for '{rel_path}': {err}"
                );
            }
            pub_date.clone()
        }
    };

    Some((
        rfc3339.clone(),
        AtomEntry {
            title,
            link: link.clone(),
            id: link,
            updated: rfc3339.clone(),
            published: rfc3339,
            summary: description,
            author,
        },
    ))
}

/// Builds the complete Atom XML feed from entries.
fn build_atom_feed(
    feed_title: &str,
    base_url: &str,
    articles: &[(String, AtomEntry)],
) -> String {
    let feed_updated = &articles[0].0;
    let entries_xml: String = articles
        .iter()
        .map(|(_, entry)| entry.to_xml())
        .collect::<Vec<_>>()
        .join("\n");

    let atom_self_link = if base_url.is_empty() {
        "atom.xml".to_string()
    } else {
        format!("{base_url}/atom.xml")
    };

    let feed_id = if base_url.is_empty() {
        "/".to_string()
    } else {
        base_url.to_string()
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>{feed_title}</title>
  <link href="{atom_self_link}" rel="self" type="application/atom+xml"/>
  <link href="{base_url}"/>
  <id>{feed_id}</id>
  <updated>{feed_updated}</updated>
{entries_xml}
</feed>
"#,
        feed_title = xml_escape(feed_title),
    )
}

/// A single Atom entry's data.
pub(super) struct AtomEntry {
    pub title: String,
    pub link: String,
    pub id: String,
    pub updated: String,
    pub published: String,
    pub summary: String,
    pub author: String,
}

impl AtomEntry {
    pub(super) fn to_xml(&self) -> String {
        let author_name = if self.author.is_empty() {
            "Unknown".to_string()
        } else {
            xml_escape(&self.author)
        };
        format!(
            r#"  <entry>
    <title>{title}</title>
    <link href="{link}"/>
    <id>{id}</id>
    <updated>{updated}</updated>
    <published>{published}</published>
    <summary>{summary}</summary>
    <author><name>{author}</name></author>
  </entry>"#,
            title = xml_escape(&self.title),
            link = xml_escape(&self.link),
            id = xml_escape(&self.id),
            updated = xml_escape(&self.updated),
            published = xml_escape(&self.published),
            summary = xml_escape(&self.summary),
            author = author_name,
        )
    }
}

/// Extracts entry metadata from an existing `rss.xml` when no sidecars
/// are available. Returns entries in the same format as `read_meta_sidecars`.
pub(super) fn extract_entries_from_rss(
    site_dir: &Path,
) -> Vec<(String, std::collections::HashMap<String, String>)> {
    let rss_path = site_dir.join("rss.xml");
    let Ok(rss_content) = fs::read_to_string(&rss_path) else {
        return Vec::new();
    };

    let mut entries = Vec::new();

    // Simple XML parsing: extract <item>…</item> blocks
    let mut search_from = 0;
    while let Some(item_start) = rss_content[search_from..].find("<item>") {
        let abs_start = search_from + item_start;
        let Some(item_end) = rss_content[abs_start..].find("</item>") else {
            break;
        };
        let item = &rss_content[abs_start..abs_start + item_end + 7];

        let mut meta = std::collections::HashMap::new();
        if let Some(title) = extract_xml_tag(item, "title") {
            let _ = meta.insert("title".to_string(), title);
        }
        if let Some(desc) = extract_xml_tag(item, "description") {
            let _ = meta.insert("description".to_string(), desc);
        }
        if let Some(date) = extract_xml_tag(item, "pubDate") {
            let _ = meta.insert("item_pub_date".to_string(), date);
        }
        if let Some(author) = extract_xml_tag(item, "author") {
            let _ = meta.insert("author".to_string(), author);
        }

        // Derive relative path from <link>
        let rel_path = extract_xml_tag(item, "link")
            .map(|link| {
                link.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default();

        if !rel_path.is_empty() && meta.contains_key("title") {
            entries.push((rel_path, meta));
        }

        search_from = abs_start + item_end + 7;
    }

    entries
}

/// Extracts text content from a simple XML tag.
///
/// Handles both `<tag>content</tag>` and `<tag attr="...">content</tag>`.
/// Strips CDATA wrappers and decodes common XML entities.
fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    // Match both <tag> and <tag attr="...">
    let open_plain = format!("<{tag}>");
    let open_attr = format!("<{tag} ");
    let close = format!("</{tag}>");

    let (start, content_start) = if let Some(pos) = xml.find(&open_plain) {
        (pos, pos + open_plain.len())
    } else {
        let pos = xml.find(&open_attr)?;
        let gt = xml[pos..].find('>')?;
        (pos, pos + gt + 1)
    };

    let _ = start; // used for finding the tag
    let end = xml[content_start..].find(&close)? + content_start;
    let content = xml[content_start..end].trim();

    // Strip CDATA wrapper
    let content = content
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(content);

    // Decode common XML entities
    let decoded = content
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");

    let decoded = decoded.trim();
    if decoded.is_empty() {
        None
    } else {
        Some(xml_escape(decoded))
    }
}

/// Inject `<link rel="alternate" type="application/atom+xml">` into
/// HTML files that don't already have one.
pub(super) fn inject_atom_link(
    site_dir: &Path,
    atom_url: &str,
) -> Result<(), SsgError> {
    let html_files = crate::walk::walk_files(site_dir, "html")
        .map_err(|e| SsgError::io(e, site_dir))?;
    for path in &html_files {
        let html = fs::read_to_string(path).with_path(path)?;

        if html.contains("application/atom+xml") {
            continue;
        }

        let link_tag = format!(
            "  <link rel=\"alternate\" type=\"application/atom+xml\" title=\"Atom Feed\" href=\"{atom_url}\"/>\n"
        );
        let modified = inject_before_head_close(&html, &link_tag);
        if modified != html {
            fs::write(path, &modified).with_path(path)?;
        }
    }
    Ok(())
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

    #[test]
    fn test_atom_feed_valid_namespace_and_elements() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Hello World".to_string());
        let _ =
            meta.insert("description".to_string(), "A test post".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Alice".to_string());
        write_meta_sidecar(tmp.path(), "hello", &meta);

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let atom_path = tmp.path().join("atom.xml");
        assert!(atom_path.exists(), "atom.xml should be created");

        let content = fs::read_to_string(&atom_path).unwrap();
        assert!(
            content.contains("xmlns=\"http://www.w3.org/2005/Atom\""),
            "Missing Atom namespace"
        );
        assert!(content.contains("<feed"), "Missing <feed> element");
        assert!(content.contains("<title>"), "Missing <title>");
        assert!(content.contains("rel=\"self\""), "Missing self link");
        assert!(content.contains("<id>"), "Missing <id>");
        assert!(content.contains("<updated>"), "Missing <updated>");
        assert!(content.contains("<entry>"), "Missing <entry>");
        assert!(content.contains("<author>"), "Missing <author>");
        assert!(
            content.contains("<name>Alice</name>"),
            "Missing author name"
        );
        assert!(content.contains("<summary>"), "Missing <summary>");
        assert!(content.contains("<published>"), "Missing <published>");
        Ok(())
    }

    #[test]
    fn test_atom_feed_entry_count_matches() -> Result<()> {
        let tmp = tempdir().unwrap();

        for i in 0..5 {
            let mut meta = HashMap::new();
            let _ = meta.insert("title".to_string(), format!("Post {i}"));
            let _ = meta.insert("description".to_string(), format!("Desc {i}"));
            let _ = meta.insert(
                "item_pub_date".to_string(),
                format!("Thu, {:02} Apr 2026 06:06:06 +0000", 10 + i),
            );
            let _ = meta.insert("author".to_string(), "Bob".to_string());
            write_meta_sidecar(tmp.path(), &format!("post-{i}"), &meta);
        }

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        let entry_count = content.matches("<entry>").count();
        assert_eq!(entry_count, 5, "Expected 5 entries, got {entry_count}");
        Ok(())
    }

    #[test]
    fn test_atom_feed_dates_are_rfc3339() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Date Test".to_string());
        let _ =
            meta.insert("description".to_string(), "Testing dates".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Charlie".to_string());
        write_meta_sidecar(tmp.path(), "datepost", &meta);

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        assert!(
            content.contains("2026-04-11T06:06:06+00:00"),
            "Expected RFC 3339 date in atom.xml, got:\n{content}"
        );
        Ok(())
    }

    #[test]
    fn test_atom_feed_idempotent() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Idempotent".to_string());
        let _ = meta.insert("description".to_string(), "Test".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Dave".to_string());
        write_meta_sidecar(tmp.path(), "idem", &meta);

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();
        let first = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();

        AtomFeedPlugin.after_compile(&ctx).unwrap();
        let second = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();

        assert_eq!(first, second, "Atom feed should be idempotent");
        Ok(())
    }

    #[test]
    fn test_atom_feed_injects_link_into_html() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Link Test".to_string());
        let _ = meta.insert("description".to_string(), "Test".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Eve".to_string());
        write_meta_sidecar(tmp.path(), "linktest", &meta);

        let html_path = tmp.path().join("index.html");
        fs::write(
            &html_path,
            "<html><head><title>Test</title></head><body></body></html>",
        )
        .unwrap();

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let html = fs::read_to_string(&html_path).unwrap();
        assert!(
            html.contains("application/atom+xml"),
            "HTML should have atom link tag"
        );
        Ok(())
    }

    #[test]
    fn test_atom_plugin_registers() {
        use crate::plugin::PluginManager;
        let mut pm = PluginManager::new();
        pm.register(AtomFeedPlugin);
        assert_eq!(pm.len(), 1);
        assert_eq!(pm.names(), vec!["atom-feed"]);
    }

    #[test]
    fn test_atom_feed_sorts_descending() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta_old = HashMap::new();
        let _ = meta_old.insert("title".to_string(), "Old Post".to_string());
        let _ = meta_old.insert("description".to_string(), "old".to_string());
        let _ = meta_old.insert(
            "item_pub_date".to_string(),
            "Mon, 01 Jan 2025 00:00:00 +0000".to_string(),
        );
        let _ = meta_old.insert("author".to_string(), "Alice".to_string());
        write_meta_sidecar(tmp.path(), "old-post", &meta_old);

        let mut meta_new = HashMap::new();
        let _ = meta_new.insert("title".to_string(), "New Post".to_string());
        let _ = meta_new.insert("description".to_string(), "new".to_string());
        let _ = meta_new.insert(
            "item_pub_date".to_string(),
            "Fri, 11 Apr 2026 12:00:00 +0000".to_string(),
        );
        let _ = meta_new.insert("author".to_string(), "Bob".to_string());
        write_meta_sidecar(tmp.path(), "new-post", &meta_new);

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        let first_entry_pos = content.find("<entry>").unwrap();
        let new_title_pos = content.find("New Post").unwrap();
        let old_title_pos = content.find("Old Post").unwrap();
        assert!(
            new_title_pos < old_title_pos,
            "Newer post should come first"
        );
        assert!(
            new_title_pos > first_entry_pos,
            "Title should be inside an entry"
        );
        Ok(())
    }

    #[test]
    fn test_atom_feed_empty_author_shows_unknown() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "No Author".to_string());
        let _ = meta.insert("description".to_string(), "test".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "no-author", &meta);

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        assert!(
            content.contains("<name>Unknown</name>"),
            "Empty author should show 'Unknown': {content}"
        );
        Ok(())
    }

    #[test]
    fn test_atom_feed_skips_empty_title() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), String::new());
        let _ = meta.insert("description".to_string(), "test".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "no-title", &meta);

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let atom_path = tmp.path().join("atom.xml");
        assert!(
            !atom_path.exists(),
            "Should not create atom.xml when all entries have empty titles"
        );
        Ok(())
    }

    #[test]
    fn test_atom_feed_xml_escapes_content() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta
            .insert("title".to_string(), "Tom & Jerry <friends>".to_string());
        let _ = meta
            .insert("description".to_string(), "A \"great\" show".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "O'Brien".to_string());
        write_meta_sidecar(tmp.path(), "escape-test", &meta);

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        assert!(content.contains("Tom &amp; Jerry"), "& should be escaped");
        assert!(
            content.contains("&lt;friends&gt;"),
            "< and > should be escaped"
        );
        assert!(
            content.contains("&quot;great&quot;"),
            "quotes should be escaped"
        );
        assert!(
            content.contains("O&apos;Brien"),
            "apostrophe should be escaped"
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // AtomEntry::to_xml direct test
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_entry_to_xml() {
        let entry = AtomEntry {
            title: "Test Post".to_string(),
            link: "https://example.com/test/".to_string(),
            id: "https://example.com/test/".to_string(),
            updated: "2026-04-11T06:06:06+00:00".to_string(),
            published: "2026-04-11T06:06:06+00:00".to_string(),
            summary: "A test summary".to_string(),
            author: "Alice".to_string(),
        };
        let xml = entry.to_xml();
        assert!(xml.contains("<entry>"));
        assert!(xml.contains("</entry>"));
        assert!(xml.contains("<title>Test Post</title>"));
        assert!(xml.contains("href=\"https://example.com/test/\""));
        assert!(xml.contains("<name>Alice</name>"));
        assert!(xml.contains("<summary>A test summary</summary>"));
    }

    #[test]
    fn test_atom_entry_empty_author() {
        let entry = AtomEntry {
            title: "No Author".to_string(),
            link: "https://example.com/".to_string(),
            id: "https://example.com/".to_string(),
            updated: "2026-01-01T00:00:00+00:00".to_string(),
            published: "2026-01-01T00:00:00+00:00".to_string(),
            summary: String::new(),
            author: String::new(),
        };
        let xml = entry.to_xml();
        assert!(
            xml.contains("<name>Unknown</name>"),
            "Empty author should show 'Unknown'"
        );
    }

    // -----------------------------------------------------------------
    // inject_atom_link
    // -----------------------------------------------------------------

    #[test]
    fn test_inject_atom_link_adds_tag() -> Result<()> {
        let tmp = tempdir().unwrap();
        let html_path = tmp.path().join("page.html");
        fs::write(
            &html_path,
            "<html><head><title>Test</title></head><body></body></html>",
        )
        .unwrap();

        inject_atom_link(tmp.path(), "https://example.com/atom.xml").unwrap();

        let result = fs::read_to_string(&html_path).unwrap();
        assert!(
            result.contains("application/atom+xml"),
            "Should inject atom link: {result}"
        );
        assert!(
            result.contains("href=\"https://example.com/atom.xml\""),
            "Should have correct href: {result}"
        );
        Ok(())
    }

    #[test]
    fn test_inject_atom_link_idempotent() -> Result<()> {
        let tmp = tempdir().unwrap();
        let html_path = tmp.path().join("page.html");
        fs::write(
            &html_path,
            "<html><head><title>Test</title></head><body></body></html>",
        )
        .unwrap();

        inject_atom_link(tmp.path(), "https://example.com/atom.xml").unwrap();
        let first = fs::read_to_string(&html_path).unwrap();

        inject_atom_link(tmp.path(), "https://example.com/atom.xml").unwrap();
        let second = fs::read_to_string(&html_path).unwrap();

        assert_eq!(first, second, "inject_atom_link should be idempotent");
        assert_eq!(
            second.matches("application/atom+xml").count(),
            1,
            "Should have exactly one atom link"
        );
        Ok(())
    }

    #[test]
    fn test_inject_atom_link_no_head() -> Result<()> {
        let tmp = tempdir().unwrap();
        let html_path = tmp.path().join("nohead.html");
        fs::write(&html_path, "<html><body>No head</body></html>").unwrap();

        inject_atom_link(tmp.path(), "https://example.com/atom.xml").unwrap();

        let result = fs::read_to_string(&html_path).unwrap();
        assert!(
            !result.contains("application/atom+xml"),
            "Should not inject when there is no </head>"
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // Plugin trait coverage
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_feed_plugin_name() {
        let plugin = AtomFeedPlugin;
        assert_eq!(plugin.name(), "atom-feed");
    }

    #[test]
    fn test_atom_feed_plugin_debug() {
        let plugin = AtomFeedPlugin;
        let debug = format!("{plugin:?}");
        assert!(debug.contains("AtomFeedPlugin"));
    }

    #[test]
    fn test_atom_feed_plugin_clone_copy() {
        let plugin = AtomFeedPlugin;
        let cloned = plugin;
        assert_eq!(cloned.name(), "atom-feed");
    }

    // -----------------------------------------------------------------
    // Empty site directory
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_feed_empty_site_dir() -> Result<()> {
        let tmp = tempdir().unwrap();
        // No sidecars, no rss.xml, nothing
        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let atom_path = tmp.path().join("atom.xml");
        assert!(
            !atom_path.exists(),
            "Should not create atom.xml with no entries"
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // Missing sidecar files / fallback paths
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_feed_falls_back_to_rss_xml() -> Result<()> {
        let tmp = tempdir().unwrap();
        // No sidecars, but an rss.xml exists
        let rss_content = r#"<?xml version="1.0"?>
<rss version="2.0">
<channel>
<title>Test</title>
<item>
<title>From RSS</title>
<description>Extracted from RSS</description>
<link>https://example.com/rss-post/</link>
<pubDate>Thu, 11 Apr 2026 06:06:06 +0000</pubDate>
<author>Alice</author>
</item>
</channel>
</rss>"#;
        fs::write(tmp.path().join("rss.xml"), rss_content).unwrap();

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let atom_path = tmp.path().join("atom.xml");
        assert!(atom_path.exists(), "Should create atom.xml from rss.xml");
        let content = fs::read_to_string(&atom_path).unwrap();
        assert!(
            content.contains("From RSS"),
            "Should contain entry from rss.xml"
        );
        Ok(())
    }

    #[test]
    fn test_atom_feed_rss_multiple_items() -> Result<()> {
        let tmp = tempdir().unwrap();
        let rss_content = r#"<?xml version="1.0"?>
<rss version="2.0">
<channel>
<title>Test</title>
<item>
<title>Post A</title>
<description>Desc A</description>
<link>https://example.com/post-a/</link>
<pubDate>Thu, 10 Apr 2026 00:00:00 +0000</pubDate>
</item>
<item>
<title>Post B</title>
<description>Desc B</description>
<link>https://example.com/post-b/</link>
<pubDate>Fri, 11 Apr 2026 00:00:00 +0000</pubDate>
</item>
</channel>
</rss>"#;
        fs::write(tmp.path().join("rss.xml"), rss_content).unwrap();

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        assert!(content.contains("Post A"));
        assert!(content.contains("Post B"));
        let entry_count = content.matches("<entry>").count();
        assert_eq!(entry_count, 2);
        Ok(())
    }

    #[test]
    fn test_atom_feed_rss_with_cdata() -> Result<()> {
        let tmp = tempdir().unwrap();
        let rss_content = r#"<?xml version="1.0"?>
<rss version="2.0">
<channel>
<title>Test</title>
<item>
<title><![CDATA[CDATA Title]]></title>
<description><![CDATA[CDATA Description]]></description>
<link>https://example.com/cdata-post/</link>
<pubDate>Thu, 11 Apr 2026 06:06:06 +0000</pubDate>
</item>
</channel>
</rss>"#;
        fs::write(tmp.path().join("rss.xml"), rss_content).unwrap();

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        assert!(content.contains("CDATA Title"), "Should unwrap CDATA");
        Ok(())
    }

    // -----------------------------------------------------------------
    // extract_xml_tag
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_xml_tag_simple() {
        let xml = "<item><title>Hello</title></item>";
        assert_eq!(extract_xml_tag(xml, "title"), Some("Hello".to_string()));
    }

    #[test]
    fn test_extract_xml_tag_with_attributes() {
        let xml = r#"<item><link href="http://example.com">text</link></item>"#;
        assert_eq!(extract_xml_tag(xml, "link"), Some("text".to_string()));
    }

    #[test]
    fn test_extract_xml_tag_missing() {
        let xml = "<item><title>Hello</title></item>";
        assert_eq!(extract_xml_tag(xml, "author"), None);
    }

    #[test]
    fn test_extract_xml_tag_empty_content() {
        let xml = "<item><title></title></item>";
        assert_eq!(extract_xml_tag(xml, "title"), None);
    }

    #[test]
    fn test_extract_xml_tag_cdata() {
        let xml = "<item><title><![CDATA[My Title]]></title></item>";
        assert_eq!(extract_xml_tag(xml, "title"), Some("My Title".to_string()));
    }

    #[test]
    fn test_extract_xml_tag_decodes_entities() {
        let xml = "<item><title>Tom &amp; Jerry</title></item>";
        // The function decodes entities then re-escapes via xml_escape
        let result = extract_xml_tag(xml, "title").unwrap();
        assert!(
            result.contains("Tom") && result.contains("Jerry"),
            "Should contain decoded text: {result}"
        );
    }

    #[test]
    fn test_extract_xml_tag_whitespace() {
        let xml = "<item><title>  Hello World  </title></item>";
        assert_eq!(
            extract_xml_tag(xml, "title"),
            Some("Hello World".to_string())
        );
    }

    // -----------------------------------------------------------------
    // extract_entries_from_rss
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_entries_from_rss_no_file() {
        let tmp = tempdir().unwrap();
        let entries = extract_entries_from_rss(tmp.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn test_extract_entries_from_rss_empty_rss() {
        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("rss.xml"),
            r#"<?xml version="1.0"?><rss><channel></channel></rss>"#,
        )
        .unwrap();
        let entries = extract_entries_from_rss(tmp.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn test_extract_entries_from_rss_item_without_title() {
        let tmp = tempdir().unwrap();
        let rss = r#"<?xml version="1.0"?>
<rss><channel>
<item>
<description>No title item</description>
<link>https://example.com/no-title/</link>
</item>
</channel></rss>"#;
        fs::write(tmp.path().join("rss.xml"), rss).unwrap();
        let entries = extract_entries_from_rss(tmp.path());
        // Has a link-derived rel_path and a description but no title,
        // so it should still be included (meta has no "title" key but
        // the filter checks contains_key("title") — so it's excluded)
        assert!(entries.is_empty());
    }

    #[test]
    fn test_extract_entries_from_rss_item_without_link() {
        let tmp = tempdir().unwrap();
        let rss = r#"<?xml version="1.0"?>
<rss><channel>
<item>
<title>No Link</title>
<description>No link item</description>
</item>
</channel></rss>"#;
        fs::write(tmp.path().join("rss.xml"), rss).unwrap();
        let entries = extract_entries_from_rss(tmp.path());
        // rel_path is empty without link => skipped
        assert!(entries.is_empty());
    }

    // -----------------------------------------------------------------
    // build_atom_entry
    // -----------------------------------------------------------------

    #[test]
    fn test_build_atom_entry_empty_rel_path() {
        let meta = HashMap::new();
        assert!(build_atom_entry("", &meta, "https://example.com").is_none());
    }

    #[test]
    fn test_build_atom_entry_empty_title() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), String::new());
        assert!(
            build_atom_entry("page", &meta, "https://example.com").is_none()
        );
    }

    #[test]
    fn test_build_atom_entry_minimal() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Test".to_string());
        let result = build_atom_entry("page", &meta, "https://example.com");
        assert!(result.is_some());
        let (date_key, entry) = result.unwrap();
        assert_eq!(entry.title, "Test");
        assert!(entry.link.contains("example.com/page/"));
        assert!(entry.author.is_empty());
        // No pub_date in meta => empty string date key
        assert!(date_key.is_empty());
    }

    #[test]
    fn test_build_atom_entry_empty_base_url() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Test".to_string());
        let result = build_atom_entry("page", &meta, "");
        assert!(result.is_some());
        let (_, entry) = result.unwrap();
        assert_eq!(entry.link, "page/");
    }

    #[test]
    fn test_build_atom_entry_with_all_fields() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Full Entry".to_string());
        let _ =
            meta.insert("description".to_string(), "A description".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Alice".to_string());

        let result = build_atom_entry("full", &meta, "https://example.com");
        let (date_key, entry) = result.unwrap();
        assert_eq!(entry.title, "Full Entry");
        assert_eq!(entry.summary, "A description");
        assert_eq!(entry.author, "Alice");
        assert!(date_key.contains("2026"));
    }

    #[test]
    fn test_build_atom_entry_unparseable_date() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Bad Date".to_string());
        let _ =
            meta.insert("item_pub_date".to_string(), "not-a-date".to_string());

        let result = build_atom_entry("baddate", &meta, "https://example.com");
        let (date_key, _) = result.unwrap();
        // Falls back to raw string
        assert_eq!(date_key, "not-a-date");
    }

    // -----------------------------------------------------------------
    // Flexible date chain (issue #586 / plan §2 item 1.4, spec A4)
    // -----------------------------------------------------------------

    #[test]
    fn test_build_atom_entry_iso_date() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "ISO Date".to_string());
        let _ =
            meta.insert("item_pub_date".to_string(), "2026-07-01".to_string());

        let result = build_atom_entry("isodate", &meta, "https://example.com");
        let (date_key, entry) = result.unwrap();
        assert_eq!(date_key, "2026-07-01T00:00:00+00:00");
        assert_eq!(entry.updated, "2026-07-01T00:00:00+00:00");
        assert_eq!(entry.published, "2026-07-01T00:00:00+00:00");
    }

    #[test]
    fn test_build_atom_entry_long_form_date() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Long Form".to_string());
        let _ = meta
            .insert("item_pub_date".to_string(), "July 1, 2026".to_string());

        let result = build_atom_entry("longform", &meta, "https://example.com");
        let (date_key, entry) = result.unwrap();
        assert_eq!(date_key, "2026-07-01T00:00:00+00:00");
        assert_eq!(entry.updated, "2026-07-01T00:00:00+00:00");
    }

    #[test]
    fn test_build_atom_entry_iso_datetime_normalised() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "ISO DT".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "2026-07-01T07:07:07Z".to_string(),
        );

        let result = build_atom_entry("isodt", &meta, "https://example.com");
        let (date_key, _) = result.unwrap();
        assert_eq!(date_key, "2026-07-01T07:07:07+00:00");
    }

    // -----------------------------------------------------------------
    // collect_atom_entries
    // -----------------------------------------------------------------

    #[test]
    fn test_collect_atom_entries_empty() {
        let entries: Vec<(String, HashMap<String, String>)> = vec![];
        let result = collect_atom_entries(&entries, "https://example.com");
        assert!(result.is_empty());
    }

    #[test]
    fn test_collect_atom_entries_filters_invalid() {
        let mut meta1 = HashMap::new();
        let _ = meta1.insert("title".to_string(), "Valid".to_string());
        let mut meta2 = HashMap::new();
        let _ = meta2.insert("title".to_string(), String::new()); // empty title

        let entries =
            vec![("valid".to_string(), meta1), ("invalid".to_string(), meta2)];
        let result = collect_atom_entries(&entries, "https://example.com");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1.title, "Valid");
    }

    // -----------------------------------------------------------------
    // build_atom_feed
    // -----------------------------------------------------------------

    #[test]
    fn test_build_atom_feed_structure() {
        let entry = AtomEntry {
            title: "Feed Test".to_string(),
            link: "https://example.com/test/".to_string(),
            id: "https://example.com/test/".to_string(),
            updated: "2026-04-11T00:00:00+00:00".to_string(),
            published: "2026-04-11T00:00:00+00:00".to_string(),
            summary: "Summary".to_string(),
            author: "Bob".to_string(),
        };
        let articles = vec![("2026-04-11T00:00:00+00:00".to_string(), entry)];
        let xml = build_atom_feed("My Feed", "https://example.com", &articles);
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("xmlns=\"http://www.w3.org/2005/Atom\""));
        assert!(xml.contains("<title>My Feed</title>"));
        assert!(xml.contains("rel=\"self\""));
        assert!(xml.contains("https://example.com/atom.xml"));
        assert!(xml.contains("<id>https://example.com</id>"));
        assert!(xml.contains("Feed Test"));
    }

    #[test]
    fn test_build_atom_feed_empty_base_url() {
        let entry = AtomEntry {
            title: "Test".to_string(),
            link: "test/".to_string(),
            id: "test/".to_string(),
            updated: "2026-01-01T00:00:00+00:00".to_string(),
            published: "2026-01-01T00:00:00+00:00".to_string(),
            summary: String::new(),
            author: String::new(),
        };
        let articles = vec![("2026-01-01T00:00:00+00:00".to_string(), entry)];
        let xml = build_atom_feed("Untitled", "", &articles);
        assert!(xml.contains("<id>/</id>"));
        assert!(xml.contains("href=\"atom.xml\""));
    }

    #[test]
    fn test_build_atom_feed_xml_escapes_title() {
        let entry = AtomEntry {
            title: "A".to_string(),
            link: "a/".to_string(),
            id: "a/".to_string(),
            updated: "2026-01-01T00:00:00+00:00".to_string(),
            published: "2026-01-01T00:00:00+00:00".to_string(),
            summary: String::new(),
            author: String::new(),
        };
        let articles = vec![("2026-01-01T00:00:00+00:00".to_string(), entry)];
        let xml = build_atom_feed(
            "Tom & Jerry's <Feed>",
            "https://example.com",
            &articles,
        );
        assert!(xml.contains("Tom &amp; Jerry"));
        assert!(xml.contains("&lt;Feed&gt;"));
    }

    // -----------------------------------------------------------------
    // AtomEntry::to_xml additional coverage
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_entry_to_xml_escapes_all_fields() {
        let entry = AtomEntry {
            title: "A & B".to_string(),
            link: "https://example.com/a&b/".to_string(),
            id: "https://example.com/a&b/".to_string(),
            updated: "2026-01-01".to_string(),
            published: "2026-01-01".to_string(),
            summary: "\"quoted\" <summary>".to_string(),
            author: "O'Brien".to_string(),
        };
        let xml = entry.to_xml();
        assert!(xml.contains("A &amp; B"), "Title not escaped");
        assert!(
            xml.contains("&quot;quoted&quot;"),
            "Summary quotes not escaped"
        );
        assert!(
            xml.contains("&lt;summary&gt;"),
            "Summary angles not escaped"
        );
        assert!(
            xml.contains("O&apos;Brien"),
            "Author apostrophe not escaped"
        );
    }

    #[test]
    fn test_atom_entry_to_xml_empty_summary() {
        let entry = AtomEntry {
            title: "No Summary".to_string(),
            link: "https://example.com/".to_string(),
            id: "https://example.com/".to_string(),
            updated: "2026-01-01".to_string(),
            published: "2026-01-01".to_string(),
            summary: String::new(),
            author: "Alice".to_string(),
        };
        let xml = entry.to_xml();
        assert!(xml.contains("<summary></summary>"));
    }

    // -----------------------------------------------------------------
    // Atom feed with config variations
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_feed_untitled_site() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Post".to_string());
        let _ = meta.insert("description".to_string(), "desc".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "post", &meta);

        // Use a config with empty site_name
        let config = crate::cmd::SsgConfig {
            base_url: "https://example.com".to_string(),
            site_name: String::new(),
            site_title: String::new(),
            site_description: String::new(),
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
        let ctx = PluginContext::with_config(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
            config,
        );

        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        assert!(
            content.contains("<title>Untitled</title>"),
            "Empty site_name should produce 'Untitled'"
        );
        Ok(())
    }

    #[test]
    fn test_atom_feed_no_config() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Post".to_string());
        let _ = meta.insert("description".to_string(), "desc".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "post", &meta);

        // PluginContext without config
        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );

        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let atom_path = tmp.path().join("atom.xml");
        assert!(atom_path.exists(), "atom.xml must be generated");
        let content = fs::read_to_string(&atom_path).unwrap();
        assert!(
            content.contains("<title>Untitled</title>"),
            "No config should produce 'Untitled'"
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // Date format parsing edge cases
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_entry_iso8601_date_passthrough() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "ISO Date".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "2026-04-11T12:00:00+00:00".to_string(),
        );
        let result = build_atom_entry("iso", &meta, "https://example.com");
        let (date_key, _) = result.unwrap();
        // ISO 8601 may not parse as RFC 2822, so raw string is used
        assert!(date_key.contains("2026"));
    }

    #[test]
    fn test_atom_entry_empty_date() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "No Date".to_string());
        let result = build_atom_entry("nodate", &meta, "https://example.com");
        let (date_key, entry) = result.unwrap();
        assert!(date_key.is_empty());
        assert!(entry.published.is_empty());
    }

    // -----------------------------------------------------------------
    // Truncation to 50 entries
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_feed_truncates_at_50() -> Result<()> {
        let tmp = tempdir().unwrap();

        for i in 0..60 {
            let mut meta = HashMap::new();
            let _ = meta.insert("title".to_string(), format!("Post {i}"));
            let _ = meta.insert("description".to_string(), format!("Desc {i}"));
            let _ = meta.insert(
                "item_pub_date".to_string(),
                format!(
                    "Thu, {:02} Apr 2026 {:02}:00:00 +0000",
                    (i % 28) + 1,
                    i % 24
                ),
            );
            let _ = meta.insert("author".to_string(), "Bot".to_string());
            write_meta_sidecar(tmp.path(), &format!("post-{i:03}"), &meta);
        }

        let ctx = make_atom_ctx(tmp.path());
        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(tmp.path().join("atom.xml")).unwrap();
        let entry_count = content.matches("<entry>").count();
        assert_eq!(
            entry_count, 50,
            "Should truncate to 50 entries, got {entry_count}"
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // inject_atom_link: multiple HTML files
    // -----------------------------------------------------------------

    #[test]
    fn test_inject_atom_link_multiple_files() -> Result<()> {
        let tmp = tempdir().unwrap();
        for name in ["index.html", "about.html", "contact.html"] {
            fs::write(
                tmp.path().join(name),
                "<html><head><title>T</title></head><body></body></html>",
            )
            .unwrap();
        }

        inject_atom_link(tmp.path(), "https://example.com/atom.xml").unwrap();

        for name in ["index.html", "about.html", "contact.html"] {
            let content = fs::read_to_string(tmp.path().join(name)).unwrap();
            assert!(
                content.contains("application/atom+xml"),
                "{name} should have atom link"
            );
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // RSS extraction edge cases
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_entries_from_rss_malformed_item() {
        let tmp = tempdir().unwrap();
        // Item that opens but never closes
        let rss = r#"<?xml version="1.0"?>
<rss><channel>
<item>
<title>Unclosed
</channel></rss>"#;
        fs::write(tmp.path().join("rss.xml"), rss).unwrap();
        let entries = extract_entries_from_rss(tmp.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn test_extract_entries_from_rss_link_trailing_slash() {
        let tmp = tempdir().unwrap();
        let rss = r#"<?xml version="1.0"?>
<rss><channel>
<item>
<title>Slash Test</title>
<link>https://example.com/my-post/</link>
</item>
</channel></rss>"#;
        fs::write(tmp.path().join("rss.xml"), rss).unwrap();
        let entries = extract_entries_from_rss(tmp.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "my-post");
    }

    // -----------------------------------------------------------------
    // Build dir .meta fallback
    // -----------------------------------------------------------------

    #[test]
    fn test_atom_feed_falls_back_to_build_meta_dir() -> Result<()> {
        let tmp = tempdir().unwrap();
        let site_dir = tmp.path().join("site");
        let build_dir = tmp.path().join("build");
        let meta_dir = build_dir.join(".meta");
        fs::create_dir_all(&site_dir).unwrap();
        fs::create_dir_all(&meta_dir).unwrap();

        // Put sidecar in build/.meta instead of site_dir
        let page_dir = meta_dir.join("fallback-post");
        fs::create_dir_all(&page_dir).unwrap();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Fallback".to_string());
        let _ = meta
            .insert("description".to_string(), "From build dir".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let json = serde_json::to_string(&meta).unwrap();
        fs::write(page_dir.join("index.meta.json"), json).unwrap();

        let config = crate::cmd::SsgConfig {
            base_url: "https://example.com".to_string(),
            site_name: "Test".to_string(),
            site_title: "Test".to_string(),
            site_description: "Test".to_string(),
            language: "en".to_string(),
            content_dir: std::path::PathBuf::from("content"),
            output_dir: build_dir.clone(),
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
        let ctx = PluginContext::with_config(
            Path::new("content"),
            &build_dir,
            &site_dir,
            Path::new("templates"),
            config,
        );

        AtomFeedPlugin.after_compile(&ctx).unwrap();

        let atom_path = site_dir.join("atom.xml");
        assert!(
            atom_path.exists(),
            "Should create atom.xml from build/.meta"
        );
        let content = fs::read_to_string(&atom_path).unwrap();
        assert!(content.contains("Fallback"));
        Ok(())
    }

    // -----------------------------------------------------------------
    // Error paths: write/read failures propagate as SsgError
    // -----------------------------------------------------------------

    fn sidecar_meta(title: &str) -> HashMap<String, String> {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), title.to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        meta
    }

    #[test]
    fn test_after_compile_errors_when_atom_xml_is_a_directory() {
        let tmp = tempdir().unwrap();
        write_meta_sidecar(tmp.path(), "post", &sidecar_meta("Post"));
        // A directory named atom.xml makes fs::write fail (EISDIR).
        fs::create_dir_all(tmp.path().join("atom.xml")).unwrap();

        let ctx = make_atom_ctx(tmp.path());
        let err = AtomFeedPlugin.after_compile(&ctx).unwrap_err();
        assert!(
            format!("{err}").contains("atom.xml"),
            "error names the path"
        );
    }

    #[test]
    fn test_after_compile_propagates_unreadable_html_error() {
        let tmp = tempdir().unwrap();
        write_meta_sidecar(tmp.path(), "post", &sidecar_meta("Post"));
        // Invalid UTF-8 makes read_to_string fail inside inject_atom_link.
        fs::write(tmp.path().join("bad.html"), [0xFF, 0xFE, 0xFD]).unwrap();

        let ctx = make_atom_ctx(tmp.path());
        let err = AtomFeedPlugin.after_compile(&ctx).unwrap_err();
        assert!(
            format!("{err}").contains("bad.html"),
            "error names the file"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_inject_atom_link_write_failure_on_readonly_html() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let html_path = tmp.path().join("index.html");
        fs::write(
            &html_path,
            "<html><head><title>T</title></head><body></body></html>",
        )
        .unwrap();
        fs::set_permissions(&html_path, fs::Permissions::from_mode(0o444))
            .unwrap();

        let result = inject_atom_link(tmp.path(), "https://x.example/atom.xml");
        let _ =
            fs::set_permissions(&html_path, fs::Permissions::from_mode(0o644));
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("index.html"));
    }

    #[test]
    #[cfg(unix)]
    fn test_inject_atom_link_walk_failure_on_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let result = inject_atom_link(tmp.path(), "https://x.example/atom.xml");
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        assert!(result.is_err(), "expected an error from the locked path");
    }

    // -----------------------------------------------------------------
    // extract_xml_tag: malformed attribute-form tags
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_xml_tag_attr_form_without_closing_bracket() {
        // `<title ` matches the attribute form, but the tag never
        // closes with `>`, so extraction bails out.
        assert_eq!(extract_xml_tag("<title attr=x no-gt", "title"), None);
    }

    #[test]
    fn test_extract_xml_tag_missing_end_tag() {
        assert_eq!(extract_xml_tag("<title>abc", "title"), None);
    }
}
