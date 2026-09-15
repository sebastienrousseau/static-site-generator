// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! SEO meta tag injection plugin.

use super::helpers::{
    escape_attr, extract_canonical, extract_description, extract_existing_meta,
    extract_first_content_image, extract_title, has_meta_tag,
};
use super::lang::resolve_page_lang;
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use anyhow::Result;
use std::path::Path;

/// Injects missing SEO meta tags into HTML files.
///
/// After compilation, this plugin scans all HTML files in the site
/// directory and adds any missing meta tags for description, Open Graph
/// (title, description, type), and Twitter Card.
///
/// The plugin is idempotent — it checks for existing tags before
/// injecting and will not duplicate them.
///
/// # Example
///
/// ```rust
/// use ssg::plugin::PluginManager;
/// use ssg::seo::SeoPlugin;
///
/// let mut pm = PluginManager::new();
/// pm.register(SeoPlugin);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct SeoPlugin;

impl Plugin for SeoPlugin {
    fn name(&self) -> &'static str {
        "seo"
    }

    fn has_transform(&self) -> bool {
        true
    }

    fn transform_html(
        &self,
        html: &str,
        path: &Path,
        ctx: &PluginContext,
    ) -> std::result::Result<String, crate::error::SsgError> {
        // spec A5 (plan §2 1.5): resolve the page language once so
        // `og:locale` agrees with JSON-LD `inLanguage` and the other
        // language sinks.
        let lang = resolve_page_lang(html, path, ctx);
        // spec B8: resolve the social-meta derivation cascade from
        // THIS page's front matter (never global config) before
        // falling back to what the rendered HTML provides.
        let social = resolve_social_meta(html, path, ctx);
        inject_seo_tags_html(html, &lang, &social)
            .map_err(|e| crate::error::SsgError::io(e, path))
    }

    fn after_compile(
        &self,
        _ctx: &PluginContext,
    ) -> std::result::Result<(), crate::error::SsgError> {
        Ok(())
    }
}

/// Per-page social metadata resolved through the spec-B8 derivation
/// cascade.
///
/// Every og/twitter field derives from the page's own front matter
/// (via its `.meta.json` sidecar) when not explicitly set:
///
/// - `og:title` / `twitter:title` ⇐ `og_title`/`twitter_title` ⇐
///   `seo_title` ⇐ `title` ⇐ rendered `<title>`
/// - `og:description` / `twitter:description` ⇐
///   `og_description`/`twitter_description` ⇐ `description` ⇐
///   extracted page text
/// - `og:image` / `twitter:image` ⇐ `og_image`/`twitter_image` ⇐
///   `banner` ⇐ `image` ⇐ existing meta / first content `<img>`
/// - `twitter:card` ⇐ `twitter_card` ⇐ sensible default
///   (`summary_large_image` when an image exists, else `summary`)
///
/// Explicit per-field front matter always wins over the derived
/// value. All values come from THIS page's sidecar — never from
/// global config — which kills the stale-`twitter_title` bug class
/// (spec B8): two pages differing only in `title` must never share a
/// social title.
#[derive(Debug, Clone, Default)]
struct SocialMeta {
    /// Resolved `og:title` text.
    og_title: String,
    /// Resolved `twitter:title` text.
    twitter_title: String,
    /// Resolved plain `<meta name="description">` text.
    description: String,
    /// Resolved `og:description` text.
    og_description: String,
    /// Resolved `twitter:description` text.
    twitter_description: String,
    /// Front-matter-derived `og:image`, when any of
    /// `og_image`/`banner`/`image` is set on the page.
    og_image: Option<String>,
    /// Front-matter-derived `twitter:image`, when any of
    /// `twitter_image`/`banner`/`image` is set on the page.
    twitter_image: Option<String>,
    /// Explicit front-matter `twitter_card`, when set.
    twitter_card: Option<String>,
}

/// Resolves the spec-B8 social-meta cascade for one page.
///
/// Front-matter values come from the page's own `.meta.json` sidecar
/// (the same per-page source the A5 language resolver uses), so the
/// derivation can never bleed one page's values into another or fall
/// back to site-wide config strings.
fn resolve_social_meta(
    html: &str,
    path: &Path,
    ctx: &PluginContext,
) -> SocialMeta {
    let rel_path = path
        .strip_prefix(&ctx.site_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let meta = super::lang::read_page_sidecar(path, ctx, &rel_path);

    // First non-empty string among `keys`, in cascade order.
    let fm = |keys: &[&str]| -> Option<String> {
        let m = meta.as_ref()?;
        keys.iter().find_map(|key| {
            m.get(*key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
    };

    let html_title = extract_title(html);
    let html_description = extract_description(html, 160);

    SocialMeta {
        og_title: fm(&["og_title", "seo_title", "title"])
            .unwrap_or_else(|| html_title.clone()),
        twitter_title: fm(&["twitter_title", "seo_title", "title"])
            .unwrap_or(html_title),
        description: fm(&["description"])
            .unwrap_or_else(|| html_description.clone()),
        og_description: fm(&["og_description", "description"])
            .unwrap_or_else(|| html_description.clone()),
        twitter_description: fm(&["twitter_description", "description"])
            .unwrap_or(html_description),
        og_image: fm(&["og_image", "banner", "image"]),
        twitter_image: fm(&["twitter_image", "banner", "image"]),
        twitter_card: fm(&["twitter_card"]),
    }
}

/// Builds Open Graph meta tags that are missing from the HTML.
///
/// `lang` is the already-resolved page language from
/// [`resolve_page_lang`] (spec A5, plan §2 1.5) in canonical BCP-47
/// hyphen form; the Open Graph underscore spelling (`en_GB`) is
/// produced at this sink only. Titles, descriptions, and images come
/// from the resolved [`SocialMeta`] cascade (spec B8).
fn build_og_tags(
    html: &str,
    social: &SocialMeta,
    canonical: &str,
    og_type: &str,
    lang: &str,
) -> Vec<String> {
    let mut tags = Vec::new();

    if !has_meta_tag(html, "og:title") && !social.og_title.is_empty() {
        tags.push(format!(
            "<meta property=\"og:title\" content=\"{}\">",
            escape_attr(&social.og_title)
        ));
    }

    if !has_meta_tag(html, "og:description")
        && !social.og_description.is_empty()
    {
        tags.push(format!(
            "<meta property=\"og:description\" content=\"{}\">",
            escape_attr(&social.og_description)
        ));
    }

    if !has_meta_tag(html, "og:type") {
        tags.push(format!("<meta property=\"og:type\" content=\"{og_type}\">"));
    }

    if !has_meta_tag(html, "og:url") && !canonical.is_empty() {
        tags.push(format!(
            "<meta property=\"og:url\" content=\"{}\">",
            escape_attr(canonical)
        ));
    }

    // OG image (spec B8): front matter (og_image ⇐ banner ⇐ image)
    // first, then existing meta, then first <img> in content.
    if !has_meta_tag(html, "og:image") {
        let image = social.og_image.clone().unwrap_or_else(|| {
            let existing = extract_existing_meta(html, "twitter:image");
            if existing.is_empty() {
                extract_first_content_image(html)
            } else {
                existing
            }
        });
        if !image.is_empty() {
            tags.push(format!(
                "<meta property=\"og:image\" content=\"{}\">",
                escape_attr(&image)
            ));
            // Social platforms render cards faster with explicit dimensions
            if !has_meta_tag(html, "og:image:width") {
                tags.push(
                    "<meta property=\"og:image:width\" content=\"1200\">"
                        .to_string(),
                );
                tags.push(
                    "<meta property=\"og:image:height\" content=\"630\">"
                        .to_string(),
                );
            }
        }
    }

    // OG locale — always emitted from the resolved page language
    // (spec A5): the resolver never returns an empty value, and the
    // hyphen→underscore conversion happens only at this sink so the
    // canonical form stays BCP-47 everywhere else.
    if !has_meta_tag(html, "og:locale") {
        let locale = lang.replace('-', "_");
        tags.push(format!(
            "<meta property=\"og:locale\" content=\"{}\">",
            escape_attr(&locale)
        ));
    }

    tags
}

/// Builds Twitter Card meta tags that are missing from the HTML.
///
/// Titles, descriptions, and images come from the resolved
/// [`SocialMeta`] cascade (spec B8); `twitter_card` is the
/// already-resolved card type (explicit front matter beats the
/// derived default).
fn build_twitter_tags(
    html: &str,
    social: &SocialMeta,
    twitter_card: &str,
) -> Vec<String> {
    let mut tags = Vec::new();

    if !has_meta_tag(html, "twitter:card") {
        tags.push(format!(
            "<meta name=\"twitter:card\" content=\"{}\">",
            escape_attr(twitter_card)
        ));
    }

    if !has_meta_tag(html, "twitter:title") && !social.twitter_title.is_empty()
    {
        tags.push(format!(
            "<meta name=\"twitter:title\" content=\"{}\">",
            escape_attr(&social.twitter_title)
        ));
    }

    if !has_meta_tag(html, "twitter:description")
        && !social.twitter_description.is_empty()
    {
        tags.push(format!(
            "<meta name=\"twitter:description\" content=\"{}\">",
            escape_attr(&social.twitter_description)
        ));
    }

    // Twitter image (spec B8): front matter (twitter_image ⇐ banner
    // ⇐ image) first, then existing meta, then first content <img>.
    if !has_meta_tag(html, "twitter:image") {
        let image = social.twitter_image.clone().unwrap_or_else(|| {
            let existing = extract_existing_meta(html, "og:image");
            if existing.is_empty() {
                extract_first_content_image(html)
            } else {
                existing
            }
        });
        if !image.is_empty() {
            tags.push(format!(
                "<meta name=\"twitter:image\" content=\"{}\">",
                escape_attr(&image)
            ));
        }
    }

    tags
}

/// Builds the meta description tag if missing from the HTML.
fn build_meta_description(html: &str, description: &str) -> Option<String> {
    if !has_meta_tag(html, "description") && !description.is_empty() {
        Some(format!(
            "<meta name=\"description\" content=\"{}\">",
            escape_attr(description)
        ))
    } else {
        None
    }
}

/// Inject missing SEO meta tags into an HTML string, returning the
/// modified HTML. `lang` is the resolved page language (spec A5);
/// `social` is the page's resolved front-matter cascade (spec B8).
fn inject_seo_tags_html(
    html: &str,
    lang: &str,
    social: &SocialMeta,
) -> Result<String> {
    fail_point!("seo::inject-tags", |_| {
        Err(anyhow::anyhow!("injected: seo::inject-tags"))
    });

    let canonical = extract_canonical(html);

    let is_article = html.contains("<article");
    let og_type = if is_article { "article" } else { "website" };

    // spec B8: explicit front-matter `twitter_card` wins; otherwise
    // `summary_large_image` when the page resolves an image (from
    // front matter, existing meta, or content) or is an article, and
    // `summary` as the final default.
    let has_image = social.og_image.is_some()
        || social.twitter_image.is_some()
        || !extract_existing_meta(html, "og:image").is_empty()
        || !extract_existing_meta(html, "twitter:image").is_empty()
        || !extract_first_content_image(html).is_empty();
    let derived_card = if has_image || is_article {
        "summary_large_image"
    } else {
        "summary"
    };
    let twitter_card = social.twitter_card.as_deref().unwrap_or(derived_card);

    let mut tags = Vec::new();

    if let Some(meta_desc) = build_meta_description(html, &social.description) {
        tags.push(meta_desc);
    }
    tags.extend(build_og_tags(html, social, &canonical, og_type, lang));
    tags.extend(build_twitter_tags(html, social, twitter_card));

    if tags.is_empty() {
        return Ok(html.to_string());
    }

    let injection = format!("{}\n", tags.join("\n"));
    Ok(inject_before_head_close(html, &injection))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    fn ctx(site: &Path) -> PluginContext {
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            site,
            Path::new("templates"),
        )
    }

    /// A `SocialMeta` with identical og/twitter title + description,
    /// as the cascade produces when only base fields exist.
    fn social(title: &str, desc: &str) -> SocialMeta {
        SocialMeta {
            og_title: title.to_string(),
            twitter_title: title.to_string(),
            description: desc.to_string(),
            og_description: desc.to_string(),
            twitter_description: desc.to_string(),
            ..SocialMeta::default()
        }
    }

    #[test]
    fn name_is_stable() {
        assert_eq!(SeoPlugin.name(), "seo");
    }

    #[test]
    #[serial_test::parallel]
    fn no_op_when_site_dir_missing() {
        let dir = tempdir().unwrap();
        SeoPlugin
            .after_compile(&ctx(&dir.path().join("nope")))
            .unwrap();
    }

    // ── build_meta_description ──────────────────────────────────

    #[test]
    fn meta_description_built_when_missing_and_text_provided() {
        let html = r#"<html><head><title>X</title></head><body></body></html>"#;
        let out = build_meta_description(html, "A cool page");
        assert_eq!(
            out.as_deref(),
            Some(r#"<meta name="description" content="A cool page">"#)
        );
    }

    #[test]
    fn meta_description_skipped_when_empty_text() {
        let html = "<html><head></head></html>";
        assert!(build_meta_description(html, "").is_none());
    }

    #[test]
    fn meta_description_skipped_when_already_present() {
        let html = r#"<html><head><meta name="description" content="X"></head></html>"#;
        assert!(build_meta_description(html, "Override?").is_none());
    }

    #[test]
    fn meta_description_escapes_attribute_value() {
        let html = "<html><head></head></html>";
        let out = build_meta_description(html, r#"X & "Y" <Z>"#).unwrap();
        // No raw `&`, raw `"` between content="...", or raw `<` in attribute.
        assert!(out.contains("content="));
        assert!(!out.contains(r#"content="X & ""#));
    }

    // ── build_og_tags ───────────────────────────────────────────

    #[test]
    fn og_tags_includes_title_description_type_url() {
        let html = "<html lang=\"en\"><head></head></html>";
        let tags = build_og_tags(
            html,
            &social("Hello", "World"),
            "https://example.com/page",
            "website",
            "en",
        );
        let joined = tags.join("\n");
        assert!(joined.contains(r#"property="og:title" content="Hello""#));
        assert!(joined.contains(r#"property="og:description" content="World""#));
        assert!(joined.contains(r#"property="og:type" content="website""#));
        assert!(joined.contains(
            r#"property="og:url" content="https://example.com/page""#
        ));
        assert!(joined.contains(r#"property="og:locale" content="en""#));
    }

    #[test]
    fn og_tags_skips_existing_tags() {
        let html = r#"<html lang="en"><head>
            <meta property="og:title" content="Existing">
            <meta property="og:type" content="article">
        </head></html>"#;
        let tags = build_og_tags(
            html,
            &social("Hello", "World"),
            "https://example.com",
            "website",
            "en",
        );
        let joined = tags.join("\n");
        assert!(
            !joined.contains(r#"property="og:title""#),
            "should not duplicate og:title: {joined}"
        );
        assert!(
            !joined.contains(r#"property="og:type""#),
            "should not duplicate og:type"
        );
    }

    #[test]
    fn og_tags_falls_back_from_twitter_image_when_og_image_missing() {
        let html = r#"<html><head>
            <meta name="twitter:image" content="/twit.png">
        </head></html>"#;
        let tags = build_og_tags(html, &social("T", "D"), "", "website", "en");
        let joined = tags.join("\n");
        assert!(
            joined.contains(r#"property="og:image" content="/twit.png""#),
            "should reuse twitter:image when og:image absent: {joined}"
        );
        // and emit explicit dimensions for fast social card render
        assert!(joined.contains(r#"property="og:image:width" content="1200""#));
        assert!(joined.contains(r#"property="og:image:height" content="630""#));
    }

    #[test]
    fn og_tags_locale_translates_resolved_lang_dashes_to_underscores() {
        // spec A5: og:locale is the resolver's BCP-47 output with the
        // hyphen→underscore conversion applied at this sink only.
        let html = "<html lang=\"en-GB\"><head></head></html>";
        let tags =
            build_og_tags(html, &social("T", "D"), "", "website", "en-GB");
        let joined = tags.join("\n");
        assert!(
            joined.contains(r#"property="og:locale" content="en_GB""#),
            "resolved en-GB should produce og:locale=\"en_GB\", got: {joined}"
        );
    }

    #[test]
    fn og_tags_always_emits_resolved_locale() {
        // Pre-A5 behaviour omitted og:locale when <html lang> was
        // missing; the resolver always produces a value now, so the
        // tag is always present and agrees with JSON-LD inLanguage.
        let html = "<html><head></head></html>";
        let tags = build_og_tags(html, &social("T", "D"), "", "website", "hi");
        let joined = tags.join("\n");
        assert!(
            joined.contains(r#"property="og:locale" content="hi""#),
            "resolved lang must always be emitted, got: {joined}"
        );
    }

    // ── build_twitter_tags ──────────────────────────────────────

    #[test]
    fn twitter_tags_includes_card_title_description() {
        let html = "<html><head></head></html>";
        let tags = build_twitter_tags(html, &social("T", "D"), "summary");
        let joined = tags.join("\n");
        assert!(joined.contains(r#"name="twitter:card" content="summary""#));
        assert!(joined.contains(r#"name="twitter:title" content="T""#));
        assert!(joined.contains(r#"name="twitter:description" content="D""#));
    }

    #[test]
    fn twitter_tags_falls_back_to_og_image_when_twitter_image_missing() {
        let html = r#"<html><head>
            <meta property="og:image" content="/og.png">
        </head></html>"#;
        let tags = build_twitter_tags(html, &social("T", "D"), "summary");
        let joined = tags.join("\n");
        assert!(
            joined.contains(r#"name="twitter:image" content="/og.png""#),
            "should reuse og:image when twitter:image absent: {joined}"
        );
    }

    // ── inject_seo_tags integration via after_compile ───────────

    #[test]
    #[serial_test::parallel]
    fn transform_html_injects_tags() {
        let dir = tempdir().unwrap();
        let c = ctx(dir.path());

        let html = r#"<!doctype html><html lang="en"><head><title>Hello</title></head>
            <body><p>World is wide.</p></body></html>"#;

        let after = SeoPlugin
            .transform_html(html, Path::new("page.html"), &c)
            .unwrap();
        assert!(after.contains("og:title"));
        assert!(after.contains("twitter:card"));
        assert!(after.contains("name=\"description\""));
    }

    #[test]
    #[serial_test::parallel]
    fn transform_html_uses_article_type_when_article_tag_present() {
        let dir = tempdir().unwrap();
        let c = ctx(dir.path());

        let html = r#"<!doctype html><html lang="en"><head><title>P</title></head>
            <body><article><p>Content.</p></article></body></html>"#;

        let after = SeoPlugin
            .transform_html(html, Path::new("post.html"), &c)
            .unwrap();
        assert!(
            after.contains(r#"og:type" content="article""#),
            "presence of <article> should set og:type=article: {after}"
        );
        assert!(
            after.contains(r#"twitter:card" content="summary_large_image""#),
            "article should use summary_large_image twitter card: {after}"
        );
    }

    #[test]
    #[serial_test::parallel]
    fn transform_html_is_idempotent() {
        let dir = tempdir().unwrap();
        let c = ctx(dir.path());

        let html = r#"<html lang="en"><head><title>Y</title></head><body>Z</body></html>"#;

        let first = SeoPlugin
            .transform_html(html, Path::new("x.html"), &c)
            .unwrap();
        let second = SeoPlugin
            .transform_html(&first, Path::new("x.html"), &c)
            .unwrap();
        assert_eq!(first, second, "second run must not duplicate meta tags");
    }

    #[cfg(feature = "i18n")]
    #[test]
    #[serial_test::parallel]
    fn after_compile_no_op_when_no_html_files() {
        let dir = tempdir().unwrap();
        // Site dir exists but is empty.
        SeoPlugin.after_compile(&ctx(dir.path())).unwrap();
    }

    // ── og:locale via resolve_page_lang (spec A5, plan §2 1.5) ──

    /// Context with a site `language` and declared `[i18n]` locales.
    #[cfg(feature = "i18n")]
    fn locale_ctx(
        site: &Path,
        language: &str,
        locales: &[&str],
    ) -> PluginContext {
        let mut c = ctx(site);
        c.config = Some(crate::cmd::SsgConfig {
            language: language.to_string(),
            i18n: Some(crate::i18n::I18nConfig {
                default_locale: locales
                    .first()
                    .map_or_else(|| "en".to_string(), |l| (*l).to_string()),
                locales: locales.iter().map(|l| (*l).to_string()).collect(),
                url_prefix: Default::default(),
            }),
            ..crate::cmd::SsgConfig::default()
        });
        c
    }

    #[cfg(feature = "i18n")]
    #[test]
    #[serial_test::parallel]
    fn og_locale_is_path_driven_on_locale_pages() {
        // The A5 signature bug: a /hi/… page carrying the site-wide
        // lang="en-GB" must emit og:locale=hi, not en_GB.
        let dir = tempdir().unwrap();
        let c = locale_ctx(dir.path(), "en-GB", &["en", "hi"]);
        let html = r#"<html lang="en-GB"><head><title>T</title></head><body>x</body></html>"#;
        let page = dir.path().join("hi/2026-06-01-post/index.html");
        let out = SeoPlugin.transform_html(html, &page, &c).unwrap();
        assert!(
            out.contains(r#"property="og:locale" content="hi""#),
            "expected path-driven og:locale=hi, got: {out}"
        );
    }

    #[cfg(feature = "i18n")]
    #[test]
    #[serial_test::parallel]
    fn og_locale_is_default_driven_with_underscore_form() {
        // en-GB default: og:locale uses the underscore spelling while
        // the resolver stays canonical BCP-47 (en-GB).
        let dir = tempdir().unwrap();
        let c = locale_ctx(dir.path(), "en-GB", &["en"]);
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let page = dir.path().join("about/index.html");
        let out = SeoPlugin.transform_html(html, &page, &c).unwrap();
        assert!(
            out.contains(r#"property="og:locale" content="en_GB""#),
            "expected default-driven og:locale=en_GB, got: {out}"
        );
    }

    #[test]
    #[serial_test::parallel]
    fn og_locale_en_fallback_only_when_nothing_resolves() {
        let dir = tempdir().unwrap();
        // No config, no sidecar, no locale prefix, no <html lang>.
        let c = ctx(dir.path());
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let page = dir.path().join("index.html");
        let out = SeoPlugin.transform_html(html, &page, &c).unwrap();
        assert!(
            out.contains(r#"property="og:locale" content="en""#),
            "expected final-constant og:locale=en, got: {out}"
        );
    }

    // ── spec B8: social-meta derivation cascade ─────────────────

    /// Context rooted in `dir` so `<dir>/build/.meta` sidecars are
    /// found for pages under `<dir>/site`.
    fn ctx_rooted(dir: &Path) -> PluginContext {
        PluginContext::new(
            Path::new("content"),
            &dir.join("build"),
            &dir.join("site"),
            Path::new("templates"),
        )
    }

    /// Writes a front-matter sidecar for the site-relative page `rel`.
    fn write_sidecar(dir: &Path, rel: &str, json: &str) {
        let sidecar = dir
            .join("build")
            .join(".meta")
            .join(rel)
            .with_extension("meta.json");
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(sidecar, json).unwrap();
    }

    fn meta_content(html: &str, attr: &str) -> String {
        extract_existing_meta(html, attr)
    }

    #[test]
    #[serial_test::parallel]
    fn b8_title_description_banner_yield_complete_consistent_social_set() {
        // Acceptance (spec B8): a post with ONLY title + description
        // + banner gets complete, mutually consistent og:*/twitter:*.
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "post/index.html",
            r#"{"title":"My Post","description":"A fine description","banner":"/img/banner.webp"}"#,
        );
        let c = ctx_rooted(dir.path());
        let html = "<html lang=\"en\"><head><title>My Post</title></head><body><p>text</p></body></html>";
        let page = dir.path().join("site/post/index.html");
        let out = SeoPlugin.transform_html(html, &page, &c).unwrap();

        assert_eq!(meta_content(&out, "og:title"), "My Post");
        assert_eq!(meta_content(&out, "twitter:title"), "My Post");
        assert_eq!(meta_content(&out, "og:description"), "A fine description");
        assert_eq!(
            meta_content(&out, "twitter:description"),
            "A fine description"
        );
        assert_eq!(meta_content(&out, "description"), "A fine description");
        assert_eq!(meta_content(&out, "og:image"), "/img/banner.webp");
        assert_eq!(meta_content(&out, "twitter:image"), "/img/banner.webp");
        // Image present ⇒ summary_large_image.
        assert_eq!(meta_content(&out, "twitter:card"), "summary_large_image");
        // Mutual consistency: og and twitter agree everywhere.
        assert_eq!(
            meta_content(&out, "og:title"),
            meta_content(&out, "twitter:title")
        );
        assert_eq!(
            meta_content(&out, "og:image"),
            meta_content(&out, "twitter:image")
        );
    }

    #[test]
    #[serial_test::parallel]
    fn b8_seo_title_beats_title_and_explicit_fields_beat_seo_title() {
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "p/index.html",
            r#"{"title":"Base","seo_title":"Seo Title","twitter_title":"Tw Title"}"#,
        );
        let c = ctx_rooted(dir.path());
        let html =
            "<html><head><title>Base</title></head><body>x</body></html>";
        let page = dir.path().join("site/p/index.html");
        let out = SeoPlugin.transform_html(html, &page, &c).unwrap();

        // twitter_title (explicit) ⇐ seo_title ⇐ title
        assert_eq!(meta_content(&out, "twitter:title"), "Tw Title");
        // og_title unset ⇒ seo_title wins over title.
        assert_eq!(meta_content(&out, "og:title"), "Seo Title");
    }

    #[test]
    #[serial_test::parallel]
    fn b8_banner_beats_image_and_og_image_beats_banner() {
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "a/index.html",
            r#"{"title":"T","image":"/i.png"}"#,
        );
        write_sidecar(
            dir.path(),
            "b/index.html",
            r#"{"title":"T","image":"/i.png","banner":"/b.png"}"#,
        );
        write_sidecar(
            dir.path(),
            "c/index.html",
            r#"{"title":"T","banner":"/b.png","og_image":"/og.png"}"#,
        );
        let c = ctx_rooted(dir.path());
        let html = "<html><head><title>T</title></head><body>x</body></html>";

        let out_a = SeoPlugin
            .transform_html(html, &dir.path().join("site/a/index.html"), &c)
            .unwrap();
        assert_eq!(meta_content(&out_a, "og:image"), "/i.png");
        assert_eq!(meta_content(&out_a, "twitter:image"), "/i.png");

        let out_b = SeoPlugin
            .transform_html(html, &dir.path().join("site/b/index.html"), &c)
            .unwrap();
        assert_eq!(meta_content(&out_b, "og:image"), "/b.png");

        let out_c = SeoPlugin
            .transform_html(html, &dir.path().join("site/c/index.html"), &c)
            .unwrap();
        assert_eq!(meta_content(&out_c, "og:image"), "/og.png");
        // twitter_image not explicitly set ⇒ banner still wins there.
        assert_eq!(meta_content(&out_c, "twitter:image"), "/b.png");
    }

    #[test]
    #[serial_test::parallel]
    fn b8_explicit_twitter_card_wins_over_derived_default() {
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "p/index.html",
            r#"{"title":"T","banner":"/b.png","twitter_card":"summary"}"#,
        );
        let c = ctx_rooted(dir.path());
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let out = SeoPlugin
            .transform_html(html, &dir.path().join("site/p/index.html"), &c)
            .unwrap();
        // Image present would derive summary_large_image, but the
        // explicit front-matter field always wins (spec B8).
        assert_eq!(meta_content(&out, "twitter:card"), "summary");
    }

    #[test]
    #[serial_test::parallel]
    fn b8_card_defaults_to_summary_without_image_or_article() {
        let dir = tempdir().unwrap();
        let c = ctx_rooted(dir.path());
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let out = SeoPlugin
            .transform_html(html, &dir.path().join("site/p/index.html"), &c)
            .unwrap();
        assert_eq!(meta_content(&out, "twitter:card"), "summary");
    }

    #[test]
    #[serial_test::parallel]
    fn b8_no_bleed_between_pages_differing_only_in_title() {
        // The stale-field bug class (spec B8): derived values must
        // come from THIS page's front matter, never another page's or
        // global config.
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "alpha/index.html",
            r#"{"title":"Alpha Page","description":"same","banner":"/same.png"}"#,
        );
        write_sidecar(
            dir.path(),
            "beta/index.html",
            r#"{"title":"Beta Page","description":"same","banner":"/same.png"}"#,
        );
        let mut c = ctx_rooted(dir.path());
        // Global config with a site name that must never leak into
        // per-page social titles.
        c.config = Some(crate::cmd::SsgConfig {
            site_name: "Global Site Name".to_string(),
            ..crate::cmd::SsgConfig::default()
        });
        let html = "<html><head><title>t</title></head><body>x</body></html>";

        let out_a = SeoPlugin
            .transform_html(html, &dir.path().join("site/alpha/index.html"), &c)
            .unwrap();
        let out_b = SeoPlugin
            .transform_html(html, &dir.path().join("site/beta/index.html"), &c)
            .unwrap();

        assert_eq!(meta_content(&out_a, "og:title"), "Alpha Page");
        assert_eq!(meta_content(&out_b, "og:title"), "Beta Page");
        assert_eq!(meta_content(&out_a, "twitter:title"), "Alpha Page");
        assert_eq!(meta_content(&out_b, "twitter:title"), "Beta Page");
        assert!(!out_a.contains("Beta Page"), "page A leaked page B's title");
        assert!(
            !out_b.contains("Alpha Page"),
            "page B leaked page A's title"
        );
        assert!(
            !out_a.contains("Global Site Name"),
            "global config bled into page meta"
        );
    }

    #[test]
    #[serial_test::parallel]
    fn transform_html_handles_html_without_head_tag() {
        let dir = tempdir().unwrap();
        let c = ctx(dir.path());
        let raw = "<!doctype html><html><body>only</body></html>";
        let after = SeoPlugin
            .transform_html(raw, Path::new("frag.html"), &c)
            .unwrap();
        assert_eq!(after, raw);
    }

    #[test]
    fn og_tags_skips_image_block_when_og_image_present() {
        // An existing og:image means the whole image block (image +
        // width/height) is left alone.
        let html = r#"<html><head>
            <meta property="og:image" content="/have.png">
        </head></html>"#;
        let tags = build_og_tags(html, &social("T", "D"), "", "website", "en");
        let joined = tags.join("\n");
        assert!(
            !joined.contains("og:image"),
            "existing og:image must suppress image emission: {joined}"
        );
    }

    #[test]
    fn og_tags_skips_dimensions_when_width_already_present() {
        // og:image is missing (and derivable from twitter:image), but
        // explicit dimensions already exist — only og:image is added.
        let html = r#"<html><head>
            <meta name="twitter:image" content="/twit.png">
            <meta property="og:image:width" content="800">
            <meta property="og:image:height" content="420">
        </head></html>"#;
        let tags = build_og_tags(html, &social("T", "D"), "", "website", "en");
        let joined = tags.join("\n");
        assert!(joined.contains(r#"property="og:image" content="/twit.png""#));
        assert!(
            !joined.contains(r#"content="1200""#),
            "must not re-emit default dimensions: {joined}"
        );
    }

    #[test]
    fn twitter_tags_skips_image_when_twitter_image_present() {
        let html = r#"<html><head>
            <meta name="twitter:image" content="/have.png">
        </head></html>"#;
        let tags = build_twitter_tags(html, &social("T", "D"), "summary");
        let joined = tags.join("\n");
        assert!(
            !joined.contains("twitter:image"),
            "existing twitter:image must suppress emission: {joined}"
        );
    }

    #[cfg(feature = "i18n")]
    #[test]
    #[serial_test::parallel]
    fn og_locale_with_empty_declared_locale_set_uses_site_language() {
        // Zero declared locales: the helper's default-locale fallback
        // kicks in and the site language drives og:locale.
        let dir = tempdir().unwrap();
        let c = locale_ctx(dir.path(), "en-GB", &[]);
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let page = dir.path().join("about/index.html");
        let out = SeoPlugin.transform_html(html, &page, &c).unwrap();
        assert!(
            out.contains(r#"property="og:locale" content="en_GB""#),
            "expected og:locale=en_GB from site language, got: {out}"
        );
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use serial_test::serial;
    use std::path::Path;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard<'a>(&'a str);

    impl Drop for FailGuard<'_> {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial]
    fn transform_html_maps_injection_failure_to_io_error() {
        let _guard = FailGuard("seo::inject-tags");
        fail::cfg("seo::inject-tags", "return").unwrap();

        let dir = tempdir().unwrap();
        let c = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            dir.path(),
            Path::new("templates"),
        );
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let err = SeoPlugin
            .transform_html(html, Path::new("page.html"), &c)
            .expect_err("failpoint must abort tag injection");
        assert!(
            err.to_string().contains("seo::inject-tags"),
            "injected error should surface with its failpoint name: {err}"
        );
    }
}
