// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! oEmbed 1.0 emitter (issue #586, port 4 of 5).
//!
//! Port of the site's Python-pipeline `OembedPlugin`: for every
//! shareable (public, titled) page, emit an
//! [oEmbed 1.0](https://oembed.com/) `link`-type document so
//! consumers (chat unfurlers, editors, CMSs) can render rich previews
//! without scraping the page.
//!
//! ## Files emitted
//!
//! For a page at `<site>/blog/post.html` the plugin writes the
//! sibling document `<site>/blog/post.oembed.json`:
//!
//! ```json
//! {
//!   "version": "1.0",
//!   "type": "link",
//!   "title": "Post title",
//!   "provider_name": "<site_name>",
//!   "provider_url": "<base_url>",
//!   "author_name": "<frontmatter author, when present>"
//! }
//! ```
//!
//! During the fused transform pass each page whose oEmbed sibling
//! exists also gains the standard discovery link in `<head>`:
//!
//! ```html
//! <link rel="alternate" type="application/json+oembed"
//!       href="<base>/blog/post.oembed.json" title="Post title">
//! ```
//!
//! ## Opt-in
//!
//! **Off by default**, per the tracker. Following the same
//! registration-gated convention as
//! [`crate::search_index::VectorSearchPlugin`], the plugin is *not*
//! part of `register_default_plugins` — sites opt in by registering
//! it explicitly:
//!
//! ```
//! use ssg::oembed::OembedPlugin;
//! use ssg::plugin::PluginManager;
//! let mut pm = PluginManager::new();
//! pm.register(OembedPlugin::default());
//! ```
//!
//! ## Lifecycle & determinism
//!
//! JSON documents are written in `after_compile`; the discovery
//! `<link>` is injected via `transform_html` which the pipeline runs
//! *after* `after_compile` (see `run_fused_transforms`), so the
//! sibling-exists check is reliable. Output contains no timestamps
//! and `serde_json`'s `BTreeMap`-backed maps serialise keys sorted —
//! byte-identical across rebuilds.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Plugin that emits per-page oEmbed 1.0 documents plus the discovery
/// `<link>` tag.
///
/// # Examples
///
/// ```
/// use ssg::oembed::OembedPlugin;
/// use ssg::plugin::Plugin;
/// assert_eq!(OembedPlugin::default().name(), "oembed");
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct OembedPlugin;

impl Plugin for OembedPlugin {
    fn name(&self) -> &'static str {
        "oembed"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if ctx.dry_run || !ctx.site_dir.exists() {
            return Ok(());
        }

        let posts = crate::agent_api::collect_posts(ctx);
        let mut written = 0usize;

        for post in &posts {
            let Some(rel) = page_rel_path(&post.url) else {
                continue;
            };
            let html_path = ctx.site_dir.join(&rel);
            // Only emit for pages that actually exist on disk — a
            // sidecar without a rendered page is stale metadata.
            if !html_path.exists() {
                continue;
            }
            let doc = build_oembed(
                &post.title,
                post.author.as_deref(),
                ctx.config.as_ref().map(|c| c.site_name.as_str()),
                ctx.config.as_ref().map(|c| c.base_url.as_str()),
            );
            let out = html_path.with_extension("oembed.json");
            let mut body = serde_json::to_string_pretty(&doc).map_err(|e| {
                SsgError::Io {
                    path: out.clone(),
                    source: std::io::Error::other(e),
                }
            })?;
            body.push('\n');
            fs::write(&out, body).with_path(&out)?;
            written += 1;
        }

        if written > 0 {
            log::info!("[oembed] Wrote {written} oembed.json document(s)");
        }
        Ok(())
    }

    fn has_transform(&self) -> bool {
        true
    }

    /// Injects the oEmbed discovery `<link>` before `</head>` when the
    /// page's `*.oembed.json` sibling exists. Idempotent.
    fn transform_html(
        &self,
        html: &str,
        path: &Path,
        ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        if html.contains("application/json+oembed") {
            return Ok(html.to_string());
        }
        let sibling = path.with_extension("oembed.json");
        if !sibling.exists() {
            return Ok(html.to_string());
        }

        let rel = path
            .strip_prefix(&ctx.site_dir)
            .unwrap_or(path)
            .with_extension("oembed.json");
        let rel = rel.to_string_lossy().replace('\\', "/");
        let base = ctx
            .config
            .as_ref()
            .map(|c| c.base_url.trim_end_matches('/').to_string())
            .unwrap_or_default();
        let href = if base.is_empty() {
            format!("/{rel}")
        } else {
            format!("{base}/{rel}")
        };

        let title = read_title(&sibling).unwrap_or_default();
        let link = format!(
            "<link rel=\"alternate\" type=\"application/json+oembed\" \
             href=\"{}\" title=\"{}\">",
            attr_escape(&href),
            attr_escape(&title),
        );
        // Parser-backed injection, not a `find("</head>")` splice: the
        // first byte match may be inside a comment or a script body in the
        // head, and the payload would land there — inert, and silently so,
        // because the document still parses (ssg#540).
        Ok(crate::util::head_dom::inject_before_head_close(html, &link))
    }
}

/// Builds one oEmbed 1.0 `link`-type document.
///
/// # Examples
///
/// ```
/// use ssg::oembed::build_oembed;
/// let doc = build_oembed(
///     "Hello",
///     Some("a@b.c (Jane)"),
///     Some("Example"),
///     Some("https://example.com"),
/// );
/// assert_eq!(doc["version"], "1.0");
/// assert_eq!(doc["type"], "link");
/// assert_eq!(doc["title"], "Hello");
/// assert_eq!(doc["provider_name"], "Example");
/// assert_eq!(doc["author_name"], "Jane");
/// ```
#[must_use]
pub fn build_oembed(
    title: &str,
    author: Option<&str>,
    provider_name: Option<&str>,
    provider_url: Option<&str>,
) -> Value {
    let mut obj = serde_json::Map::new();
    let _ = obj.insert("version".to_string(), Value::String("1.0".to_string()));
    let _ = obj.insert("type".to_string(), Value::String("link".to_string()));
    let _ = obj.insert("title".to_string(), Value::String(title.to_string()));
    if let Some(name) = provider_name.filter(|n| !n.is_empty()) {
        let _ = obj.insert(
            "provider_name".to_string(),
            Value::String(name.to_string()),
        );
    }
    if let Some(url) = provider_url.filter(|u| !u.is_empty()) {
        let _ = obj.insert(
            "provider_url".to_string(),
            Value::String(url.trim_end_matches('/').to_string()),
        );
    }
    if let Some(raw) = author {
        let (name, _) = crate::agent_api::parse_author(raw);
        if let Some(name) = name {
            let _ = obj.insert("author_name".to_string(), Value::String(name));
        }
    }
    Value::Object(obj)
}

/// Extracts the site-relative page path (`blog/post.html`) from an
/// absolute or root-relative post URL. Pretty (directory-shaped)
/// URLs resolve to their `index.html`.
fn page_rel_path(url: &str) -> Option<String> {
    let path = if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        let slash = rest.find('/')?;
        &rest[slash + 1..]
    } else {
        url.trim_start_matches('/')
    };
    if path.is_empty() {
        None
    } else if path.ends_with('/') {
        Some(format!("{path}index.html"))
    } else {
        Some(path.to_string())
    }
}

/// Reads the `title` field back out of an emitted oEmbed document.
fn read_title(oembed_path: &Path) -> Option<String> {
    let body = fs::read_to_string(oembed_path).ok()?;
    let doc: Value = serde_json::from_str(&body).ok()?;
    doc.get("title").and_then(Value::as_str).map(str::to_string)
}

/// Minimal HTML attribute escaping for injected markup.
fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::SsgConfig;
    use tempfile::{tempdir, TempDir};

    fn make_ctx() -> (TempDir, PluginContext) {
        let dir = tempdir().expect("tempdir");
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        fs::create_dir_all(build.join(".meta")).unwrap();
        fs::create_dir_all(&site).unwrap();
        let cfg = SsgConfig::builder()
            .site_name("Example".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            dir.path(),
            &build,
            &site,
            dir.path(),
            cfg,
        );
        (dir, ctx)
    }

    fn add_page(ctx: &PluginContext, stem: &str, meta: &str) {
        fs::write(
            ctx.build_dir
                .join(".meta")
                .join(format!("{stem}.meta.json")),
            meta,
        )
        .unwrap();
        fs::write(
            ctx.site_dir.join(format!("{stem}.html")),
            "<html><head><title>t</title></head><body>b</body></html>",
        )
        .unwrap();
    }

    #[test]
    fn name_is_stable() {
        assert_eq!(OembedPlugin.name(), "oembed");
        let via_default: OembedPlugin = OembedPlugin;
        assert_eq!(via_default.name(), "oembed");
    }

    #[test]
    fn opts_into_transform_pass() {
        assert!(OembedPlugin.has_transform());
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "p", r#"{"title":"P"}"#);
        let ctx = ctx.with_dry_run(true);
        OembedPlugin.after_compile(&ctx).unwrap();
        assert!(!ctx.site_dir.join("p.oembed.json").exists());
    }

    #[test]
    fn missing_site_dir_is_noop() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        let ctx =
            PluginContext::new(dir.path(), dir.path(), &missing, dir.path());
        OembedPlugin.after_compile(&ctx).unwrap();
        assert!(!missing.exists());
    }

    #[test]
    fn emits_sibling_document_per_public_page() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "post", r#"{"title":"Post","author":"a@b.c (Jane)"}"#);
        OembedPlugin.after_compile(&ctx).unwrap();
        let body =
            fs::read_to_string(ctx.site_dir.join("post.oembed.json")).unwrap();
        let doc: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(doc["version"], "1.0");
        assert_eq!(doc["type"], "link");
        assert_eq!(doc["title"], "Post");
        assert_eq!(doc["provider_name"], "Example");
        assert_eq!(doc["provider_url"], "https://example.com");
        assert_eq!(doc["author_name"], "Jane");
        assert!(body.ends_with('\n'));
    }

    #[test]
    fn skips_pages_without_rendered_html() {
        let (_tmp, ctx) = make_ctx();
        // Sidecar exists but no HTML on disk.
        fs::write(
            ctx.build_dir.join(".meta/ghost.meta.json"),
            r#"{"title":"Ghost"}"#,
        )
        .unwrap();
        OembedPlugin.after_compile(&ctx).unwrap();
        assert!(!ctx.site_dir.join("ghost.oembed.json").exists());
    }

    #[test]
    fn skips_drafts() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "d", r#"{"title":"D","draft":true}"#);
        OembedPlugin.after_compile(&ctx).unwrap();
        assert!(!ctx.site_dir.join("d.oembed.json").exists());
    }

    #[test]
    fn nested_pages_get_nested_siblings() {
        let (_tmp, ctx) = make_ctx();
        fs::create_dir_all(ctx.build_dir.join(".meta/blog")).unwrap();
        fs::create_dir_all(ctx.site_dir.join("blog")).unwrap();
        add_page(&ctx, "blog/deep", r#"{"title":"Deep"}"#);
        OembedPlugin.after_compile(&ctx).unwrap();
        assert!(ctx.site_dir.join("blog/deep.oembed.json").exists());
    }

    #[test]
    fn output_is_byte_identical_across_runs() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "p", r#"{"title":"P"}"#);
        OembedPlugin.after_compile(&ctx).unwrap();
        let first =
            fs::read_to_string(ctx.site_dir.join("p.oembed.json")).unwrap();
        OembedPlugin.after_compile(&ctx).unwrap();
        let second =
            fs::read_to_string(ctx.site_dir.join("p.oembed.json")).unwrap();
        assert_eq!(first, second);
    }

    // -----------------------------------------------------------------
    // transform_html — discovery link injection
    // -----------------------------------------------------------------

    #[test]
    fn transform_injects_discovery_link() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "post", r#"{"title":"Post"}"#);
        OembedPlugin.after_compile(&ctx).unwrap();

        let path = ctx.site_dir.join("post.html");
        let html = fs::read_to_string(&path).unwrap();
        let out = OembedPlugin.transform_html(&html, &path, &ctx).unwrap();
        assert!(out.contains("application/json+oembed"));
        assert!(out.contains("href=\"https://example.com/post.oembed.json\""));
        assert!(out.contains("title=\"Post\""));
        // Link must land inside <head>.
        let link = out.find("json+oembed").unwrap();
        let head_end = out.find("</head>").unwrap();
        assert!(link < head_end);
    }

    #[test]
    fn transform_is_idempotent() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "post", r#"{"title":"Post"}"#);
        OembedPlugin.after_compile(&ctx).unwrap();
        let path = ctx.site_dir.join("post.html");
        let html = fs::read_to_string(&path).unwrap();
        let once = OembedPlugin.transform_html(&html, &path, &ctx).unwrap();
        let twice = OembedPlugin.transform_html(&once, &path, &ctx).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn transform_skips_pages_without_sibling() {
        let (_tmp, ctx) = make_ctx();
        let path = ctx.site_dir.join("plain.html");
        let html = "<html><head></head><body></body></html>";
        fs::write(&path, html).unwrap();
        let out = OembedPlugin.transform_html(html, &path, &ctx).unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn transform_skips_html_without_head() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "post", r#"{"title":"Post"}"#);
        OembedPlugin.after_compile(&ctx).unwrap();
        let path = ctx.site_dir.join("post.html");
        let html = "<p>fragment only</p>";
        let out = OembedPlugin.transform_html(html, &path, &ctx).unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn transform_escapes_title_attribute() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "post", r#"{"title":"A \"quoted\" & <tagged>"}"#);
        OembedPlugin.after_compile(&ctx).unwrap();
        let path = ctx.site_dir.join("post.html");
        let html = fs::read_to_string(&path).unwrap();
        let out = OembedPlugin.transform_html(&html, &path, &ctx).unwrap();
        assert!(out.contains("A &quot;quoted&quot; &amp; &lt;tagged>"));
    }

    // -----------------------------------------------------------------
    // helpers
    // -----------------------------------------------------------------

    #[test]
    fn build_oembed_omits_empty_fields() {
        let doc = build_oembed("T", None, None, None);
        assert!(doc.get("provider_name").is_none());
        assert!(doc.get("provider_url").is_none());
        assert!(doc.get("author_name").is_none());
        let doc = build_oembed("T", None, Some(""), Some(""));
        assert!(doc.get("provider_name").is_none());
        assert!(doc.get("provider_url").is_none());
    }

    #[test]
    fn page_rel_path_handles_absolute_and_relative() {
        assert_eq!(
            page_rel_path("https://x.example/blog/a.html").as_deref(),
            Some("blog/a.html")
        );
        assert_eq!(page_rel_path("/a.html").as_deref(), Some("a.html"));
        assert_eq!(
            page_rel_path("https://x.example/contact/").as_deref(),
            Some("contact/index.html"),
            "pretty URLs resolve to their index.html"
        );
        assert_eq!(page_rel_path("https://x.example/"), None);
        assert_eq!(page_rel_path("/"), None);
    }

    #[test]
    fn attr_escape_covers_specials() {
        assert_eq!(attr_escape(r#"a&"<"#), "a&amp;&quot;&lt;");
    }

    #[test]
    fn read_title_missing_or_invalid_is_none() {
        let dir = tempdir().unwrap();
        assert!(read_title(&dir.path().join("nope.json")).is_none());
        let bad = dir.path().join("bad.json");
        fs::write(&bad, "not json").unwrap();
        assert!(read_title(&bad).is_none());
    }

    #[test]
    fn read_title_valid_json_without_title_field_is_none() {
        // Valid JSON that parses fine but has no "title" key: the
        // `doc.get("title")` lookup itself must return `None` (as
        // opposed to the parse-failure path already covered above).
        let dir = tempdir().unwrap();
        let f = dir.path().join("no_title.json");
        fs::write(&f, r#"{"version":"1.0","type":"link"}"#).unwrap();
        assert!(read_title(&f).is_none());
    }

    #[test]
    fn page_rel_path_none_when_scheme_url_has_no_path() {
        // Scheme present but no `/` after the host at all.
        assert_eq!(page_rel_path("https://example.com"), None);
    }

    #[test]
    fn build_oembed_omits_author_when_name_unparseable() {
        // `<email>` form yields no display name, so author_name is
        // absent.
        let doc = build_oembed("T", Some("<jane@example.com>"), None, None);
        assert!(doc.get("author_name").is_none());
    }

    #[test]
    fn after_compile_fails_when_oembed_path_squatted_by_dir() {
        let (_tmp, ctx) = make_ctx();
        add_page(&ctx, "p", r#"{"title":"P"}"#);
        fs::create_dir_all(ctx.site_dir.join("p.oembed.json")).unwrap();
        let err = OembedPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    fn transform_html_without_config_uses_root_relative_href() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("p.oembed.json"), r#"{"title":"P"}"#).unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());

        let html = "<html><head></head><body>x</body></html>";
        let out = OembedPlugin
            .transform_html(html, &site.join("p.html"), &ctx)
            .unwrap();
        assert!(out.contains("href=\"/p.oembed.json\""));
    }

    #[test]
    fn transform_html_path_outside_site_dir_uses_full_path_as_rel() {
        // `path.strip_prefix(&ctx.site_dir).unwrap_or(path)` — when the
        // page path isn't actually nested under `ctx.site_dir`,
        // `strip_prefix` fails and the fallback returns `path`
        // unchanged. This is otherwise never exercised because every
        // other test's page path lives under the context's site_dir.
        let (_tmp, ctx) = make_ctx();
        let elsewhere = tempdir().unwrap();
        let path = elsewhere.path().join("post.html");
        let sibling = path.with_extension("oembed.json");
        fs::write(&sibling, r#"{"title":"Elsewhere"}"#).unwrap();

        let html = "<html><head></head><body>x</body></html>";
        let out = OembedPlugin.transform_html(html, &path, &ctx).unwrap();
        assert!(out.contains("application/json+oembed"));
        assert!(out.contains("title=\"Elsewhere\""));
    }
}
