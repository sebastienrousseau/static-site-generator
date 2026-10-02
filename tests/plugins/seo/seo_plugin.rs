// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::cmd::SsgConfig;
use ssg::i18n::I18nConfig;
use ssg::plugin::{Plugin, PluginContext};
use ssg::seo::{JsonLdPlugin, SeoPlugin};
use std::path::Path;

#[test]
fn seo_plugin_name_is_stable() {
    assert!(!SeoPlugin.name().is_empty(), "should not be empty");
}

/// Builds a `PluginContext` carrying a site `language` and declared
/// `[i18n]` locales, rooted under `dir`.
fn locale_ctx(dir: &Path, language: &str, locales: &[&str]) -> PluginContext {
    PluginContext::with_config(
        Path::new("content"),
        &dir.join("build"),
        &dir.join("site"),
        Path::new("templates"),
        SsgConfig {
            language: language.to_string(),
            i18n: Some(I18nConfig {
                default_locale: locales
                    .first()
                    .map_or_else(|| "en".to_string(), |l| (*l).to_string()),
                locales: locales.iter().map(|l| (*l).to_string()).collect(),
                url_prefix: Default::default(),
            }),
            ..SsgConfig::default()
        },
    )
}

/// Extracts the `content` of the injected `og:locale` meta tag.
fn og_locale(html: &str) -> Option<&str> {
    let start = html.find(r#"property="og:locale" content=""#)?
        + r#"property="og:locale" content=""#.len();
    html[start..].find('"').map(|end| &html[start..start + end])
}

/// Extracts `inLanguage` from the injected JSON-LD block.
fn in_language(html: &str) -> Option<String> {
    let start = html.find(r#"<script type="application/ld+json">"#)?
        + r#"<script type="application/ld+json">"#.len();
    let end = html[start..].find("</script>")?;
    let v: serde_json::Value =
        serde_json::from_str(html[start..start + end].trim()).ok()?;
    v["inLanguage"].as_str().map(str::to_string)
}

/// spec A5 (plan §2 1.5): JSON-LD `inLanguage` and `og:locale` must
/// agree (modulo the Open Graph underscore form) on every page —
/// front-matter-driven, path-driven and default-driven alike.
#[test]
fn jsonld_in_language_and_og_locale_agree_per_locale() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = locale_ctx(dir.path(), "en-GB", &["en", "fr", "hi"]);
    let jsonld = JsonLdPlugin::from_site("https://example.com", "Org");

    // The site-wide template lang (the pre-A5 bug leaked this onto
    // locale pages as inLanguage=en-GB).
    let html = concat!(
        r#"<html lang="en-GB"><head><title>T</title></head>"#,
        "<body><p>content</p></body></html>"
    );

    for (rel, want) in [
        ("en/post/index.html", "en"),
        ("fr/post/index.html", "fr"),
        ("hi/2026-06-01-post/index.html", "hi"),
        ("about/index.html", "en-GB"), // default-driven, regioned
    ] {
        let page = dir.path().join("site").join(rel);

        let with_jsonld = jsonld.transform_html(html, &page, &ctx).unwrap();
        let with_og = SeoPlugin.transform_html(html, &page, &ctx).unwrap();

        let lang = in_language(&with_jsonld)
            .unwrap_or_else(|| panic!("no inLanguage on {rel}"));
        let locale = og_locale(&with_og)
            .unwrap_or_else(|| panic!("no og:locale on {rel}"));

        assert_eq!(lang, want, "inLanguage mismatch on {rel}");
        assert_eq!(
            locale,
            want.replace('-', "_"),
            "og:locale must agree with inLanguage modulo underscore on {rel}"
        );
    }
}

/// spec A5: the terminal `"en"` constant fires only when no source
/// resolves (no front matter, no locale prefix, no `<html lang>`,
/// no site config).
#[test]
fn en_fallback_fires_only_without_any_language_source() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = PluginContext::new(
        Path::new("content"),
        &dir.path().join("build"),
        &dir.path().join("site"),
        Path::new("templates"),
    );
    let html = "<html><head><title>T</title></head><body>x</body></html>";
    let page = dir.path().join("site/index.html");

    let jsonld = JsonLdPlugin::from_site("https://example.com", "Org");
    let with_jsonld = jsonld.transform_html(html, &page, &ctx).unwrap();
    let with_og = SeoPlugin.transform_html(html, &page, &ctx).unwrap();

    assert_eq!(in_language(&with_jsonld).as_deref(), Some("en"));
    assert_eq!(og_locale(&with_og), Some("en"));
}
