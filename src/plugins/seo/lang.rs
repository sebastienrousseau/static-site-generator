// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Single page-language resolver (spec A5, plan §2 1.5).
//!
//! Four sinks must agree on the language of every page: JSON-LD
//! `inLanguage`, `og:locale`, `<html lang>`, and the hreflang
//! self-reference. Before v0.0.47 each sink derived its own value —
//! JSON-LD fell back to a hard-coded `"en"` and `og:locale` copied
//! whatever `<html lang>` said, so locale pages (e.g. `/hi/…`) could
//! emit `inLanguage: "en-GB"` while living under a `hi` prefix.
//!
//! [`resolve_page_lang`] is now the one place that decides the value.
//! The resolver always answers in canonical BCP-47 hyphen form
//! (`en-GB`, `hi`); sinks that need another spelling (Open Graph's
//! underscore form `en_GB`) convert at the sink.

// This module is deliberately crate-internal (`pub(crate) mod lang`)
// so the resolver does not become public API before Wave 2 rewires
// the remaining sinks. rustc's `unreachable_pub` demands `pub(crate)`
// on the items while clippy's nursery `redundant_pub_crate` demands
// plain `pub` — silence the nursery lint and keep the honest
// `pub(crate)` spelling.
#![allow(clippy::redundant_pub_crate)]

use super::helpers::extract_html_lang;
use crate::plugin::PluginContext;
use std::fs;
use std::path::Path;

/// Final constant fallback when no other source resolves (spec A5).
///
/// This is the *only* place the historic `"en"` default survives; the
/// inline fallbacks that used to live at the JSON-LD sinks
/// (`jsonld/mod.rs:115,168` pre-v0.0.47) are gone.
pub(crate) const DEFAULT_PAGE_LANG: &str = "en";

/// Resolves the canonical BCP-47 language for a built HTML page
/// (spec A5, plan §2 1.5).
///
/// Precedence, first match wins:
///
/// 1. Front-matter `language` (via the page's `.meta.json` sidecar).
/// 2. Front-matter `hreflang` (same sidecar).
/// 3. Locale path prefix of the site-relative path
///    (`hi/2026-…/index.html` → `hi`). When an `[i18n]` section is
///    configured the prefix must be one of the declared locales;
///    without one, only a plausible `xx` / `xx-XX` shaped prefix is
///    accepted so arbitrary first directories (`blog/`, `api/`) are
///    never mistaken for locales.
/// 4. The `<html lang>` attribute already on the rendered page.
///    Interim signal: the `<html lang>` emitters (default templates)
///    are not yet routed through this resolver — until Wave 2 rewires
///    them, an explicit per-page template lang stays authoritative
///    below page-specific sources but above the site-wide default.
///    Once the emitters call this resolver this step is a fixpoint.
/// 5. The site default `language` from `SsgConfig`.
/// 6. [`DEFAULT_PAGE_LANG`] (`"en"`) as the final constant.
///
/// Every returned value is normalised to BCP-47 hyphen form:
/// lowercase primary subtag, uppercase two-letter region
/// (`EN_gb` → `en-GB`).
#[must_use]
pub(crate) fn resolve_page_lang(
    html: &str,
    path: &Path,
    ctx: &PluginContext,
) -> String {
    let rel_path = site_relative(path, ctx);

    // 1 + 2. Front-matter `language`, then front-matter `hreflang`.
    if let Some(meta) = read_page_sidecar(path, ctx, &rel_path) {
        for key in ["language", "hreflang"] {
            if let Some(lang) = meta
                .get(key)
                .and_then(serde_json::Value::as_str)
                .and_then(normalize_bcp47)
            {
                return lang;
            }
        }
    }

    // 3. Locale path prefix of the site-relative path.
    if let Some(lang) = locale_from_path(&rel_path, ctx) {
        return lang;
    }

    // 4. `<html lang>` already present on the rendered page.
    if let Some(lang) = normalize_bcp47(&extract_html_lang(html)) {
        return lang;
    }

    // 5. Site default language from config.
    if let Some(lang) = ctx
        .config
        .as_ref()
        .and_then(|cfg| normalize_bcp47(&cfg.language))
    {
        return lang;
    }

    // 6. Final constant.
    DEFAULT_PAGE_LANG.to_string()
}

/// Returns the site-relative, forward-slash form of a built page path.
fn site_relative(path: &Path, ctx: &PluginContext) -> String {
    path.strip_prefix(&ctx.site_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Reads the frontmatter `.meta.json` sidecar for a built HTML file.
///
/// Shared by the language resolver (spec A5) and the ISO 20022
/// JSON-LD extension. Checks, in order:
///
/// 1. `<build_dir>/.meta/<rel_path>.meta.json` (HTML-keyed sidecar),
/// 2. `<build_dir>/.meta/<rel stem>.md.meta.json` (the `emit_sidecars`
///    convention for content compiled from `.md` sources),
/// 3. `<page>.meta.json` next to the HTML file itself (legacy
///    `frontmatter::read_sidecar` location).
///
/// Returns `None` when no sidecar exists or it does not parse —
/// callers fall through to their next source.
pub(crate) fn read_page_sidecar(
    path: &Path,
    ctx: &PluginContext,
    rel_path: &str,
) -> Option<serde_json::Value> {
    let sidecar_dir = ctx.build_dir.join(".meta");
    let candidate = sidecar_dir.join(rel_path).with_extension("meta.json");

    let raw = if candidate.exists() {
        fs::read_to_string(&candidate).ok()?
    } else {
        // Fallback: the `<stem>.md.meta.json` convention used by
        // `emit_sidecars` for content coming from `.md` source files.
        // `rel_path` here is `<...>/foo.html`; the original markdown
        // sidecar is at `<...>/foo.md.meta.json` keyed off the *input*
        // file extension. Strip `.html` and append `.md.meta.json`.
        let alt = sidecar_dir.join(rel_path.trim_end_matches(".html"));
        let alt = alt.with_extension("md.meta.json");
        if alt.exists() {
            fs::read_to_string(&alt).ok()?
        } else {
            // Last resort — look next to the HTML file itself (legacy
            // emission location used by `frontmatter::read_sidecar`).
            let inline = path.with_extension("meta.json");
            if inline.exists() {
                fs::read_to_string(&inline).ok()?
            } else {
                return None;
            }
        }
    };

    serde_json::from_str(&raw).ok()
}

/// Extracts a locale from the first directory component of the
/// site-relative path, if that component plausibly names a locale.
///
/// With an `[i18n]` config the prefix is validated strictly against
/// the declared locale set (`locales` ∪ `default_locale`); without
/// one, only a `xx` / `xx-XX` shaped prefix qualifies.
fn locale_from_path(rel_path: &str, ctx: &PluginContext) -> Option<String> {
    // The prefix must be a *directory* — a bare `index.html` at the
    // site root has no locale prefix.
    let (first, _) = rel_path.trim_start_matches('/').split_once('/')?;
    let candidate = normalize_bcp47(first)?;

    if let Some(declared) =
        ctx.config.as_ref().and_then(|cfg| cfg.i18n_locale_set())
    {
        // Strict: when locales are declared, only a declared locale
        // counts. An undeclared `de/` directory is not a locale page.
        return declared
            .iter()
            .filter_map(|loc| normalize_bcp47(loc))
            .find(|loc| *loc == candidate);
    }

    // Heuristic: no declared locale set reachable — accept only the
    // plausible `xx` / `xx-XX` shape so `blog/`, `api/`, `docs/` etc.
    // are never treated as locales.
    if is_plausible_locale_shape(first) {
        Some(candidate)
    } else {
        None
    }
}

/// Returns `true` when `s` looks like a locale directory name:
/// exactly two ASCII letters, optionally followed by `-`/`_` and a
/// two-letter region (`hi`, `en-GB`, `pt_BR`).
fn is_plausible_locale_shape(s: &str) -> bool {
    let normalized = s.replace('_', "-");
    let is_alpha2 = |part: &str| {
        part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic())
    };
    match normalized.split_once('-') {
        None => is_alpha2(&normalized),
        Some((primary, region)) => is_alpha2(primary) && is_alpha2(region),
    }
}

/// Normalises a raw language tag into canonical BCP-47 hyphen form.
///
/// Thin delegation to [`crate::core_group::lang::normalize_bcp47`] —
/// the render-time engine and the SEO sinks must normalise
/// identically or the A5 four-sink agreement breaks (see the
/// coordination note in `core::lang`'s module docs).
fn normalize_bcp47(raw: &str) -> Option<String> {
    crate::core_group::lang::normalize_bcp47(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::SsgConfig;
    #[cfg(feature = "i18n")]
    use crate::i18n::I18nConfig;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// Builds a `PluginContext` rooted in `dir` with an optional site
    /// language and optional declared locale set.
    fn ctx_with(
        dir: &Path,
        language: Option<&str>,
        locales: Option<&[&str]>,
    ) -> PluginContext {
        let site = dir.join("site");
        let build = dir.join("build");
        let mut ctx = PluginContext::new(
            Path::new("content"),
            &build,
            &site,
            Path::new("templates"),
        );
        if language.is_some() || locales.is_some() {
            #[cfg(not(feature = "i18n"))]
            let _ = locales;
            ctx.config = Some(SsgConfig {
                language: language.unwrap_or("").to_string(),
                #[cfg(feature = "i18n")]
                i18n: locales.map(|set| I18nConfig {
                    default_locale: set.first().map_or_else(
                        || "en".to_string(),
                        |loc| (*loc).to_string(),
                    ),
                    locales: set.iter().map(|loc| (*loc).to_string()).collect(),
                    url_prefix: Default::default(),
                }),
                ..SsgConfig::default()
            });
        }
        ctx
    }

    /// Writes a `.meta.json` sidecar for `rel` under `<build>/.meta`.
    fn write_sidecar(dir: &Path, rel: &str, json: &str) {
        let sidecar = dir
            .join("build")
            .join(".meta")
            .join(rel)
            .with_extension("meta.json");
        fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        fs::write(sidecar, json).unwrap();
    }

    fn page(dir: &Path, rel: &str) -> PathBuf {
        dir.join("site").join(rel)
    }

    // ── normalize_bcp47 ─────────────────────────────────────────

    #[test]
    fn normalize_canonicalises_case_and_separators() {
        assert_eq!(normalize_bcp47("EN_gb").as_deref(), Some("en-GB"));
        assert_eq!(normalize_bcp47("fr-fr").as_deref(), Some("fr-FR"));
        assert_eq!(normalize_bcp47("hi").as_deref(), Some("hi"));
        assert_eq!(normalize_bcp47("ZH-HANS").as_deref(), Some("zh-Hans"));
    }

    #[test]
    fn normalize_rejects_empty_and_garbage() {
        assert_eq!(normalize_bcp47(""), None);
        assert_eq!(normalize_bcp47("   "), None);
        assert_eq!(normalize_bcp47("english"), None);
        assert_eq!(normalize_bcp47("e"), None);
        assert_eq!(normalize_bcp47("en-"), None);
        assert_eq!(normalize_bcp47("12"), None);
    }

    // ── precedence: front matter ────────────────────────────────

    #[cfg(feature = "i18n")]
    #[test]
    fn frontmatter_language_wins_over_everything() {
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "hi/post/index.html",
            r#"{"language":"fr","hreflang":"de"}"#,
        );
        let ctx = ctx_with(dir.path(), Some("en-GB"), Some(&["en", "hi"]));
        let html = r#"<html lang="en-GB"><head></head></html>"#;
        let lang = resolve_page_lang(
            html,
            &page(dir.path(), "hi/post/index.html"),
            &ctx,
        );
        assert_eq!(lang, "fr");
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn frontmatter_hreflang_used_when_language_absent() {
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "hi/post/index.html",
            r#"{"hreflang":"en_gb"}"#,
        );
        let ctx = ctx_with(dir.path(), Some("en"), Some(&["en", "hi"]));
        let lang = resolve_page_lang(
            "<html><head></head></html>",
            &page(dir.path(), "hi/post/index.html"),
            &ctx,
        );
        assert_eq!(lang, "en-GB", "hreflang should be normalised to BCP-47");
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn invalid_frontmatter_language_falls_through_to_path() {
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "hi/post/index.html",
            r#"{"language":"not a lang"}"#,
        );
        let ctx = ctx_with(dir.path(), Some("en-GB"), Some(&["en", "hi"]));
        let lang = resolve_page_lang(
            "<html><head></head></html>",
            &page(dir.path(), "hi/post/index.html"),
            &ctx,
        );
        assert_eq!(lang, "hi");
    }

    // ── precedence: locale path prefix ──────────────────────────

    #[cfg(feature = "i18n")]
    #[test]
    fn declared_locale_path_prefix_beats_html_lang_and_default() {
        // The A5 signature bug: /hi/… pages carried the site-wide
        // <html lang="en-GB"> and emitted inLanguage=en-GB.
        let dir = tempdir().unwrap();
        let ctx = ctx_with(dir.path(), Some("en-GB"), Some(&["en", "hi"]));
        let html = r#"<html lang="en-GB"><head></head></html>"#;
        let lang = resolve_page_lang(
            html,
            &page(dir.path(), "hi/2026-06-01-post/index.html"),
            &ctx,
        );
        assert_eq!(lang, "hi");
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn undeclared_prefix_is_not_a_locale_when_i18n_configured() {
        // `de/` exists on disk but is not declared → not a locale page.
        let dir = tempdir().unwrap();
        let ctx = ctx_with(dir.path(), Some("en-GB"), Some(&["en", "hi"]));
        let lang = resolve_page_lang(
            "<html><head></head></html>",
            &page(dir.path(), "de/page/index.html"),
            &ctx,
        );
        assert_eq!(lang, "en-GB", "should fall through to site default");
    }

    #[test]
    fn shape_heuristic_accepts_xx_and_xx_region_without_i18n_config() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with(dir.path(), None, None);
        assert_eq!(
            resolve_page_lang(
                "",
                &page(dir.path(), "fr/about/index.html"),
                &ctx
            ),
            "fr"
        );
        assert_eq!(
            resolve_page_lang(
                "",
                &page(dir.path(), "pt-BR/about/index.html"),
                &ctx
            ),
            "pt-BR"
        );
    }

    #[test]
    fn shape_heuristic_rejects_ordinary_directories() {
        // `blog` (4 letters), `api` (3 letters) and root pages must
        // never be treated as locale prefixes.
        let dir = tempdir().unwrap();
        let ctx = ctx_with(dir.path(), None, None);
        for rel in ["blog/post/index.html", "api/index.html", "index.html"] {
            assert_eq!(
                resolve_page_lang("", &page(dir.path(), rel), &ctx),
                DEFAULT_PAGE_LANG,
                "{rel} must not resolve a locale from its path"
            );
        }
    }

    // ── precedence: html lang, config default, final constant ───

    #[test]
    fn html_lang_used_when_no_frontmatter_or_locale_prefix() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with(dir.path(), Some("en"), None);
        let html = r#"<html lang="fr-FR"><head></head></html>"#;
        let lang = resolve_page_lang(
            html,
            &page(dir.path(), "about/index.html"),
            &ctx,
        );
        assert_eq!(lang, "fr-FR");
    }

    #[test]
    fn config_default_used_when_page_has_no_lang_signal() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with(dir.path(), Some("en-GB"), None);
        let lang = resolve_page_lang(
            "<html><head></head></html>",
            &page(dir.path(), "about/index.html"),
            &ctx,
        );
        assert_eq!(lang, "en-GB");
    }

    #[test]
    fn en_fallback_fires_only_when_nothing_else_resolves() {
        let dir = tempdir().unwrap();
        // No sidecar, no locale prefix, no html lang, no config.
        let ctx = ctx_with(dir.path(), None, None);
        let lang = resolve_page_lang(
            "<html><head></head></html>",
            &page(dir.path(), "index.html"),
            &ctx,
        );
        assert_eq!(lang, DEFAULT_PAGE_LANG);

        // …and stops firing the moment any earlier source resolves.
        let ctx = ctx_with(dir.path(), Some("hi"), None);
        let lang = resolve_page_lang(
            "<html><head></head></html>",
            &page(dir.path(), "index.html"),
            &ctx,
        );
        assert_eq!(lang, "hi");
    }

    // ── sidecar lookup ──────────────────────────────────────────

    #[test]
    fn sidecar_md_convention_is_found() {
        let dir = tempdir().unwrap();
        // emit_sidecars writes `<stem>.md.meta.json` for .md sources.
        let sidecar = dir
            .path()
            .join("build")
            .join(".meta")
            .join("post.md.meta.json");
        fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        fs::write(sidecar, r#"{"language":"hi"}"#).unwrap();

        let ctx = ctx_with(dir.path(), Some("en"), None);
        let lang = resolve_page_lang("", &page(dir.path(), "post.html"), &ctx);
        assert_eq!(lang, "hi");
    }

    #[test]
    fn unparseable_sidecar_falls_through() {
        let dir = tempdir().unwrap();
        write_sidecar(dir.path(), "p/index.html", "not json at all");
        let ctx = ctx_with(dir.path(), Some("en-GB"), None);
        let lang =
            resolve_page_lang("", &page(dir.path(), "p/index.html"), &ctx);
        assert_eq!(lang, "en-GB");
    }

    #[test]
    fn unreadable_primary_sidecar_returns_none() {
        // The sidecar path exists but is a directory, so the
        // `fs::read_to_string(..).ok()?` branch bails with `None`.
        let dir = tempdir().unwrap();
        let sidecar = dir
            .path()
            .join("build")
            .join(".meta")
            .join("p")
            .join("index.meta.json");
        fs::create_dir_all(&sidecar).unwrap();

        let ctx = ctx_with(dir.path(), None, None);
        let got = read_page_sidecar(
            &page(dir.path(), "p/index.html"),
            &ctx,
            "p/index.html",
        );
        assert!(got.is_none());
    }

    #[test]
    fn unreadable_md_convention_sidecar_returns_none() {
        // `<stem>.md.meta.json` exists but is a directory → the second
        // lookup's `.ok()?` short-circuits to `None`.
        let dir = tempdir().unwrap();
        let alt = dir
            .path()
            .join("build")
            .join(".meta")
            .join("post.md.meta.json");
        fs::create_dir_all(&alt).unwrap();

        let ctx = ctx_with(dir.path(), None, None);
        let got = read_page_sidecar(
            &page(dir.path(), "post.html"),
            &ctx,
            "post.html",
        );
        assert!(got.is_none());
    }

    #[test]
    fn inline_legacy_sidecar_next_to_html_is_found() {
        // Last-resort lookup: `<page>.meta.json` next to the HTML file
        // itself (legacy `frontmatter::read_sidecar` location).
        let dir = tempdir().unwrap();
        let page_dir = dir.path().join("site").join("p");
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(page_dir.join("index.meta.json"), r#"{"language":"hi"}"#)
            .unwrap();

        let ctx = ctx_with(dir.path(), Some("en"), None);
        let lang =
            resolve_page_lang("", &page(dir.path(), "p/index.html"), &ctx);
        assert_eq!(lang, "hi");
    }

    #[test]
    fn unreadable_inline_sidecar_returns_none() {
        // Inline `<page>.meta.json` exists but is a directory → the
        // third lookup's `.ok()?` short-circuits to `None`.
        let dir = tempdir().unwrap();
        let inline = dir.path().join("site").join("p").join("index.meta.json");
        fs::create_dir_all(&inline).unwrap();

        let ctx = ctx_with(dir.path(), None, None);
        let got = read_page_sidecar(
            &page(dir.path(), "p/index.html"),
            &ctx,
            "p/index.html",
        );
        assert!(got.is_none());
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn empty_declared_locale_set_defaults_to_en() {
        // An `[i18n]` block with zero declared locales — the helper's
        // default-locale fallback kicks in and no path prefix can ever
        // match, so the site language decides.
        let dir = tempdir().unwrap();
        let ctx = ctx_with(dir.path(), None, Some(&[]));
        let i18n = ctx.config.as_ref().unwrap().i18n.as_ref().unwrap();
        assert_eq!(i18n.default_locale, "en");
        assert!(i18n.locales.is_empty());

        let lang = resolve_page_lang(
            "",
            &page(dir.path(), "fr/about/index.html"),
            &ctx,
        );
        assert_eq!(lang, DEFAULT_PAGE_LANG, "fr/ is undeclared → no locale");
    }
}
