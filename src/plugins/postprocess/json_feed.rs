// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! JSON Feed 1.1 plugin.
//!
//! Emits a `feed.json` file at the site root conforming to the
//! [JSON Feed 1.1 spec](https://jsonfeed.org/version/1.1).
//!
//! Runs alongside `RssAggregatePlugin` and `AtomFeedPlugin` in
//! `after_compile`, reading the same `.meta.json` sidecars (with the
//! same `build_dir/.meta` and `rss.xml` fallbacks).

use super::helpers::read_meta_sidecars;
use crate::dates::parse_flexible_date;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// JSON Feed version URL — the required top-level `version` field.
const JSON_FEED_VERSION: &str = "https://jsonfeed.org/version/1.1";

/// Maximum number of items emitted per feed (matches RSS/Atom).
const MAX_ITEMS: usize = 50;

/// Generates a JSON Feed 1.1 `feed.json` from `.meta.json` sidecars.
///
/// Runs in `after_compile`, alongside `RssAggregatePlugin` and
/// `AtomFeedPlugin`. Mirrors the sidecar discovery logic of
/// `AtomFeedPlugin` (`site_dir` → `build_dir/.meta` → `rss.xml` fallback)
/// so the three feeds stay in sync.
#[derive(Debug, Clone, Copy)]
pub struct JsonFeedPlugin;

impl Plugin for JsonFeedPlugin {
    fn name(&self) -> &'static str {
        "json-feed"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        let mut meta_entries =
            read_meta_sidecars(&ctx.site_dir).unwrap_or_default();

        if meta_entries.is_empty() {
            let meta_dir = ctx.build_dir.join(".meta");
            if meta_dir.exists() {
                meta_entries =
                    read_meta_sidecars(&meta_dir).unwrap_or_default();
            }
        }

        if meta_entries.is_empty() {
            meta_entries = super::atom::extract_entries_from_rss(&ctx.site_dir);
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

        let default_locale = extract_default_locale(ctx);
        let known_locales = extract_known_locales(ctx);

        let mut items = collect_items(&meta_entries, &base_url, &known_locales);
        // Sort by date descending, then by `id` ascending as a
        // deterministic tiebreaker. `read_meta_sidecars` walks the
        // filesystem tree, whose entry order is OS-dependent (ext4 vs
        // APFS) — without a tiebreaker, items sharing a sort_key
        // (common in synthetic fixtures) retain that non-deterministic
        // order through the stable sort, failing the cross-OS
        // determinism gate.
        items.sort_by(|a, b| {
            b.sort_key.cmp(&a.sort_key).then_with(|| a.id.cmp(&b.id))
        });
        items.truncate(MAX_ITEMS);

        if items.is_empty() {
            return Ok(());
        }

        let feed_url = if base_url.is_empty() {
            "feed.json".to_string()
        } else {
            format!("{base_url}/feed.json")
        };
        let home_page_url = if base_url.is_empty() {
            "/".to_string()
        } else {
            format!("{base_url}/")
        };

        let feed_json = build_feed_json(
            &feed_title,
            &home_page_url,
            &feed_url,
            &default_locale,
            &items,
        );

        let feed_path = ctx.site_dir.join("feed.json");
        let serialized = serialize_feed(&feed_json)
            .unwrap_or_else(|_| feed_json.to_string());
        fs::write(&feed_path, serialized).with_path(&feed_path)?;

        inject_json_feed_link(&ctx.site_dir, &feed_url)?;

        log::info!(
            "[json-feed] Generated feed.json with {} items",
            items.len()
        );
        Ok(())
    }
}

/// Serialize the feed with a fault-injection hook so tests can drive
/// the compact-encoding fallback branch (pretty-printing a `Value`
/// built from owned strings cannot fail in practice).
fn serialize_feed(feed_json: &Value) -> serde_json::Result<String> {
    fail_point!("postprocess::json-feed-serialize", |_| Err(
        <serde_json::Error as serde::ser::Error>::custom(
            "injected: postprocess::json-feed-serialize"
        )
    ));
    serde_json::to_string_pretty(feed_json)
}

/// A single JSON Feed item ready for serialisation.
pub(super) struct JsonFeedItem {
    pub sort_key: String,
    pub id: String,
    pub url: String,
    pub title: String,
    pub content_html: String,
    pub date_published: String,
    pub date_modified: String,
    pub author: String,
    pub tags: Vec<String>,
    pub language: Option<String>,
}

impl JsonFeedItem {
    /// Convert to a `serde_json::Value` for the items array.
    pub(super) fn to_json(&self) -> Value {
        let mut obj = Map::new();
        let _ = obj.insert("id".into(), Value::String(self.id.clone()));
        let _ = obj.insert("url".into(), Value::String(self.url.clone()));
        let _ = obj.insert("title".into(), Value::String(self.title.clone()));
        let _ = obj.insert(
            "content_html".into(),
            Value::String(self.content_html.clone()),
        );
        let _ = obj.insert(
            "date_published".into(),
            Value::String(self.date_published.clone()),
        );
        let _ = obj.insert(
            "date_modified".into(),
            Value::String(self.date_modified.clone()),
        );

        // authors[] — JSON Feed 1.1 uses an array.
        let author_name = if self.author.is_empty() {
            "Unknown".to_string()
        } else {
            self.author.clone()
        };
        let _ = obj.insert(
            "authors".into(),
            Value::Array(vec![json!({ "name": author_name })]),
        );

        // tags[] — always present (may be empty).
        let _ = obj.insert(
            "tags".into(),
            Value::Array(
                self.tags.iter().map(|t| Value::String(t.clone())).collect(),
            ),
        );

        if let Some(ref lang) = self.language {
            let _ = obj.insert("language".into(), Value::String(lang.clone()));
        }

        Value::Object(obj)
    }
}

/// Builds the top-level feed `Value`.
pub(super) fn build_feed_json(
    title: &str,
    home_page_url: &str,
    feed_url: &str,
    language: &str,
    items: &[JsonFeedItem],
) -> Value {
    let items_json: Vec<Value> =
        items.iter().map(JsonFeedItem::to_json).collect();

    let mut feed = Map::new();
    let _ = feed.insert(
        "version".into(),
        Value::String(JSON_FEED_VERSION.to_string()),
    );
    let _ = feed.insert("title".into(), Value::String(title.to_string()));
    let _ = feed.insert(
        "home_page_url".into(),
        Value::String(home_page_url.to_string()),
    );
    let _ = feed.insert("feed_url".into(), Value::String(feed_url.to_string()));
    if !language.is_empty() {
        let _ =
            feed.insert("language".into(), Value::String(language.to_string()));
    }
    let _ = feed.insert("items".into(), Value::Array(items_json));
    Value::Object(feed)
}

/// Collects JSON Feed items from metadata sidecars.
pub(super) fn collect_items(
    meta_entries: &[(String, HashMap<String, String>)],
    base_url: &str,
    known_locales: &[String],
) -> Vec<JsonFeedItem> {
    meta_entries
        .iter()
        .filter_map(|(rel_path, meta)| {
            build_item(rel_path, meta, base_url, known_locales)
        })
        .collect()
}

/// Builds a single `JsonFeedItem` from metadata, or `None` if invalid.
pub(super) fn build_item(
    rel_path: &str,
    meta: &HashMap<String, String>,
    base_url: &str,
    known_locales: &[String],
) -> Option<JsonFeedItem> {
    if rel_path.is_empty() {
        return None;
    }
    let title = meta.get("title").cloned().unwrap_or_default();
    if title.is_empty() {
        return None;
    }

    let description = meta.get("description").cloned().unwrap_or_default();
    let pub_date = meta.get("item_pub_date").cloned().unwrap_or_default();
    let modified_date = meta
        .get("last_build_date")
        .or_else(|| meta.get("date_modified"))
        .cloned()
        .unwrap_or_else(|| pub_date.clone());
    let author = meta.get("author").cloned().unwrap_or_default();

    let url = if base_url.is_empty() {
        format!("{rel_path}/")
    } else {
        format!("{base_url}/{rel_path}/")
    };

    // Issue #586 / plan §2 item 1.4 (spec A4): shared flexible date
    // chain — RFC 2822, long-form, and ISO 8601 all normalise to the
    // RFC 3339 shape JSON Feed 1.1 requires; unparseable values pass
    // through verbatim (previous behaviour) with a warning naming the
    // failing field.
    let flex_rfc3339 = |field: &str, raw: &str| match parse_flexible_date(raw) {
        Ok(dt) => dt.to_rfc3339(),
        Err(err) => {
            if !raw.is_empty() {
                log::warn!("[json-feed] '{field}' for '{rel_path}': {err}");
            }
            raw.to_string()
        }
    };
    let date_published = flex_rfc3339("item_pub_date", &pub_date);
    let date_modified =
        flex_rfc3339("last_build_date/date_modified", &modified_date);

    // Tags: prefer "tags" (comma-separated), fall back to "category".
    let mut tags: Vec<String> = meta
        .get("tags")
        .map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if tags.is_empty() {
        if let Some(cat) = meta.get("category") {
            let trimmed = cat.trim();
            if !trimmed.is_empty() {
                tags.push(trimmed.to_string());
            }
        }
    }

    // Per-item language: explicit `language`/`locale` meta, else
    // derived from path prefix matching a known locale (e.g. "fr/...").
    let language = meta
        .get("language")
        .or_else(|| meta.get("locale"))
        .cloned()
        .or_else(|| detect_locale_from_path(rel_path, known_locales));

    Some(JsonFeedItem {
        sort_key: date_published.clone(),
        id: url.clone(),
        url,
        title,
        content_html: description,
        date_published,
        date_modified,
        author,
        tags,
        language,
    })
}

/// Resolve the top-level feed language: prefer `i18n.default_locale`,
/// then `config.language`, then `"en"`.
pub(super) fn extract_default_locale(ctx: &PluginContext) -> String {
    if let Some(cfg) = ctx.config.as_ref() {
        if let Some(ref i18n) = cfg.i18n {
            if !i18n.default_locale.is_empty() {
                return i18n.default_locale.clone();
            }
        }
        if !cfg.language.is_empty() {
            return cfg.language.clone();
        }
    }
    "en".to_string()
}

/// Get the list of configured locales (for per-item language detection).
pub(super) fn extract_known_locales(ctx: &PluginContext) -> Vec<String> {
    ctx.config
        .as_ref()
        .and_then(|c| c.i18n.as_ref())
        .map(|i| i.locales.clone())
        .unwrap_or_default()
}

/// If the first path segment matches a known locale code, return it.
fn detect_locale_from_path(
    rel_path: &str,
    known_locales: &[String],
) -> Option<String> {
    // `split` always yields at least one segment, so this cannot be
    // empty-handed; `unwrap_or_default` keeps the expression total
    // without an unreachable `None` branch.
    let first = rel_path.split('/').next().unwrap_or_default();
    if known_locales.iter().any(|l| l == first) {
        Some(first.to_string())
    } else {
        None
    }
}

/// Inject `<link rel="alternate" type="application/feed+json">` into
/// every HTML page under `site_dir` that doesn't already have one.
pub(super) fn inject_json_feed_link(
    site_dir: &Path,
    feed_url: &str,
) -> Result<(), SsgError> {
    let html_files = crate::walk::walk_files(site_dir, "html")
        .map_err(|e| SsgError::io(e, site_dir))?;
    for path in &html_files {
        let html = fs::read_to_string(path).with_path(path)?;

        if html.contains("application/feed+json") {
            continue;
        }
        let link_tag = format!(
            "  <link rel=\"alternate\" type=\"application/feed+json\" title=\"JSON Feed\" href=\"{feed_url}\"/>\n"
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

    fn make_ctx(site_dir: &Path) -> PluginContext {
        crate::test_support::init_logger();
        let config = crate::cmd::SsgConfig {
            listings: Vec::new(),
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
    #[serial_test::parallel]
    fn test_json_feed_top_level_fields() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Hello World".to_string());
        let _ = meta.insert(
            "description".to_string(),
            "<p>A test post</p>".to_string(),
        );
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Alice".to_string());
        let _ = meta.insert("tags".to_string(), "rust, web".to_string());
        write_meta_sidecar(tmp.path(), "hello", &meta);

        let ctx = make_ctx(tmp.path());
        JsonFeedPlugin.after_compile(&ctx).unwrap();

        let feed_path = tmp.path().join("feed.json");
        assert!(feed_path.exists(), "feed.json should be created");

        let raw = fs::read_to_string(&feed_path).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["version"], JSON_FEED_VERSION);
        assert_eq!(value["title"], "Test Site");
        assert_eq!(value["home_page_url"], "https://example.com/");
        assert_eq!(value["feed_url"], "https://example.com/feed.json");
        assert_eq!(value["language"], "en");
        assert!(value["items"].is_array());
        assert_eq!(value["items"].as_array().unwrap().len(), 1);
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn test_json_feed_item_required_fields() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Item Test".to_string());
        let _ =
            meta.insert("description".to_string(), "<p>body</p>".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        let _ = meta.insert("author".to_string(), "Bob".to_string());
        let _ = meta.insert("tags".to_string(), "alpha".to_string());
        write_meta_sidecar(tmp.path(), "item-test", &meta);

        let ctx = make_ctx(tmp.path());
        JsonFeedPlugin.after_compile(&ctx).unwrap();

        let value: Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("feed.json")).unwrap(),
        )
        .unwrap();
        let item = &value["items"][0];

        assert!(item["id"].is_string());
        assert!(item["url"].is_string());
        assert_eq!(item["title"], "Item Test");
        assert_eq!(item["content_html"], "<p>body</p>");
        assert_eq!(item["date_published"], "2026-04-11T06:06:06+00:00");
        assert_eq!(item["date_modified"], "2026-04-11T06:06:06+00:00");

        let authors = item["authors"].as_array().unwrap();
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0]["name"], "Bob");

        let tags = item["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0], "alpha");
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn test_json_feed_injects_link_into_html() -> Result<()> {
        let tmp = tempdir().unwrap();

        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Link Test".to_string());
        let _ = meta.insert("description".to_string(), "x".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "linktest", &meta);

        let html_path = tmp.path().join("index.html");
        fs::write(
            &html_path,
            "<html><head><title>T</title></head><body></body></html>",
        )
        .unwrap();

        let ctx = make_ctx(tmp.path());
        JsonFeedPlugin.after_compile(&ctx).unwrap();

        let html = fs::read_to_string(&html_path).unwrap();
        assert!(
            html.contains("application/feed+json"),
            "missing feed+json link tag in: {html}"
        );
        assert!(html.contains("href=\"https://example.com/feed.json\""));
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn test_json_feed_empty_site_dir() -> Result<()> {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        JsonFeedPlugin.after_compile(&ctx).unwrap();
        assert!(!tmp.path().join("feed.json").exists());
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn test_json_feed_sorts_descending_and_truncates() -> Result<()> {
        let tmp = tempdir().unwrap();
        for i in 0..60 {
            let mut meta = HashMap::new();
            let _ = meta.insert("title".to_string(), format!("P{i}"));
            let _ = meta.insert("description".to_string(), format!("body {i}"));
            let _ = meta.insert(
                "item_pub_date".to_string(),
                format!(
                    "Thu, {:02} Apr 2026 {:02}:00:00 +0000",
                    (i % 28) + 1,
                    i % 24
                ),
            );
            write_meta_sidecar(tmp.path(), &format!("post-{i:03}"), &meta);
        }

        let ctx = make_ctx(tmp.path());
        JsonFeedPlugin.after_compile(&ctx).unwrap();
        let value: Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("feed.json")).unwrap(),
        )
        .unwrap();
        let items = value["items"].as_array().unwrap();
        assert_eq!(items.len(), MAX_ITEMS);
        // sorted descending => first sort_key >= last
        let first = items[0]["date_published"].as_str().unwrap();
        let last = items[items.len() - 1]["date_published"].as_str().unwrap();
        assert!(first >= last);
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn test_json_feed_empty_author_shows_unknown() -> Result<()> {
        let tmp = tempdir().unwrap();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "T".to_string());
        let _ = meta.insert("description".to_string(), "b".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "noauth", &meta);

        let ctx = make_ctx(tmp.path());
        JsonFeedPlugin.after_compile(&ctx).unwrap();
        let value: Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("feed.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["items"][0]["authors"][0]["name"], "Unknown");
        Ok(())
    }

    #[test]
    fn test_json_feed_locale_detection_from_path() {
        let known = vec!["en".to_string(), "fr".to_string()];
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Bonjour".to_string());
        let _ = meta.insert("description".to_string(), "x".to_string());
        let item =
            build_item("fr/bonjour", &meta, "https://example.com", &known)
                .unwrap();
        assert_eq!(item.language, Some("fr".to_string()));
    }

    #[test]
    fn test_json_feed_explicit_locale_overrides_path() {
        let known = vec!["en".to_string(), "fr".to_string()];
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "T".to_string());
        let _ = meta.insert("description".to_string(), "x".to_string());
        let _ = meta.insert("language".to_string(), "de".to_string());
        let item = build_item("fr/post", &meta, "https://example.com", &known)
            .unwrap();
        assert_eq!(item.language, Some("de".to_string()));
    }

    #[test]
    fn test_json_feed_skips_empty_title() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), String::new());
        assert!(build_item("post", &meta, "https://example.com", &[]).is_none());
    }

    #[test]
    fn test_json_feed_skips_empty_path() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "T".to_string());
        assert!(build_item("", &meta, "https://example.com", &[]).is_none());
    }

    #[test]
    fn test_json_feed_id_matches_url() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "T".to_string());
        let _ = meta.insert("description".to_string(), "x".to_string());
        let item = build_item("p", &meta, "https://example.com", &[]).unwrap();
        assert_eq!(item.id, item.url);
        assert_eq!(item.url, "https://example.com/p/");
    }

    // -----------------------------------------------------------------
    // Flexible date chain (issue #586 / plan §2 item 1.4, spec A4)
    // -----------------------------------------------------------------

    #[test]
    fn test_build_item_iso_date_normalised_to_rfc3339() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "ISO".to_string());
        let _ =
            meta.insert("item_pub_date".to_string(), "2026-07-01".to_string());
        let item = build_item("iso", &meta, "https://example.com", &[])
            .expect("valid item");
        assert_eq!(item.date_published, "2026-07-01T00:00:00+00:00");
        assert_eq!(item.date_modified, "2026-07-01T00:00:00+00:00");
    }

    #[test]
    fn test_build_item_long_form_date_normalised_to_rfc3339() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Long".to_string());
        let _ = meta
            .insert("item_pub_date".to_string(), "July 1, 2026".to_string());
        let _ = meta.insert(
            "date_modified".to_string(),
            "2026-07-02T07:07:07Z".to_string(),
        );
        let item = build_item("long", &meta, "https://example.com", &[])
            .expect("valid item");
        assert_eq!(item.date_published, "2026-07-01T00:00:00+00:00");
        assert_eq!(item.date_modified, "2026-07-02T07:07:07+00:00");
    }

    #[test]
    fn test_build_item_unparseable_date_passes_through() {
        crate::test_support::init_logger();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Bad".to_string());
        let _ =
            meta.insert("item_pub_date".to_string(), "not-a-date".to_string());
        let item = build_item("bad", &meta, "https://example.com", &[])
            .expect("valid item");
        // Verbatim fallback preserves the plugin's previous output.
        assert_eq!(item.date_published, "not-a-date");
    }

    #[test]
    fn test_json_feed_plugin_name() {
        assert_eq!(JsonFeedPlugin.name(), "json-feed");
    }

    #[test]
    fn test_json_feed_plugin_registers() {
        use crate::plugin::PluginManager;
        let mut pm = PluginManager::new();
        pm.register(JsonFeedPlugin);
        assert!(pm.names().contains(&"json-feed"));
    }

    #[test]
    fn test_json_feed_idempotent_link_injection() -> Result<()> {
        let tmp = tempdir().unwrap();
        let html_path = tmp.path().join("page.html");
        fs::write(
            &html_path,
            "<html><head><title>T</title></head><body></body></html>",
        )
        .unwrap();
        inject_json_feed_link(tmp.path(), "https://example.com/feed.json")
            .unwrap();
        let first = fs::read_to_string(&html_path).unwrap();
        inject_json_feed_link(tmp.path(), "https://example.com/feed.json")
            .unwrap();
        let second = fs::read_to_string(&html_path).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            second.matches("application/feed+json").count(),
            1,
            "should inject exactly one link tag"
        );
        Ok(())
    }

    #[test]
    fn test_json_feed_link_skips_files_without_head() -> Result<()> {
        let tmp = tempdir().unwrap();
        let html_path = tmp.path().join("frag.html");
        fs::write(&html_path, "<div>no head</div>").unwrap();
        inject_json_feed_link(tmp.path(), "https://example.com/feed.json")
            .unwrap();
        let result = fs::read_to_string(&html_path).unwrap();
        assert!(!result.contains("application/feed+json"));
        Ok(())
    }

    #[test]
    fn test_extract_default_locale_prefers_i18n() {
        use crate::i18n::I18nConfig;
        let tmp = tempdir().unwrap();
        let config = crate::cmd::SsgConfig {
            listings: Vec::new(),
            base_url: "https://example.com".to_string(),
            site_name: "S".to_string(),
            site_title: "S".to_string(),
            site_description: String::new(),
            language: "en".to_string(),
            content_dir: std::path::PathBuf::from("c"),
            output_dir: std::path::PathBuf::from("b"),
            template_dir: std::path::PathBuf::from("t"),
            theme: None,
            serve_dir: None,
            i18n: Some(I18nConfig {
                default_locale: "fr".to_string(),
                locales: vec!["en".into(), "fr".into()],
                url_prefix: crate::i18n::UrlPrefixStrategy::SubPath,
            }),
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
            tmp.path(),
            Path::new("t"),
            config,
        );
        assert_eq!(extract_default_locale(&ctx), "fr");
        assert_eq!(extract_known_locales(&ctx), vec!["en", "fr"]);
    }

    #[test]
    fn test_extract_default_locale_falls_back_to_language() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        assert_eq!(extract_default_locale(&ctx), "en");
        assert!(extract_known_locales(&ctx).is_empty());
    }

    #[test]
    fn test_extract_default_locale_no_config_defaults_en() {
        let ctx = PluginContext::new(
            Path::new("c"),
            Path::new("b"),
            Path::new("s"),
            Path::new("t"),
        );
        assert_eq!(extract_default_locale(&ctx), "en");
    }

    #[test]
    fn test_build_feed_json_omits_empty_language() {
        let items: Vec<JsonFeedItem> = vec![JsonFeedItem {
            sort_key: "2026".into(),
            id: "https://x/".into(),
            url: "https://x/".into(),
            title: "T".into(),
            content_html: "h".into(),
            date_published: "2026".into(),
            date_modified: "2026".into(),
            author: "A".into(),
            tags: vec![],
            language: None,
        }];
        let v = build_feed_json(
            "Title",
            "https://x/",
            "https://x/feed.json",
            "",
            &items,
        );
        assert!(v.get("language").is_none());
        assert_eq!(v["version"], JSON_FEED_VERSION);
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_tags_fallback_to_category() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "T".to_string());
        let _ = meta.insert("category".to_string(), "Tech".to_string());
        let item = build_item("p", &meta, "https://example.com", &[]).unwrap();
        assert_eq!(item.tags, vec!["Tech"]);
    }

    #[test]
    #[serial_test::parallel]
    fn test_json_feed_no_base_url() -> Result<()> {
        let tmp = tempdir().unwrap();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "T".to_string());
        let _ = meta.insert("description".to_string(), "x".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "p", &meta);

        let config = crate::cmd::SsgConfig {
            listings: Vec::new(),
            base_url: String::new(),
            site_name: "S".to_string(),
            site_title: "S".to_string(),
            site_description: String::new(),
            language: "en".to_string(),
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
            tmp.path(),
            Path::new("t"),
            config,
        );
        JsonFeedPlugin.after_compile(&ctx).unwrap();

        let value: Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("feed.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["feed_url"], "feed.json");
        assert_eq!(value["home_page_url"], "/");
        assert_eq!(value["items"][0]["url"], "p/");
        Ok(())
    }

    // -----------------------------------------------------------------
    // build_dir/.meta fallback (site_dir has no sidecars)
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel]
    fn test_json_feed_falls_back_to_build_meta_dir() {
        let tmp = tempdir().unwrap();
        let build = tmp.path().join("build");
        let site = tmp.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let page_dir = build.join(".meta").join("post");
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(
            page_dir.join("index.meta.json"),
            r#"{"title":"From Build Meta","item_pub_date":"Thu, 11 Apr 2026 06:06:06 +0000"}"#,
        )
        .unwrap();

        crate::test_support::init_logger();
        let config = crate::cmd::SsgConfig {
            listings: Vec::new(),
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
        let ctx = PluginContext::with_config(
            Path::new("content"),
            &build,
            &site,
            Path::new("templates"),
            config,
        );
        JsonFeedPlugin.after_compile(&ctx).unwrap();

        let raw = fs::read_to_string(site.join("feed.json")).unwrap();
        assert!(raw.contains("From Build Meta"));
    }

    // -----------------------------------------------------------------
    // JsonFeedItem::to_json: per-item language
    // -----------------------------------------------------------------

    #[test]
    fn test_to_json_includes_language_when_present() {
        let item = JsonFeedItem {
            sort_key: "2026".to_string(),
            id: "id".to_string(),
            url: "u/".to_string(),
            title: "T".to_string(),
            content_html: "C".to_string(),
            date_published: "2026-01-01T00:00:00+00:00".to_string(),
            date_modified: "2026-01-01T00:00:00+00:00".to_string(),
            author: "A".to_string(),
            tags: Vec::new(),
            language: Some("fr".to_string()),
        };
        let v = item.to_json();
        assert_eq!(v["language"], "fr");
    }

    // -----------------------------------------------------------------
    // build_item: whitespace-only category yields no tags
    // -----------------------------------------------------------------

    #[test]
    fn test_build_item_ignores_whitespace_only_category() {
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "T".to_string());
        let _ = meta.insert("category".to_string(), "   ".to_string());
        let item = build_item("p", &meta, "", &[]).unwrap();
        assert!(item.tags.is_empty(), "blank category must not become a tag");
    }

    // -----------------------------------------------------------------
    // extract_default_locale fallbacks
    // -----------------------------------------------------------------

    fn ctx_with_locale(
        site_dir: &Path,
        default_locale: &str,
        language: &str,
    ) -> PluginContext {
        let config = crate::cmd::SsgConfig {
            listings: Vec::new(),
            base_url: String::new(),
            site_name: "S".to_string(),
            site_title: String::new(),
            site_description: String::new(),
            language: language.to_string(),
            content_dir: std::path::PathBuf::from("c"),
            output_dir: std::path::PathBuf::from("b"),
            template_dir: std::path::PathBuf::from("t"),
            theme: None,
            serve_dir: None,
            i18n: Some(crate::i18n::I18nConfig {
                default_locale: default_locale.to_string(),
                locales: vec!["en".to_string(), "fr".to_string()],
                url_prefix: Default::default(),
            }),
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
            Path::new("c"),
            Path::new("b"),
            site_dir,
            Path::new("t"),
            config,
        )
    }

    #[test]
    fn test_extract_default_locale_empty_i18n_uses_language() {
        let tmp = tempdir().unwrap();
        let ctx = ctx_with_locale(tmp.path(), "", "de");
        assert_eq!(extract_default_locale(&ctx), "de");
    }

    #[test]
    fn test_extract_default_locale_all_empty_falls_back_to_en() {
        let tmp = tempdir().unwrap();
        let ctx = ctx_with_locale(tmp.path(), "", "");
        assert_eq!(extract_default_locale(&ctx), "en");
    }

    // -----------------------------------------------------------------
    // Error paths
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel]
    fn test_after_compile_errors_when_feed_json_is_a_directory() {
        let tmp = tempdir().unwrap();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Post".to_string());
        write_meta_sidecar(tmp.path(), "post", &meta);
        fs::create_dir_all(tmp.path().join("feed.json")).unwrap();

        let ctx = make_ctx(tmp.path());
        let err = JsonFeedPlugin.after_compile(&ctx).unwrap_err();
        assert!(format!("{err}").contains("feed.json"));
    }

    #[test]
    #[serial_test::parallel]
    fn test_after_compile_propagates_unreadable_html_error() {
        let tmp = tempdir().unwrap();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Post".to_string());
        write_meta_sidecar(tmp.path(), "post", &meta);
        fs::write(tmp.path().join("bad.html"), [0xFF, 0xFE, 0xFD]).unwrap();

        let ctx = make_ctx(tmp.path());
        let err = JsonFeedPlugin.after_compile(&ctx).unwrap_err();
        assert!(format!("{err}").contains("bad.html"));
    }

    #[test]
    #[cfg(unix)]
    fn test_inject_json_feed_link_write_failure_on_readonly_html() {
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

        let result =
            inject_json_feed_link(tmp.path(), "https://x.example/feed.json");
        let _ =
            fs::set_permissions(&html_path, fs::Permissions::from_mode(0o644));
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("index.html"));
    }

    #[test]
    #[cfg(unix)]
    fn test_inject_json_feed_link_walk_failure_on_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let result =
            inject_json_feed_link(tmp.path(), "https://x.example/feed.json");
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        assert!(result.is_err(), "expected an error from the locked path");
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::plugin::PluginContext;
    use serial_test::serial;
    use std::path::Path;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

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

    /// When `serialize_feed` fails (fault-injected), `after_compile`
    /// falls back to `Value::to_string()` (compact encoding) rather
    /// than propagating an error — feed.json must still be produced
    /// and remain valid JSON, just without pretty-printing.
    #[test]
    #[serial]
    fn after_compile_falls_back_to_compact_encoding_on_serialize_failure() {
        let _guard = FailGuard("postprocess::json-feed-serialize");
        fail::cfg("postprocess::json-feed-serialize", "return")
            .expect("activate failpoint");

        let tmp = tempdir().unwrap();
        let mut meta = HashMap::new();
        let _ = meta.insert("title".to_string(), "Fallback".to_string());
        let _ = meta.insert("description".to_string(), "x".to_string());
        let _ = meta.insert(
            "item_pub_date".to_string(),
            "Thu, 11 Apr 2026 06:06:06 +0000".to_string(),
        );
        write_meta_sidecar(tmp.path(), "fallback-post", &meta);

        crate::test_support::init_logger();
        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );
        JsonFeedPlugin
            .after_compile(&ctx)
            .expect("fallback path must not surface an error");

        let feed_path = tmp.path().join("feed.json");
        let raw = fs::read_to_string(&feed_path).unwrap();
        // Compact `Value::to_string()` output has no newlines, unlike
        // `to_string_pretty`, proving the fallback branch ran.
        assert!(
            !raw.contains('\n'),
            "expected compact fallback encoding, got: {raw}"
        );
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["items"][0]["title"], "Fallback");
    }
}
