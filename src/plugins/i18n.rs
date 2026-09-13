// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! # Internationalisation (i18n) routing primitives
//!
//! Provides hreflang link injection, per-locale sitemap generation,
//! and a language switcher HTML helper.
//!
//! ## Overview
//!
//! The `I18nPlugin` scans the site output directory for locale-prefixed
//! subdirectories (e.g. `/en/`, `/fr/`) and:
//!
//! 1. Injects `<link rel="alternate" hreflang="…">` tags into every HTML
//!    page that exists in multiple locales.
//! 2. Adds an `x-default` alternate pointing to the default locale.
//! 3. Generates per-locale sitemaps (`sitemap-en.xml`, `sitemap-fr.xml`, …)
//!    with `xhtml:link` alternates.
//!
//! The injection is **idempotent** — pages that already contain hreflang
//! links are skipped.
//!
//! ## Pairing pages across locales
//!
//! Before anything can be injected, the plugin has to decide which
//! pages in different locales are translations of one another. It
//! builds a matrix of `key -> {locale -> path}` and treats every page
//! sharing a key as one document.
//!
//! A page's key is its `translation_key` front-matter value when it
//! declares one, and its locale-relative path otherwise:
//!
//! ```yaml
//! ---
//! title: "À propos"
//! translation_key: "about"
//! ---
//! ```
//!
//! Path matching alone cannot pair `/about/` with `/fr/a-propos/` —
//! the paths differ, so each is a singleton and **neither receives any
//! hreflang at all**. Because that failure is silent, it is easy to
//! ship. A shared `translation_key` pairs them regardless of slug.
//!
//! Pages without a key keep pairing by path, so a site with no
//! `translation_key` anywhere produces exactly the matrix it produced
//! before the field existed.
//!
//! The value is read from the front-matter sidecars written by
//! [`crate::frontmatter::emit_sidecars`], because the plugin runs
//! after compilation and can no longer see the source front matter.
//!
//! ## Where the default locale lives
//!
//! The default locale may occupy the site root, with only the other
//! locales taking a URL segment (`/about/` alongside
//! `/fr/a-propos/`) — the default in Hugo, Astro and Next.js. This is
//! detected, not configured — the root locale is used when the default
//! locale has no output directory of its own and HTML exists outside
//! the other locale directories.
//!
//! ## Reciprocity
//!
//! Each alternate link is labelled with the resolved language of the
//! document it points *at*, not with the bare locale directory name.
//! Labelling by directory lets the two halves of a pair disagree — an
//! English page calling its Hindi alternate `hi` while the Hindi page
//! calls itself `hi-IN` — which fails Google's reciprocity requirement
//! and the `hreflang` audit gate with it. An authored locale code is
//! preserved byte-for-byte (`zh-tw` stays `zh-tw`); a resolved
//! language replaces it only on a genuine front-matter override.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close as inject_head;
// The locale/URL/hreflang logic now lives in `ssg-i18n`, a leaf crate
// with no knowledge of plugins or the filesystem (#588). Re-exported
// below so `crate::i18n::I18nConfig` and the rest keep working.
// Public before the move, so public after it: ssg's API is unchanged.
/// Re-exports of items that were crate-private before #588.
///
/// They have to be `pub` in `ssg-i18n` to cross the crate boundary, but
/// re-exporting them publicly here would widen ssg's API, which a
/// mechanical move must not do. These `compile_fail` doctests are the
/// assertion that it did not happen — if one of them ever compiles,
/// something private has escaped:
///
/// ```compile_fail
/// use ssg::i18n::build_url;
/// ```
///
/// ```compile_fail
/// use ssg::i18n::build_hreflang_links;
/// ```
///
/// ```compile_fail
/// use ssg::i18n::find_lang_switcher_element;
/// ```
///
/// ```compile_fail
/// use ssg::i18n::generate_lang_switcher_html_with_self_lang;
/// ```
///
/// ```compile_fail
/// use ssg::i18n::rewrite_ap_lang_items;
/// ```
///
/// ```compile_fail
/// use ssg::i18n::sidecar_candidates;
/// ```
///
/// ```compile_fail
/// use ssg::i18n::HREFLANG_MARKER;
/// ```
///
/// ```compile_fail
/// use ssg::i18n::LANG_SWITCHER_MARKER;
/// ```
pub(crate) use ssg_i18n::{
    build_hreflang_links, build_url, find_lang_switcher_element,
    generate_lang_switcher_html_with_self_lang, rewrite_ap_lang_items,
    sidecar_candidates, HREFLANG_MARKER, LANG_SWITCHER_MARKER,
};
pub use ssg_i18n::{
    generate_lang_switcher_html, negotiate_locale, parse_accept_language,
    I18nConfig, UrlPrefixStrategy,
};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

// ── Configuration ────────────────────────────────────────────────────

/// Cached locale matrix shared between `after_compile` and `transform_html`.
///
/// Built lazily on first invocation per `(site_dir, locales)` pairing so
/// that the per-file `transform_html` hook does not re-walk the locale
/// directories for every HTML file processed in the fused transform pass.
#[derive(Debug, Default)]
struct LocaleMatrixCache {
    site_dir: Option<PathBuf>,
    present_locales: Vec<String>,
    /// The locale served from the site root with no URL segment, if
    /// any — see [`detect_locales`].
    root_locale: Option<String>,
    /// Translation matrix: `key -> {locale -> rel_path}`.
    ///
    /// The key is the page's `translation_key` front-matter value when
    /// it declares one, and its locale-relative path otherwise (the
    /// pre-`translation_key` path-matching behaviour, kept so existing
    /// sites are unaffected).
    pages: LocaleMatrix,
    /// Reverse index: `(locale, rel_path) -> key`.
    keys: HashMap<(String, String), String>,
}

/// Reverse index of [`LocaleMatrix`]: `(locale, rel_path) -> key`.
type LocaleKeyIndex = HashMap<(String, String), String>;

/// `key -> {locale -> rel_path}`.
///
/// `BTreeMap` for the inner map so hreflang links, sitemap alternates
/// and the language switcher all emit locales in one stable order
/// without a sort at every call site.
type LocaleMatrix = HashMap<String, BTreeMap<String, String>>;

/// I18n plugin that injects hreflang links and generates per-locale sitemaps.
///
/// Implements two complementary phases:
///
/// 1. **`transform_html`** — per-file hreflang `<link>` injection that runs
///    inside the fused transform pass, ensuring Taxonomy/Pagination output
///    is covered alongside template-engine pages.
/// 2. **`after_compile`** — per-locale sitemap generation and the root-level
///    locale-redirect index page (whole-site artefacts that cannot be
///    produced from a per-file hook).
#[derive(Debug)]
pub struct I18nPlugin {
    config: I18nConfig,
    /// Lazily-populated locale matrix shared by hooks.
    ///
    /// `RwLock` rather than `Mutex` (plan §4 3.4): after warm-up the
    /// matrix is read-mostly — parallel `transform_html` workers only
    /// take the shared read lock, so lookups no longer serialise. The
    /// write lock is taken only on first fill and on the deliberate
    /// `after_compile` invalidation.
    matrix: RwLock<LocaleMatrixCache>,
}

impl I18nPlugin {
    /// Creates a new `I18nPlugin` with the given i18n configuration.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::i18n::{I18nConfig, I18nPlugin};
    /// use ssg::plugin::Plugin;
    ///
    /// let cfg = I18nConfig::default();
    /// let p = I18nPlugin::new(cfg);
    /// assert_eq!(p.name(), "i18n");
    /// ```
    #[must_use]
    pub fn new(config: I18nConfig) -> Self {
        Self {
            config,
            matrix: RwLock::new(LocaleMatrixCache::default()),
        }
    }

    /// Ensures the locale matrix cache is populated for the given site
    /// directory. Cheap on subsequent calls — the directory walk only
    /// executes once per `site_dir`.
    fn ensure_matrix(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        let site_dir = ctx.site_dir.as_path();
        // Fast path: shared read lock. After warm-up every caller
        // (including parallel `transform_html` workers) takes only this
        // branch (plan §4 3.4).
        {
            let cache = self
                .matrix
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.site_dir.as_deref() == Some(site_dir) {
                return Ok(());
            }
        }

        let mut cache = self
            .matrix
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Double-check under the write lock — another thread may have
        // filled the cache while we waited for it.
        if cache.site_dir.as_deref() == Some(site_dir) {
            return Ok(());
        }
        let (present_locales, root_locale) = detect_locales(
            site_dir,
            &self.config.locales,
            &self.config.default_locale,
        );
        let (pages, keys) = if present_locales.len() >= 2 {
            collect_locale_pages(
                site_dir,
                &resolve_sidecar_dir(ctx),
                &present_locales,
                root_locale.as_deref(),
            )
            .map_err(|e| SsgError::io(e, site_dir))?
        } else {
            (HashMap::new(), HashMap::new())
        };
        cache.site_dir = Some(site_dir.to_path_buf());
        cache.present_locales = present_locales;
        cache.root_locale = root_locale;
        cache.pages = pages;
        cache.keys = keys;
        Ok(())
    }
}

impl Plugin for I18nPlugin {
    fn name(&self) -> &'static str {
        "i18n"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        // Only operate when more than one locale is configured.
        if self.config.locales.len() < 2 {
            return Ok(());
        }

        // Re-walk the locale matrix here so that pages emitted by
        // Taxonomy/Pagination during their own `after_compile` hooks are
        // picked up. Always rebuild on `after_compile` to guarantee the
        // matrix reflects the final on-disk state.
        {
            let mut cache = self
                .matrix
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.site_dir = None;
        }
        self.ensure_matrix(ctx)?;

        let (present_locales, root_locale, pages) = {
            let cache = self
                .matrix
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                cache.present_locales.clone(),
                cache.root_locale.clone(),
                cache.pages.clone(),
            )
        };

        if present_locales.len() < 2 {
            return Ok(());
        }

        // Determine the base URL (needed for sitemaps).
        let base_url = ctx.config.as_ref().map_or_else(
            || "https://example.com".to_string(),
            |c| c.base_url.clone(),
        );

        // Inject hreflang into each HTML page.
        inject_hreflang_all(
            ctx,
            &pages,
            &present_locales,
            &self.config.default_locale,
            &base_url,
            &self.config.url_prefix,
            root_locale.as_deref(),
        )
        .map_err(|e| SsgError::io(e, &ctx.site_dir))?;

        // Generate per-locale sitemaps.
        generate_locale_sitemaps(
            ctx,
            &pages,
            &present_locales,
            &self.config.default_locale,
            &base_url,
            &self.config.url_prefix,
            root_locale.as_deref(),
        )
        .map_err(|e| SsgError::io(e, &ctx.site_dir))?;

        // Generate locale redirect index.html at site root — but only
        // when every locale lives in its own directory. With a
        // root-hosted default locale the site root already IS that
        // locale's home page; a redirect there would either be skipped
        // (the page carries no marker) or, on a rebuild over a previous
        // redirect, shadow a real page.
        if root_locale.is_none() {
            crate::server::generate_locale_redirect(
                &ctx.site_dir,
                &present_locales,
                &self.config.default_locale,
            )
            .map_err(|e| SsgError::io(e, &ctx.site_dir))?;
        }

        Ok(())
    }

    fn has_transform(&self) -> bool {
        true
    }

    /// Per-file hreflang injection.
    ///
    /// Runs inside the fused transform pass so it covers HTML produced by
    /// any plugin (Taxonomy, Pagination, template engine). Idempotent — a
    /// page that already carries hreflang `<link>` tags is returned
    /// unchanged. Pages without parallel translations are left untouched
    /// so that missing translations never yield broken hreflang entries.
    fn transform_html(
        &self,
        html: &str,
        path: &Path,
        ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        if self.config.locales.len() < 2 {
            return Ok(html.to_string());
        }
        if html.contains(HREFLANG_MARKER) {
            return Ok(html.to_string());
        }
        self.ensure_matrix(ctx)?;

        // Snapshot the cached matrix data we need.
        let (locale_for_file, locale_map, root_locale) = {
            let cache = self
                .matrix
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.present_locales.len() < 2 {
                return Ok(html.to_string());
            }
            let Some((locale, rel)) = resolve_locale_and_rel(
                path,
                &ctx.site_dir,
                &cache.present_locales,
                cache.root_locale.as_deref(),
            ) else {
                return Ok(html.to_string());
            };
            let Some(key) = cache.keys.get(&(locale.clone(), rel.clone()))
            else {
                return Ok(html.to_string());
            };
            let Some(locale_map) = cache.pages.get(key).cloned() else {
                return Ok(html.to_string());
            };
            (locale, locale_map, cache.root_locale.clone())
        };

        // Skip pages that only exist in one locale — AC4 (no broken
        // hreflangs).
        if locale_map.len() < 2 {
            return Ok(html.to_string());
        }

        let base_url = ctx.config.as_ref().map_or_else(
            || "https://example.com".to_string(),
            |c| c.base_url.clone(),
        );
        let base = base_url.trim_end_matches('/');

        // Resolve this page's language once (spec A5, plan §2 1.5) so
        // the hreflang self-reference and the switcher's self entry
        // agree with `<html lang>` / `inLanguage` / `og:locale`.
        let self_lang = crate::seo::lang::resolve_page_lang(html, path, ctx);

        let labels = hreflang_labels(
            ctx,
            &locale_map,
            &locale_for_file,
            &self_lang,
            root_locale.as_deref(),
        );

        let links = build_hreflang_links(
            &locale_map,
            &labels,
            &self.config.default_locale,
            base,
            &self.config.url_prefix,
            root_locale.as_deref(),
        );

        let Some(mut out) = inject_before_head_close(html, &links) else {
            return Ok(html.to_string());
        };

        // Lang switcher + ap-lang-item rewrite (kept consistent with the
        // `after_compile` injection path).
        out = inject_lang_switcher(
            &out,
            &locale_for_file,
            &locale_map,
            &labels,
            base,
            &self.config.url_prefix,
            root_locale.as_deref(),
        );
        out = rewrite_ap_lang_items(
            &out,
            &locale_map,
            base,
            &self.config.url_prefix,
            root_locale.as_deref(),
        );

        Ok(out)
    }
}

/// Splits an absolute HTML path inside `site_dir` into (locale, rel-path).
///
/// A path whose first component names a locale *directory* belongs to
/// that locale, and `rel-path` is what follows it. Any other path
/// belongs to `root_locale` when one is configured — that locale has no
/// directory of its own and is served straight from the site root, so
/// its `rel-path` is the whole site-relative path.
fn resolve_locale_and_rel(
    path: &Path,
    site_dir: &Path,
    locales: &[String],
    root_locale: Option<&str>,
) -> Option<(String, String)> {
    let rel = path.strip_prefix(site_dir).ok()?;
    let mut comps = rel.components();
    let first = comps.next()?.as_os_str().to_string_lossy().into_owned();

    // The root-hosted locale owns no directory, so its name must never
    // be treated as a locale-directory segment.
    if locales.contains(&first) && root_locale != Some(first.as_str()) {
        let remaining: PathBuf = comps.as_path().to_path_buf();
        if remaining.as_os_str().is_empty() {
            return None;
        }
        let rel_str = remaining.to_string_lossy().replace('\\', "/");
        return Some((first, rel_str));
    }

    let root = root_locale?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if rel_str.is_empty() {
        return None;
    }
    Some((root.to_string(), rel_str))
}

// ── Locale detection ─────────────────────────────────────────────────

/// Returns `(present_locales, root_locale)`.
///
/// A configured locale is present when `site_dir/<locale>` exists.
/// Additionally, the **default** locale is present without a directory
/// of its own when the site root holds HTML outside the other locale
/// directories — the root-hosted-default-locale convention shared by
/// Hugo (`defaultContentLanguageInSubdir = false`), Astro
/// (`prefixDefaultLocale: false`) and Next.js. In that case it is also
/// returned as `root_locale`, and everything downstream renders its
/// URLs without a locale segment.
///
/// The returned list preserves the configured order.
fn detect_locales(
    site_dir: &Path,
    locales: &[String],
    default_locale: &str,
) -> (Vec<String>, Option<String>) {
    let dir_locales: Vec<String> = locales
        .iter()
        .filter(|l| site_dir.join(l).is_dir())
        .cloned()
        .collect();

    let root_locale = if locales.iter().any(|l| l == default_locale)
        && !dir_locales.iter().any(|l| l == default_locale)
        && root_holds_html_outside(site_dir, &dir_locales)
    {
        Some(default_locale.to_string())
    } else {
        None
    };

    let present = locales
        .iter()
        .filter(|l| {
            dir_locales.contains(l)
                || root_locale.as_deref() == Some(l.as_str())
        })
        .cloned()
        .collect();

    (present, root_locale)
}

/// `true` when at least one HTML file sits at the site root or in a
/// directory that is not one of `locale_dirs`.
///
/// Walk errors are treated as "no HTML" here: detection must not fail a
/// build on its own. The same directories are re-walked by
/// [`collect_locale_pages`], which does propagate I/O errors.
fn root_holds_html_outside(site_dir: &Path, locale_dirs: &[String]) -> bool {
    let mut found = Vec::new();
    collect_root_html_files(site_dir, site_dir, locale_dirs, "", &mut found)
        .is_ok()
        && !found.is_empty()
}

// ── Page collection ──────────────────────────────────────────────────

/// One built page, as seen by the locale walker.
#[derive(Debug)]
struct LocalePage {
    /// Locale serving the page.
    locale: String,
    /// Path relative to the locale's URL root — `about/index.html`
    /// both for `<site>/en/about/index.html` and, when `en` is the
    /// root-hosted locale, for `<site>/about/index.html`.
    rel: String,
    /// Path relative to the site root, used to find the page's
    /// front-matter sidecar.
    site_rel: String,
}

/// Builds the translation matrix and its reverse index.
///
/// Returns `(key -> {locale -> rel_path}, (locale, rel_path) -> key)`.
///
/// The key of a page is its `translation_key` front-matter value when
/// it declares one, and its locale-relative path otherwise. Path
/// matching therefore survives untouched as the fallback: a site with
/// no `translation_key` anywhere produces exactly the matrix the
/// pre-`translation_key` implementation produced.
///
/// `root_locale`, when set, names the locale whose pages live at the
/// site root rather than in a directory of their own.
fn collect_locale_pages(
    site_dir: &Path,
    sidecar_dir: &Path,
    locales: &[String],
    root_locale: Option<&str>,
) -> Result<(LocaleMatrix, LocaleKeyIndex), SsgError> {
    let locale_dirs: Vec<String> = locales
        .iter()
        .filter(|l| root_locale != Some(l.as_str()))
        .cloned()
        .collect();

    let mut found: Vec<LocalePage> = Vec::new();
    for locale in locales {
        if root_locale == Some(locale.as_str()) {
            collect_root_html_files(
                site_dir,
                site_dir,
                &locale_dirs,
                locale,
                &mut found,
            )?;
            continue;
        }
        let locale_dir = site_dir.join(locale);
        if !locale_dir.is_dir() {
            continue;
        }
        collect_html_files_recursive(
            &locale_dir,
            &locale_dir,
            locale,
            locale,
            &mut found,
        )?;
    }

    let mut matrix: LocaleMatrix = HashMap::new();
    let mut keys: LocaleKeyIndex = HashMap::new();
    for page in found {
        let key = translation_key_for(sidecar_dir, &page.site_rel)
            .unwrap_or_else(|| page.rel.clone());
        let _ = matrix
            .entry(key.clone())
            .or_default()
            .insert(page.locale.clone(), page.rel.clone());
        let _ = keys.insert((page.locale, page.rel), key);
    }

    Ok((matrix, keys))
}

/// Reads the `translation_key` front-matter value for the built page at
/// site-relative `site_rel`, if it declares one.
///
/// `emit_sidecars` keys sidecars by the *content* path
/// (`content/fr/a-propos.md` → `.meta/fr/a-propos.meta.json`), while
/// the compiler publishes that page at `fr/a-propos/index.html`. Both
/// spellings are tried, plus the `<page>.html → <page>.meta.json` form
/// for pages that are not directory-indexed.
fn translation_key_for(sidecar_dir: &Path, site_rel: &str) -> Option<String> {
    for candidate in sidecar_candidates(site_rel) {
        let path = sidecar_dir.join(candidate);
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if let Some(key) = value
            .get("translation_key")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|k| !k.is_empty())
        {
            return Some(key.to_string());
        }
    }
    None
}

/// Locates the front-matter sidecar directory, mirroring
/// `template_plugin::resolve_sidecar_dir`: `<build>/.meta` while the
/// build directory still exists, `<site>/.meta` after it has been
/// promoted to the site directory.
fn resolve_sidecar_dir(ctx: &PluginContext) -> PathBuf {
    let build_meta = ctx.build_dir.join(".meta");
    if build_meta.is_dir() {
        build_meta
    } else {
        ctx.site_dir.join(".meta")
    }
}

/// Walks the site root for the root-hosted locale's pages, skipping the
/// other locales' directories and every dot-directory (`.meta`,
/// `.ssg-cache`, …) — those hold build metadata, not pages.
fn collect_root_html_files(
    root: &Path,
    current: &Path,
    locale_dirs: &[String],
    locale: &str,
    out: &mut Vec<LocalePage>,
) -> Result<(), SsgError> {
    let entries = fs::read_dir(current).with_path(current)?;

    for entry in entries {
        let entry = entry.with_path(current)?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name.starts_with('.') {
                continue;
            }
            // Only top-level directories can be locale directories.
            if current == root && locale_dirs.contains(&name) {
                continue;
            }
            collect_root_html_files(root, &path, locale_dirs, locale, out)?;
        } else if path.extension().is_some_and(|e| e == "html") {
            let rel = rel_of(&path, root);
            out.push(LocalePage {
                locale: locale.to_string(),
                site_rel: rel.clone(),
                rel,
            });
        }
    }

    Ok(())
}

/// Recursively walk `current` under `root`, recording relative HTML paths.
fn collect_html_files_recursive(
    root: &Path,
    current: &Path,
    locale: &str,
    locale_dir_name: &str,
    out: &mut Vec<LocalePage>,
) -> Result<(), SsgError> {
    let entries = fs::read_dir(current).with_path(current)?;

    for entry in entries {
        let entry = entry.with_path(current)?;
        let path = entry.path();
        if path.is_dir() {
            collect_html_files_recursive(
                root,
                &path,
                locale,
                locale_dir_name,
                out,
            )?;
        } else if path.extension().is_some_and(|e| e == "html") {
            let rel = rel_of(&path, root);
            out.push(LocalePage {
                locale: locale.to_string(),
                site_rel: format!("{locale_dir_name}/{rel}"),
                rel,
            });
        }
    }

    Ok(())
}

/// Slash-normalised path of `path` relative to `root`.
fn rel_of(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Resolves the on-disk path of a page: `site_dir/<locale>/<rel>` for a
/// directory-hosted locale, `site_dir/<rel>` for the root-hosted one.
fn page_file_path(
    site_dir: &Path,
    locale: &str,
    rel_path: &str,
    root_locale: Option<&str>,
) -> PathBuf {
    if root_locale == Some(locale) {
        site_dir.join(rel_path)
    } else {
        site_dir.join(locale).join(rel_path)
    }
}

// ── Hreflang injection ───────────────────────────────────────────────

/// Inject hreflang `<link>` tags into every HTML page that exists in at
/// least two locales.
///
/// Each page's SELF-reference `hreflang` (and the language-switcher
/// self entry) carries the language resolved by
/// `seo::lang::resolve_page_lang` for that page — the same value the
/// `<html lang>`, JSON-LD `inLanguage`, and `og:locale` sinks publish
/// (spec A5 acceptance: four sinks, one value).
fn inject_hreflang_all(
    ctx: &PluginContext,
    pages: &LocaleMatrix,
    locales: &[String],
    default_locale: &str,
    base_url: &str,
    strategy: &UrlPrefixStrategy,
    root_locale: Option<&str>,
) -> Result<(), SsgError> {
    let site_dir = ctx.site_dir.as_path();
    let base = base_url.trim_end_matches('/');
    let mut count = 0usize;

    for locale_map in pages.values() {
        // Only inject when the page exists in more than one locale.
        if locale_map.len() < 2 {
            continue;
        }

        for locale in locales {
            let Some(rel_path) = locale_map.get(locale) else {
                continue;
            };

            let file = page_file_path(site_dir, locale, rel_path, root_locale);
            if !file.exists() {
                continue;
            }

            let html = fs::read_to_string(&file).with_path(&file)?;

            // Idempotency: skip if already injected.
            if html.contains(HREFLANG_MARKER) {
                continue;
            }

            // Resolve this page's language once so every self-labelled
            // emission below agrees with the other language sinks.
            let self_lang =
                crate::seo::lang::resolve_page_lang(&html, &file, ctx);

            let labels = hreflang_labels(
                ctx,
                locale_map,
                locale,
                &self_lang,
                root_locale,
            );

            let links = build_hreflang_links(
                locale_map,
                &labels,
                default_locale,
                base,
                strategy,
                root_locale,
            );

            let html = if let Some(injected) =
                inject_before_head_close(&html, &links)
            {
                injected
            } else {
                html
            };

            // Also inject visible language switcher at the marker
            let html = inject_lang_switcher(
                &html,
                locale,
                locale_map,
                &labels,
                base,
                strategy,
                root_locale,
            );

            // Rewrite existing ap-lang-item links to the exact localized page path
            let html = rewrite_ap_lang_items(
                &html,
                locale_map,
                base,
                strategy,
                root_locale,
            );

            fs::write(&file, html).with_path(&file)?;
            count += 1;
        }
    }

    if count > 0 {
        println!(
            "[i18n] Injected hreflang + lang switcher into {count} HTML pages"
        );
    }

    Ok(())
}

/// Resolves the `hreflang` label to advertise for every locale serving
/// a page.
///
/// The SELF entry uses `self_lang` — the value
/// `seo::lang::resolve_page_lang` gave the page being written, i.e. the
/// same value its `<html lang>`, JSON-LD `inLanguage` and `og:locale`
/// publish (spec A5). Every OTHER entry describes a *different*
/// document, so it carries **that** page's resolved language rather
/// than its bare locale directory name. Without this the two sides of a
/// pair can disagree (`/` labelling itself `en-GB` while `/fr/` labels
/// it `en`), and Google's reciprocity requirement — enforced by the
/// `hreflang` audit gate — is not met.
///
/// Issue #522 AC5: an authored locale code is preserved byte-for-byte
/// (`zh-tw` stays `zh-tw`); the resolved language only replaces it when
/// it differs beyond case, i.e. on a genuine front-matter override.
fn hreflang_labels(
    ctx: &PluginContext,
    locale_map: &BTreeMap<String, String>,
    self_locale: &str,
    self_lang: &str,
    root_locale: Option<&str>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (locale, rel) in locale_map {
        let resolved = if locale == self_locale {
            self_lang.to_string()
        } else {
            resolved_page_lang_for(ctx, locale, rel, root_locale)
        };
        let label = if resolved.eq_ignore_ascii_case(locale) {
            locale.clone()
        } else {
            resolved
        };
        let _ = out.insert(locale.clone(), label);
    }
    out
}

/// Replaces the `<!-- ssg:lang-switcher -->` marker with a full language
/// switcher listing every available locale. Called by the i18n plugin
/// only when multiple locales are present on disk.
///
/// `self_lang` is the current page's resolved language
/// (`seo::lang::resolve_page_lang`); the switcher's self entry
/// advertises it in `lang=`/`hreflang=` so the switcher agrees with
/// the page's other language sinks (spec A5).
fn inject_lang_switcher(
    html: &str,
    current_locale: &str,
    locale_map: &BTreeMap<String, String>,
    labels: &BTreeMap<String, String>,
    base_url: &str,
    strategy: &UrlPrefixStrategy,
    root_locale: Option<&str>,
) -> String {
    let has_comment = html.contains(LANG_SWITCHER_MARKER);
    let element = find_lang_switcher_element(html);
    if !has_comment && element.is_none() {
        return html.to_string();
    }
    let switcher = generate_lang_switcher_html_with_self_lang(
        locale_map,
        labels,
        current_locale,
        base_url,
        strategy,
        root_locale,
    );
    let out = if let Some((start, end)) = element {
        let mut s = String::with_capacity(html.len() + switcher.len());
        s.push_str(&html[..start]);
        s.push_str(&switcher);
        s.push_str(&html[end..]);
        s
    } else {
        html.to_string()
    };
    out.replace(LANG_SWITCHER_MARKER, &switcher)
}

/// Insert `links` just before the first `</head>` tag, if present.
///
/// Thin shim over the shared [`inject_head`] helper that returns `None`
/// when the document has no `<head>` — keeping the historical
/// `Option<String>` signature for the test suite that asserts on it.
fn inject_before_head_close(html: &str, links: &str) -> Option<String> {
    if !html.to_ascii_lowercase().contains("</head>") {
        return None;
    }
    let result = inject_head(html, links);
    if result == html {
        None
    } else {
        Some(result)
    }
}

// ── Per-locale sitemaps ──────────────────────────────────────────────

/// Generate `sitemap-{locale}.xml` for every present locale.
///
/// Inside `sitemap-{L}.xml`, each `<url>` names the `L`-locale copy of
/// a page, so the `xhtml:link` whose `hreflang` matches `L` is that
/// page's SELF-reference. That entry is routed through
/// [`resolved_page_lang_for`] (spec A5, plan §2 1.5) so it carries the
/// same value as `<html lang>`, JSON-LD `inLanguage`, `og:locale`, and
/// the in-page hreflang self-reference. Alternates for other locales
/// keep their per-target-locale labels.
fn generate_locale_sitemaps(
    ctx: &PluginContext,
    pages: &LocaleMatrix,
    locales: &[String],
    default_locale: &str,
    base_url: &str,
    strategy: &UrlPrefixStrategy,
    root_locale: Option<&str>,
) -> Result<(), SsgError> {
    let site_dir = ctx.site_dir.as_path();
    let base = base_url.trim_end_matches('/');

    for locale in locales {
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\"\n\
                     xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">\n",
        );

        // Sort by this locale's own path so the file is deterministic
        // even though translated slugs differ between locales.
        let mut entries: Vec<(&String, &BTreeMap<String, String>)> = pages
            .iter()
            .filter(|(_, locale_map)| locale_map.contains_key(locale))
            .collect();
        entries.sort_by(|a, b| a.1.get(locale).cmp(&b.1.get(locale)));

        for (_key, locale_map) in entries {
            // `filter` above guarantees this locale is present.
            let Some(rel_path) = locale_map.get(locale) else {
                continue;
            };
            let loc = build_url(base, locale, rel_path, strategy, root_locale);
            xml.push_str("  <url>\n");
            xml.push_str(&format!("    <loc>{loc}</loc>\n"));

            // The page this <url> entry describes — its resolved
            // language labels the self-referencing xhtml:link, and each
            // alternate carries the language of the page it names.
            let self_lang =
                resolved_page_lang_for(ctx, locale, rel_path, root_locale);
            let labels = hreflang_labels(
                ctx,
                locale_map,
                locale,
                &self_lang,
                root_locale,
            );

            for (alt_locale, alt_rel) in locale_map {
                let alt_href =
                    build_url(base, alt_locale, alt_rel, strategy, root_locale);
                let hreflang = labels.get(alt_locale).unwrap_or(alt_locale);
                xml.push_str(&format!(
                    "    <xhtml:link rel=\"alternate\" hreflang=\"{hreflang}\" href=\"{alt_href}\" />\n"
                ));
            }
            // x-default, when the default locale actually serves it.
            if let Some(default_rel) = locale_map.get(default_locale) {
                let default_href = build_url(
                    base,
                    default_locale,
                    default_rel,
                    strategy,
                    root_locale,
                );
                xml.push_str(&format!(
                    "    <xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"{default_href}\" />\n"
                ));
            }

            xml.push_str("  </url>\n");
        }

        xml.push_str("</urlset>\n");

        let sitemap_path = site_dir.join(format!("sitemap-{locale}.xml"));
        fs::write(&sitemap_path, &xml).with_path(&sitemap_path)?;
    }

    println!("[i18n] Generated {} locale sitemaps", locales.len());
    Ok(())
}

/// Resolves the language of the built page at
/// `<site_dir>/<locale>/<rel_path>` via
/// `seo::lang::resolve_page_lang`, falling back to the locale
/// directory name when the file cannot be read (deleted mid-build,
/// permissions) — the pre-A5 label, so output never regresses below
/// the historic behaviour.
fn resolved_page_lang_for(
    ctx: &PluginContext,
    locale: &str,
    rel_path: &str,
    root_locale: Option<&str>,
) -> String {
    let file = page_file_path(&ctx.site_dir, locale, rel_path, root_locale);
    fs::read_to_string(&file).map_or_else(
        |_| locale.to_string(),
        |html| crate::seo::lang::resolve_page_lang(&html, &file, ctx),
    )
}

// ── Accept-Language parsing ─────────────────────────────────────────

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginContext;
    use std::path::Path;
    use tempfile::tempdir;

    /// Builds a translation-matrix row where every locale serves the
    /// same `rel` path — the shape a site without translated slugs has.
    fn locale_map(locales: &[&str], rel: &str) -> BTreeMap<String, String> {
        locales
            .iter()
            .map(|l| ((*l).to_string(), rel.to_string()))
            .collect()
    }

    #[test]
    fn test_rewrite_ap_lang_items() {
        let input = r#"<a class="ap-lang-item" href="/fr/" data-lang="fr" role="menuitem">Français</a>"#;
        let locales = locale_map(&["en", "fr"], "posts/hello.html");
        let output = rewrite_ap_lang_items(
            input,
            &locales,
            "https://sebastienrousseau.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert!(output.contains(r#"href="/fr/posts/hello.html""#));
    }

    fn make_ctx(site_dir: &Path) -> PluginContext {
        let config = crate::cmd::SsgConfig::builder()
            .site_name("test".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("test config");
        PluginContext::with_config(
            Path::new("content"),
            Path::new("build"),
            site_dir,
            Path::new("templates"),
            config,
        )
    }

    /// Like [`make_ctx`] but with a real build dir so
    /// `seo::lang::resolve_page_lang` can find `.meta` sidecars.
    fn make_ctx_with_build(site_dir: &Path, build_dir: &Path) -> PluginContext {
        let config = crate::cmd::SsgConfig::builder()
            .site_name("test".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("test config");
        PluginContext::with_config(
            Path::new("content"),
            build_dir,
            site_dir,
            Path::new("templates"),
            config,
        )
    }

    /// Writes a front-matter sidecar under `<build>/.meta/<rel>.meta.json`.
    fn write_lang_sidecar(build_dir: &Path, rel_html: &str, json: &str) {
        let sidecar = build_dir
            .join(".meta")
            .join(rel_html)
            .with_extension("meta.json");
        fs::create_dir_all(sidecar.parent().expect("parent")).expect("mkdir");
        fs::write(sidecar, json).expect("write sidecar");
    }

    /// Helper: create an HTML file with a `</head>` tag.
    fn write_html(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        // dir.join(rel) always has a parent; expect avoids an
        // uncoverable `if let` fallthrough region.
        let parent = path.parent().expect("joined path has a parent");
        fs::create_dir_all(parent).expect("mkdir");
        let html = format!(
            "<!DOCTYPE html><html><head><title>Test</title></head><body>{body}</body></html>"
        );
        fs::write(&path, html).expect("write html");
    }

    // ── detect_locale_dirs ───────────────────────────────────────

    #[test]
    fn detect_finds_existing_locale_dirs() {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("en")).unwrap();
        fs::create_dir(tmp.path().join("fr")).unwrap();

        let (found, root) = detect_locales(
            tmp.path(),
            &["en".into(), "fr".into(), "de".into()],
            "en",
        );
        assert_eq!(found, vec!["en", "fr"]);
        assert_eq!(root, None, "en has its own directory — not root-hosted");
    }

    #[test]
    fn detect_returns_empty_when_none_exist() {
        let tmp = tempdir().unwrap();
        let (found, root) =
            detect_locales(tmp.path(), &["en".into(), "fr".into()], "en");
        assert!(found.is_empty());
        assert_eq!(root, None, "an empty site root hosts no default locale");
    }

    // ── hreflang injection ───────────────────────────────────────

    #[test]
    fn injects_hreflang_into_shared_pages() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/index.html", "Hello");
        write_html(site, "fr/index.html", "Bonjour");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        let plugin = I18nPlugin::new(config);
        plugin.after_compile(&ctx).unwrap();

        // Both files should contain hreflang links.
        let en = fs::read_to_string(site.join("en/index.html")).unwrap();
        let fr = fs::read_to_string(site.join("fr/index.html")).unwrap();

        assert!(en.contains(HREFLANG_MARKER), "en missing hreflang");
        assert!(fr.contains(HREFLANG_MARKER), "fr missing hreflang");

        // Check x-default points to en.
        assert!(
            en.contains("hreflang=\"x-default\""),
            "en missing x-default"
        );
        assert!(
            en.contains("https://example.com/en/index.html"),
            "en x-default wrong href"
        );
    }

    #[test]
    fn skips_pages_existing_in_only_one_locale() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/index.html", "Hello");
        write_html(site, "en/about.html", "About");
        // fr only has index
        write_html(site, "fr/index.html", "Bonjour");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        // about.html only exists in en — should NOT have hreflang.
        let about = fs::read_to_string(site.join("en/about.html")).unwrap();
        assert!(
            !about.contains(HREFLANG_MARKER),
            "about.html should not have hreflang"
        );
    }

    #[test]
    fn idempotent_injection() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/index.html", "Hello");
        write_html(site, "fr/index.html", "Bonjour");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        let plugin = I18nPlugin::new(config);

        // Run twice.
        plugin.after_compile(&ctx).unwrap();
        plugin.after_compile(&ctx).unwrap();

        let en = fs::read_to_string(site.join("en/index.html")).unwrap();
        let count = en.matches(HREFLANG_MARKER).count();
        // en + fr + x-default = 3 links, and only one run should inject.
        assert_eq!(count, 3, "expected 3 hreflang links, got {count}");
    }

    // ── x-default ────────────────────────────────────────────────

    #[test]
    fn x_default_points_to_default_locale() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/page.html", "EN");
        write_html(site, "fr/page.html", "FR");
        write_html(site, "de/page.html", "DE");

        let config = I18nConfig {
            default_locale: "fr".into(),
            locales: vec!["en".into(), "fr".into(), "de".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en = fs::read_to_string(site.join("en/page.html")).unwrap();
        // x-default should point to fr (the configured default).
        assert!(
            en.contains("hreflang=\"x-default\" href=\"https://example.com/fr/page.html\""),
            "x-default should point to fr"
        );
    }

    // ── multi-locale detection ───────────────────────────────────

    #[test]
    fn three_locale_injection() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/index.html", "EN");
        write_html(site, "fr/index.html", "FR");
        write_html(site, "de/index.html", "DE");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into(), "de".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en = fs::read_to_string(site.join("en/index.html")).unwrap();
        // Should have de, en, fr + x-default = 4 links.
        let count = en.matches(HREFLANG_MARKER).count();
        assert_eq!(
            count, 4,
            "expected 4 hreflang links for 3 locales + x-default"
        );
    }

    // ── sitemap generation ───────────────────────────────────────

    #[test]
    fn generates_per_locale_sitemaps() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/index.html", "EN");
        write_html(site, "fr/index.html", "FR");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en_sm = site.join("sitemap-en.xml");
        let fr_sm = site.join("sitemap-fr.xml");
        assert!(en_sm.exists(), "sitemap-en.xml should exist");
        assert!(fr_sm.exists(), "sitemap-fr.xml should exist");

        let en_content = fs::read_to_string(&en_sm).unwrap();
        assert!(
            en_content.contains("<loc>https://example.com/en/index.html</loc>")
        );
        assert!(en_content.contains("xhtml:link"));
        assert!(en_content.contains("hreflang=\"x-default\""));
    }

    // ── SubDomain strategy ───────────────────────────────────────

    #[test]
    fn subdomain_strategy_builds_correct_urls() {
        let url = build_url(
            "https://example.com",
            "fr",
            "about/index.html",
            &UrlPrefixStrategy::SubDomain,
            None,
        );
        assert_eq!(url, "https://fr.example.com/about/index.html");
    }

    #[test]
    fn subpath_strategy_builds_correct_urls() {
        let url = build_url(
            "https://example.com",
            "fr",
            "about/index.html",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(url, "https://example.com/fr/about/index.html");
    }

    // ── Language switcher ────────────────────────────────────────

    #[test]
    fn lang_switcher_html() {
        let html = generate_lang_switcher_html(
            &["en".into(), "fr".into()],
            "en",
            "about/index.html",
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
        );
        assert!(html.contains("lang=\"en\""));
        assert!(html.contains("lang=\"fr\""));
        assert!(html.contains("aria-current=\"page\""));
        assert!(html.contains("class=\"lang-switcher\""));
    }

    // ── inject_before_head_close ─────────────────────────────────

    #[test]
    fn inject_before_head_close_works() {
        let html = "<html><head><title>T</title></head><body></body></html>";
        let result = inject_before_head_close(html, "INJECTED\n").unwrap();
        assert!(result.contains("INJECTED\n</head>"));
    }

    #[test]
    fn inject_before_head_close_returns_none_without_head() {
        let html = "<html><body>no head</body></html>";
        assert!(inject_before_head_close(html, "X").is_none());
    }

    // ── Plugin basics ────────────────────────────────────────────

    #[test]
    fn plugin_name() {
        let p = I18nPlugin::new(I18nConfig::default());
        assert_eq!(p.name(), "i18n");
    }

    #[test]
    fn plugin_skips_nonexistent_site_dir() {
        let ctx = PluginContext::new(
            Path::new("c"),
            Path::new("b"),
            Path::new("/does/not/exist"),
            Path::new("t"),
        );
        let p = I18nPlugin::new(I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        });
        assert!(p.after_compile(&ctx).is_ok());
    }

    #[test]
    fn plugin_skips_single_locale() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        let p = I18nPlugin::new(I18nConfig::default());
        // Default has only "en" — should be a no-op.
        assert!(p.after_compile(&ctx).is_ok());
    }

    // ── I18nConfig defaults ──────────────────────────────────────

    #[test]
    fn default_config() {
        let cfg = I18nConfig::default();
        assert_eq!(cfg.default_locale, "en");
        assert_eq!(cfg.locales, vec!["en"]);
        assert_eq!(cfg.url_prefix, UrlPrefixStrategy::SubPath);
    }

    // ── Nested page paths ────────────────────────────────────────

    // ── Language switcher edge cases ────────────────────────────────

    #[test]
    fn lang_switcher_empty_locales() {
        let html = generate_lang_switcher_html(
            &[],
            "en",
            "index.html",
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
        );
        assert!(html.contains("<nav"));
        assert!(html.contains("</nav>"));
        // No <li> items
        assert!(!html.contains("<li>"));
    }

    #[test]
    fn lang_switcher_single_locale() {
        let html = generate_lang_switcher_html(
            &["en".into()],
            "en",
            "index.html",
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
        );
        assert!(html.contains("aria-current=\"page\""));
        // Only one <li>
        assert_eq!(html.matches("<li>").count(), 1);
    }

    #[test]
    fn lang_switcher_subdomain_strategy() {
        let html = generate_lang_switcher_html(
            &["en".into(), "fr".into()],
            "fr",
            "about/index.html",
            "https://example.com",
            &UrlPrefixStrategy::SubDomain,
        );
        assert!(html.contains("https://en.example.com/about/index.html"));
        assert!(html.contains("https://fr.example.com/about/index.html"));
    }

    // ── Per-locale sitemap with xhtml:link alternates ────────────

    #[test]
    fn sitemap_contains_xhtml_link_alternates() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/index.html", "EN");
        write_html(site, "fr/index.html", "FR");
        write_html(site, "de/index.html", "DE");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into(), "de".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en_sm = fs::read_to_string(site.join("sitemap-en.xml")).unwrap();
        // Should contain xhtml:link alternates for all 3 locales + x-default
        assert!(en_sm.contains("hreflang=\"en\""));
        assert!(en_sm.contains("hreflang=\"fr\""));
        assert!(en_sm.contains("hreflang=\"de\""));
        assert!(en_sm.contains("hreflang=\"x-default\""));
        // x-default should point to en (default locale)
        assert!(en_sm.contains(
            "hreflang=\"x-default\" href=\"https://example.com/en/index.html\""
        ));
    }

    // ── Resolved self-reference language (spec A5, plan §2 1.5) ──

    /// Full fixture: `/en/about.html` + `/hi/about.html` where the hi
    /// page's front-matter sidecar declares `language: hi-IN`.
    fn a5_fixture(tmp: &Path) -> PluginContext {
        let site = tmp.join("site");
        let build = tmp.join("build");
        fs::create_dir_all(&site).expect("mkdir site");
        write_html(&site, "en/about.html", "EN About");
        write_html(&site, "hi/about.html", "HI About");
        write_lang_sidecar(&build, "hi/about.html", r#"{"language":"hi-IN"}"#);
        make_ctx_with_build(&site, &build)
    }

    #[test]
    fn hreflang_self_reference_uses_resolved_page_language() {
        let tmp = tempdir().unwrap();
        let ctx = a5_fixture(tmp.path());
        let site = ctx.site_dir.clone();

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "hi".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let hi = fs::read_to_string(site.join("hi/about.html")).unwrap();
        // Self-reference carries the resolver's value, not the
        // directory name — one value across all four sinks.
        assert!(
            hi.contains(
                "hreflang=\"hi-IN\" href=\"https://example.com/hi/about.html\""
            ),
            "hi self-reference should be resolved to hi-IN: {hi}"
        );
        // Alternate to the OTHER locale keeps its per-target label.
        assert!(
            hi.contains(
                "hreflang=\"en\" href=\"https://example.com/en/about.html\""
            ),
            "alternate to en must keep its label: {hi}"
        );

        let en = fs::read_to_string(site.join("en/about.html")).unwrap();
        // From the en page, the link *to* the hi page describes the hi
        // DOCUMENT, so it carries that document's resolved language —
        // the same `hi-IN` the hi page advertises for itself. The two
        // sides of the pair must agree on the tag or Google's
        // reciprocity requirement (and the `hreflang` audit gate) is
        // not met.
        assert!(
            en.contains(
                "hreflang=\"hi-IN\" href=\"https://example.com/hi/about.html\""
            ),
            "en page's alternate to hi must carry hi's resolved language: {en}"
        );
        assert!(
            en.contains(
                "hreflang=\"en\" href=\"https://example.com/en/about.html\""
            ),
            "en self-reference resolves to en: {en}"
        );
    }

    #[test]
    fn locale_sitemap_self_reference_uses_resolved_page_language() {
        let tmp = tempdir().unwrap();
        let ctx = a5_fixture(tmp.path());
        let site = ctx.site_dir.clone();

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "hi".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let hi_sm = fs::read_to_string(site.join("sitemap-hi.xml")).unwrap();
        // sitemap-hi.xml describes the /hi/ copies: the self xhtml:link
        // is resolver-labelled.
        assert!(
            hi_sm.contains(
                "hreflang=\"hi-IN\" href=\"https://example.com/hi/about.html\""
            ),
            "sitemap-hi self alternate should be hi-IN: {hi_sm}"
        );
        assert!(
            hi_sm.contains(
                "hreflang=\"en\" href=\"https://example.com/en/about.html\""
            ),
            "sitemap-hi alternate to en keeps its label: {hi_sm}"
        );

        let en_sm = fs::read_to_string(site.join("sitemap-en.xml")).unwrap();
        // From sitemap-en.xml, the hi entry names the hi document, so
        // it carries that document's resolved language — matching the
        // in-page alternates above.
        assert!(
            en_sm.contains(
                "hreflang=\"hi-IN\" href=\"https://example.com/hi/about.html\""
            ),
            "sitemap-en alternate to hi must carry hi's resolved language: {en_sm}"
        );
    }

    #[test]
    fn lang_switcher_self_entry_uses_resolved_page_language() {
        let tmp = tempdir().unwrap();
        let site = tmp.path().join("site");
        let build = tmp.path().join("build");
        fs::create_dir_all(&site).unwrap();
        // Pages carry the switcher marker so injection kicks in.
        write_html(&site, "en/page.html", LANG_SWITCHER_MARKER);
        write_html(&site, "hi/page.html", LANG_SWITCHER_MARKER);
        write_lang_sidecar(&build, "hi/page.html", r#"{"language":"hi-IN"}"#);
        let ctx = make_ctx_with_build(&site, &build);

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "hi".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let hi = fs::read_to_string(site.join("hi/page.html")).unwrap();
        // Self entry: resolved language on lang=/hreflang=, visible
        // label still the locale directory name.
        assert!(
            hi.contains(
                "lang=\"hi-IN\" hreflang=\"hi-IN\" aria-current=\"page\">hi</a>"
            ),
            "switcher self entry should use the resolved language: {hi}"
        );
        // Other-locale entry unchanged.
        assert!(
            hi.contains("lang=\"en\" hreflang=\"en\">en</a>"),
            "switcher alternate entry keeps the locale label: {hi}"
        );
    }

    #[test]
    fn transform_html_self_reference_uses_resolved_page_language() {
        let tmp = tempdir().unwrap();
        let ctx = a5_fixture(tmp.path());
        let site = ctx.site_dir.clone();

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "hi".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let plugin = I18nPlugin::new(config);

        let hi_path = site.join("hi/about.html");
        let html = fs::read_to_string(&hi_path).unwrap();
        let out = plugin.transform_html(&html, &hi_path, &ctx).unwrap();
        assert!(
            out.contains(
                "hreflang=\"hi-IN\" href=\"https://example.com/hi/about.html\""
            ),
            "fused-transform self-reference should be resolved: {out}"
        );
        assert!(
            out.contains(
                "hreflang=\"en\" href=\"https://example.com/en/about.html\""
            ),
            "fused-transform alternate keeps its label: {out}"
        );
    }

    // ── I18nPlugin with actual locale directories ───────────────

    #[test]
    fn plugin_with_locale_dirs_but_no_shared_pages_skips_injection() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        // en has page A, fr has page B — no overlap
        write_html(site, "en/about.html", "EN About");
        write_html(site, "fr/contact.html", "FR Contact");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        // No hreflang should be injected since no pages are shared
        let en = fs::read_to_string(site.join("en/about.html")).unwrap();
        let fr = fs::read_to_string(site.join("fr/contact.html")).unwrap();
        assert!(!en.contains(HREFLANG_MARKER));
        assert!(!fr.contains(HREFLANG_MARKER));
    }

    #[test]
    fn plugin_skips_when_only_one_locale_dir_exists() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        // Only en directory exists, fr is configured but missing
        write_html(site, "en/index.html", "EN");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en = fs::read_to_string(site.join("en/index.html")).unwrap();
        assert!(!en.contains(HREFLANG_MARKER));
    }

    // ── Root-hosted default locale (blocker 2) ───────────────────

    #[test]
    fn root_hosted_default_locale_gets_reciprocal_hreflang() {
        // Hosting the default locale at the site root — Hugo's
        // `defaultContentLanguageInSubdir = false`, Astro's
        // `prefixDefaultLocale: false`, Next.js's default — is the
        // prevailing convention and keeps clean URLs for the locale
        // that usually carries most of the traffic.
        //
        // `en` therefore has NO `en/` directory: its pages sit at the
        // site root, alongside the `fr/` directory.
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "index.html", "Hello");
        write_html(site, "about/index.html", "About");
        write_html(site, "fr/index.html", "Bonjour");
        write_html(site, "fr/about/index.html", "À propos");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en = fs::read_to_string(site.join("index.html")).unwrap();
        let fr = fs::read_to_string(site.join("fr/index.html")).unwrap();

        assert!(
            en.contains(HREFLANG_MARKER),
            "root-hosted en home page must receive hreflang: {en}"
        );
        assert!(
            fr.contains(HREFLANG_MARKER),
            "fr home page must receive hreflang: {fr}"
        );

        // The root-hosted locale's own URL carries no locale segment,
        // and nothing anywhere emits an `/en/` path.
        assert!(
            en.contains("href=\"https://example.com/index.html\""),
            "root-hosted self URL must omit the locale segment: {en}"
        );
        assert!(
            en.contains("href=\"https://example.com/fr/index.html\""),
            "en must point at the fr alternate: {en}"
        );
        // …and reciprocally, so the hreflang audit gate passes.
        assert!(
            fr.contains("href=\"https://example.com/index.html\""),
            "fr must link back to the root-hosted en page: {fr}"
        );
        assert!(
            en.contains(
                "hreflang=\"x-default\" href=\"https://example.com/index.html\""
            ),
            "x-default must resolve through the root-hosted default: {en}"
        );
        for (name, page) in [("en", &en), ("fr", &fr)] {
            assert!(
                !page.contains("https://example.com/en/"),
                "{name} page must never emit an /en/ URL: {page}"
            );
        }

        // Nested pages resolve the same way.
        let en_about =
            fs::read_to_string(site.join("about/index.html")).unwrap();
        assert!(
            en_about
                .contains("href=\"https://example.com/fr/about/index.html\""),
            "nested root-hosted page must find its fr alternate: {en_about}"
        );
        assert!(
            en_about.contains("href=\"https://example.com/about/index.html\""),
            "nested root-hosted self URL must omit the segment: {en_about}"
        );

        // The root index is a real page — the locale-redirect shim must
        // not be written over it.
        assert!(
            !en.contains("ssg-locale-redirect"),
            "root-hosted home page must not be replaced by the redirect shim"
        );
    }

    #[test]
    fn detect_locales_reports_root_hosted_default_locale() {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("fr")).unwrap();
        write_html(tmp.path(), "index.html", "Hello");

        let (present, root) =
            detect_locales(tmp.path(), &["en".into(), "fr".into()], "en");

        assert_eq!(present, vec!["en", "fr"]);
        assert_eq!(root.as_deref(), Some("en"));
    }

    #[test]
    fn detect_locales_ignores_root_html_when_default_has_its_own_dir() {
        // The site root's index.html here is the locale-redirect shim,
        // not a page — `en/` exists, so nothing is root-hosted.
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("en")).unwrap();
        fs::create_dir(tmp.path().join("fr")).unwrap();
        write_html(tmp.path(), "index.html", "redirect");

        let (present, root) =
            detect_locales(tmp.path(), &["en".into(), "fr".into()], "en");

        assert_eq!(present, vec!["en", "fr"]);
        assert_eq!(root, None);
    }

    #[test]
    fn detect_locales_needs_root_html_to_call_the_default_root_hosted() {
        // `fr/` alone with an empty root is a single-locale site, not a
        // root-hosted default — otherwise every partially-built site
        // would claim two locales.
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("fr")).unwrap();

        let (present, root) =
            detect_locales(tmp.path(), &["en".into(), "fr".into()], "en");

        assert_eq!(present, vec!["fr"]);
        assert_eq!(root, None);
    }

    #[test]
    fn build_url_root_hosted_locale_omits_the_locale_segment() {
        for strategy in
            [UrlPrefixStrategy::SubPath, UrlPrefixStrategy::SubDomain]
        {
            assert_eq!(
                build_url(
                    "https://example.com",
                    "en",
                    "about/index.html",
                    &strategy,
                    Some("en"),
                ),
                "https://example.com/about/index.html",
                "root-hosted locale keeps no segment under {strategy:?}"
            );
        }
        // Non-root locales are unaffected.
        assert_eq!(
            build_url(
                "https://example.com",
                "fr",
                "about/index.html",
                &UrlPrefixStrategy::SubPath,
                Some("en"),
            ),
            "https://example.com/fr/about/index.html"
        );
    }

    #[test]
    fn resolve_locale_and_rel_assigns_root_pages_to_the_root_locale() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        let locales = vec!["en".to_string(), "fr".to_string()];

        // Root page -> the root-hosted locale, whole path as rel.
        let (locale, rel) = resolve_locale_and_rel(
            &site.join("about/index.html"),
            site,
            &locales,
            Some("en"),
        )
        .unwrap();
        assert_eq!((locale.as_str(), rel.as_str()), ("en", "about/index.html"));

        // A real locale directory still wins.
        let (locale, rel) = resolve_locale_and_rel(
            &site.join("fr/about/index.html"),
            site,
            &locales,
            Some("en"),
        )
        .unwrap();
        assert_eq!((locale.as_str(), rel.as_str()), ("fr", "about/index.html"));

        // Without a root locale, root pages stay unclaimed.
        assert!(resolve_locale_and_rel(
            &site.join("about/index.html"),
            site,
            &locales,
            None
        )
        .is_none());
    }

    #[test]
    fn collect_locale_pages_root_walk_skips_locale_and_dot_dirs() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        write_html(site, "index.html", "EN");
        write_html(site, "guides/one/index.html", "EN nested");
        write_html(site, "fr/index.html", "FR");
        // Build metadata, not pages.
        write_html(site, ".meta/leak.html", "meta");
        write_html(site, ".ssg-cache/leak.html", "cache");

        let (pages, keys) = collect_locale_pages(
            site,
            &site.join(".meta"),
            &["en".to_string(), "fr".to_string()],
            Some("en"),
        )
        .unwrap();

        let mut en_paths: Vec<&String> = pages
            .values()
            .filter_map(|locale_map| locale_map.get("en"))
            .collect();
        en_paths.sort();
        assert_eq!(
            en_paths,
            vec!["guides/one/index.html", "index.html"],
            "root walk must skip fr/ and every dot-directory"
        );
        assert!(
            keys.contains_key(&("en".to_string(), "index.html".to_string())),
            "the reverse index must resolve every collected page"
        );
        assert_eq!(
            pages.get("index.html").map(BTreeMap::len),
            Some(2),
            "index.html is served by both locales"
        );
    }

    // ── Translated slugs via `translation_key` (blocker 1) ───────

    /// Writes a front-matter sidecar keyed the way `emit_sidecars`
    /// keys them: by the *content* path, not the compiled HTML path.
    /// `content/fr/a-propos.md` → `.meta/fr/a-propos.meta.json`, which
    /// backs the page compiled to `fr/a-propos/index.html`.
    fn write_content_sidecar(build_dir: &Path, content_rel: &str, json: &str) {
        let sidecar = build_dir
            .join(".meta")
            .join(format!("{content_rel}.meta.json"));
        fs::create_dir_all(sidecar.parent().expect("parent")).expect("mkdir");
        fs::write(sidecar, json).expect("write sidecar");
    }

    #[test]
    fn translation_key_links_pages_with_translated_slugs() {
        // `/about/` and `/fr/a-propos/` are the same logical page.
        // Nothing in their paths says so — only a shared
        // `translation_key` in front matter does, exactly as Hugo's
        // `translationKey` works.
        let tmp = tempdir().unwrap();
        let site = tmp.path().join("site");
        let build = tmp.path().join("build");
        fs::create_dir_all(&site).unwrap();

        write_html(&site, "index.html", "Home");
        write_html(&site, "about/index.html", "About");
        write_html(&site, "fr/index.html", "Accueil");
        write_html(&site, "fr/a-propos/index.html", "À propos");

        write_content_sidecar(&build, "index", r#"{"translation_key":"home"}"#);
        write_content_sidecar(
            &build,
            "about",
            r#"{"translation_key":"about"}"#,
        );
        write_content_sidecar(
            &build,
            "fr/index",
            r#"{"translation_key":"home"}"#,
        );
        write_content_sidecar(
            &build,
            "fr/a-propos",
            r#"{"translation_key":"about"}"#,
        );

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx_with_build(&site, &build);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en_about =
            fs::read_to_string(site.join("about/index.html")).unwrap();
        let fr_about =
            fs::read_to_string(site.join("fr/a-propos/index.html")).unwrap();

        assert!(
            en_about.contains(HREFLANG_MARKER),
            "/about/ must be linked to its translation: {en_about}"
        );
        assert!(
            en_about.contains(
                "href=\"https://example.com/fr/a-propos/index.html\""
            ),
            "/about/ must point at the TRANSLATED fr slug: {en_about}"
        );
        assert!(
            !en_about.contains("https://example.com/fr/about/"),
            "the untranslated fr slug must never be emitted: {en_about}"
        );
        assert!(
            fr_about.contains("href=\"https://example.com/about/index.html\""),
            "/fr/a-propos/ must link back to the en slug: {fr_about}"
        );

        // Home pages share a key too, with identical slugs.
        let en_home = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(
            en_home.contains("href=\"https://example.com/fr/index.html\""),
            "home pages must still pair up: {en_home}"
        );
    }

    #[test]
    fn pages_without_translation_key_keep_path_matching() {
        // Existing single- and multi-locale sites carry no
        // `translation_key`; identical paths must go on pairing.
        let tmp = tempdir().unwrap();
        let site = tmp.path().join("site");
        let build = tmp.path().join("build");
        fs::create_dir_all(&site).unwrap();
        write_html(&site, "en/about.html", "About");
        write_html(&site, "fr/about.html", "À propos");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx_with_build(&site, &build);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en = fs::read_to_string(site.join("en/about.html")).unwrap();
        assert!(
            en.contains("href=\"https://example.com/fr/about.html\""),
            "path matching must survive as the fallback: {en}"
        );
    }

    // ── build_url subdomain fallback ────────────────────────────

    #[test]
    fn subdomain_strategy_fallback_without_scheme() {
        // When base has no "://" it falls back to sub-path style
        let url = build_url(
            "example.com",
            "fr",
            "page.html",
            &UrlPrefixStrategy::SubDomain,
            None,
        );
        assert_eq!(url, "example.com/fr/page.html");
    }

    #[test]
    fn nested_pages_get_hreflang() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/docs/guide.html", "EN Guide");
        write_html(site, "fr/docs/guide.html", "FR Guide");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let en = fs::read_to_string(site.join("en/docs/guide.html")).unwrap();
        assert!(en.contains(HREFLANG_MARKER));
        assert!(en.contains("https://example.com/en/docs/guide.html"));
        assert!(en.contains("https://example.com/fr/docs/guide.html"));
    }

    // ── parse_accept_language ───────────────────────────────────

    #[test]
    fn parse_accept_language_basic() {
        let result = parse_accept_language("en, fr, de");
        assert_eq!(result, vec!["en", "fr", "de"]);
    }

    #[test]
    fn parse_accept_language_with_quality() {
        let result = parse_accept_language(
            "fr-CH, fr;q=0.9, en;q=0.8, de;q=0.7, *;q=0.5",
        );
        assert_eq!(result, vec!["fr-CH", "fr", "en", "de", "*"]);
    }

    #[test]
    fn parse_accept_language_with_whitespace() {
        let result = parse_accept_language("  en , fr ; q=0.8 , de ; q=0.5 ");
        assert_eq!(result, vec!["en", "fr", "de"]);
    }

    #[test]
    fn parse_accept_language_empty() {
        let result = parse_accept_language("");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_accept_language_single() {
        let result = parse_accept_language("en");
        assert_eq!(result, vec!["en"]);
    }

    #[test]
    fn parse_accept_language_wildcard_only() {
        let result = parse_accept_language("*");
        assert_eq!(result, vec!["*"]);
    }

    // ── negotiate_locale ────────────────────────────────────────

    #[test]
    fn negotiate_exact_match() {
        let preferred = vec!["fr".into()];
        let available = vec!["en".into(), "fr".into(), "de".into()];
        assert_eq!(negotiate_locale(&preferred, &available, "en"), "fr");
    }

    #[test]
    fn negotiate_prefix_match() {
        let preferred = vec!["fr-CH".into()];
        let available = vec!["en".into(), "fr".into(), "de".into()];
        assert_eq!(negotiate_locale(&preferred, &available, "en"), "fr");
    }

    #[test]
    fn negotiate_default_fallback() {
        let preferred = vec!["ja".into()];
        let available = vec!["en".into(), "fr".into()];
        assert_eq!(negotiate_locale(&preferred, &available, "en"), "en");
    }

    #[test]
    fn negotiate_case_insensitive() {
        let preferred = vec!["FR".into()];
        let available = vec!["en".into(), "fr".into()];
        assert_eq!(negotiate_locale(&preferred, &available, "en"), "fr");
    }

    #[test]
    fn negotiate_wildcard_ignored() {
        let preferred = vec!["*".into()];
        let available = vec!["en".into(), "fr".into()];
        assert_eq!(negotiate_locale(&preferred, &available, "en"), "en");
    }

    #[test]
    fn negotiate_no_match_returns_default() {
        let preferred: Vec<String> = vec![];
        let available = vec!["en".into(), "fr".into()];
        assert_eq!(negotiate_locale(&preferred, &available, "fr"), "fr");
    }

    // ── generate_locale_redirect ────────────────────────────────

    #[test]
    fn locale_redirect_contains_all_locales() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        fs::create_dir_all(site).unwrap();

        let locales = vec!["en".into(), "fr".into(), "de".into()];
        crate::server::generate_locale_redirect(site, &locales, "en").unwrap();

        let content = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(content.contains("\"en\""), "missing en locale");
        assert!(content.contains("\"fr\""), "missing fr locale");
        assert!(content.contains("\"de\""), "missing de locale");
    }

    #[test]
    fn locale_redirect_noscript_fallback() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        fs::create_dir_all(site).unwrap();

        crate::server::generate_locale_redirect(
            site,
            &["en".into(), "fr".into()],
            "en",
        )
        .unwrap();

        let content = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(content.contains("<noscript>"), "missing noscript tag");
        assert!(
            content.contains("url=/en/"),
            "noscript should redirect to default locale"
        );
    }

    #[test]
    fn locale_redirect_preserves_existing_non_redirect_index() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        fs::create_dir_all(site).unwrap();

        // Write a custom index.html first
        fs::write(site.join("index.html"), "<html>Custom</html>").unwrap();

        crate::server::generate_locale_redirect(
            site,
            &["en".into(), "fr".into()],
            "en",
        )
        .unwrap();

        let content = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(content, "<html>Custom</html>");
    }

    #[test]
    fn after_compile_generates_locale_redirect() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();

        write_html(site, "en/index.html", "EN");
        write_html(site, "fr/index.html", "FR");

        let config = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };

        let ctx = make_ctx(site);
        I18nPlugin::new(config).after_compile(&ctx).unwrap();

        let index = site.join("index.html");
        assert!(index.exists(), "root index.html should be generated");
        let content = fs::read_to_string(&index).unwrap();
        assert!(content.contains("ssg-locale-redirect"));
        assert!(content.contains("\"en\""));
        assert!(content.contains("\"fr\""));
    }

    #[test]
    fn test_collect_html_files_recursive_missing_dir_returns_io_error() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("missing");
        let mut found = Vec::new();
        let res = collect_html_files_recursive(
            &missing, &missing, "en", "en", &mut found,
        );
        assert!(res.is_err());
        let dbg = format!("{:?}", res.unwrap_err());
        assert!(dbg.contains("Io"), "expected Io variant, got: {dbg}");
        assert!(
            dbg.contains("missing"),
            "error should carry the missing path: {dbg}"
        );
    }

    #[test]
    fn test_generate_locale_sitemaps_invalid_dir_returns_io_error() {
        let tmp = tempdir().unwrap();
        let file_path = tmp.path().join("file");
        fs::write(&file_path, "").unwrap();

        let mut pages: LocaleMatrix = HashMap::new();
        let _ = pages.insert(
            "index.html".to_string(),
            locale_map(&["en"], "index.html"),
        );

        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            &file_path,
            Path::new("templates"),
        );
        let res = generate_locale_sitemaps(
            &ctx,
            &pages,
            &["en".to_string()],
            "en",
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert!(res.is_err());
        let dbg = format!("{:?}", res.unwrap_err());
        assert!(dbg.contains("Io"), "expected Io variant, got: {dbg}");
    }

    // ── transform_html (issue #522 fused-transform pass) ────────────

    #[test]
    fn has_transform_is_true() {
        let p = I18nPlugin::new(I18nConfig::default());
        assert!(p.has_transform());
    }

    #[test]
    fn transform_html_single_locale_returns_unchanged() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        let p = I18nPlugin::new(I18nConfig::default());
        let out = p
            .transform_html("<html><head></head></html>", tmp.path(), &ctx)
            .unwrap();
        assert_eq!(out, "<html><head></head></html>");
    }

    #[test]
    fn transform_html_already_injected_is_idempotent() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        let html = "<html><head><link rel=\"alternate\" hreflang=\"en\" href=\"x\" /></head></html>";
        let out = p.transform_html(html, tmp.path(), &ctx).unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn transform_html_fewer_than_two_locales_on_disk_returns_unchanged() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        write_html(site, "en/index.html", "EN");
        // Only one locale dir → cache.present_locales.len() < 2 path.
        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        let ctx = make_ctx(site);
        let path = site.join("en/index.html");
        let out = p
            .transform_html("<html><head></head></html>", &path, &ctx)
            .unwrap();
        assert_eq!(out, "<html><head></head></html>");
    }

    #[test]
    fn transform_html_path_outside_locale_returns_unchanged() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        write_html(site, "en/index.html", "EN");
        write_html(site, "fr/index.html", "FR");

        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        let ctx = make_ctx(site);
        // Path has no recognised locale prefix segment.
        let path = site.join("untracked.html");
        let out = p
            .transform_html("<html><head></head></html>", &path, &ctx)
            .unwrap();
        assert_eq!(out, "<html><head></head></html>");
    }

    #[test]
    fn transform_html_page_missing_from_matrix_returns_unchanged() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        write_html(site, "en/index.html", "EN");
        write_html(site, "fr/index.html", "FR");

        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        let ctx = make_ctx(site);
        // Path under a known locale dir but not present in either matrix
        // (we never wrote `en/missing.html`).
        let path = site.join("en/missing.html");
        let out = p
            .transform_html("<html><head></head></html>", &path, &ctx)
            .unwrap();
        assert_eq!(out, "<html><head></head></html>");
    }

    #[test]
    fn transform_html_injects_hreflang_for_shared_page() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        write_html(site, "en/index.html", "EN");
        write_html(site, "fr/index.html", "FR");

        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        let ctx = make_ctx(site);
        let path = site.join("en/index.html");
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let out = p.transform_html(html, &path, &ctx).unwrap();
        assert!(out.contains(HREFLANG_MARKER), "missing hreflang: {out}");
        assert!(out.contains("hreflang=\"x-default\""));
        // SubPath strategy default base_url.
        assert!(out.contains("https://example.com/en/index.html"));
    }

    #[test]
    fn transform_html_single_locale_page_returns_unchanged() {
        // Page exists in only one locale even though two locale dirs are
        // present on disk — `page_locales.len() < 2` early-out.
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        write_html(site, "en/only.html", "EN");
        write_html(site, "fr/other.html", "FR");

        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        let ctx = make_ctx(site);
        let path = site.join("en/only.html");
        let html = "<html><head></head></html>";
        let out = p.transform_html(html, &path, &ctx).unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn transform_html_no_head_close_returns_unchanged() {
        let tmp = tempdir().unwrap();
        let site = tmp.path();
        write_html(site, "en/index.html", "EN");
        write_html(site, "fr/index.html", "FR");

        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        let ctx = make_ctx(site);
        let path = site.join("en/index.html");
        // No </head> tag at all — inject_before_head_close returns None.
        let html = "<html><body>no head close</body></html>";
        let out = p.transform_html(html, &path, &ctx).unwrap();
        assert_eq!(out, html);
    }

    // ── resolve_locale_and_rel direct unit tests ────────────────────

    #[test]
    fn resolve_locale_and_rel_extracts_locale_and_rel() {
        let site = PathBuf::from("/site");
        let path = PathBuf::from("/site/en/about/index.html");
        let locales = vec!["en".to_string(), "fr".to_string()];
        let res = resolve_locale_and_rel(&path, &site, &locales, None).unwrap();
        assert_eq!(res.0, "en");
        assert_eq!(res.1, "about/index.html");
    }

    #[test]
    fn resolve_locale_and_rel_returns_none_when_not_under_site_dir() {
        let site = PathBuf::from("/site");
        let path = PathBuf::from("/somewhere-else/en/index.html");
        let locales = vec!["en".to_string()];
        assert!(resolve_locale_and_rel(&path, &site, &locales, None).is_none());
    }

    #[test]
    fn resolve_locale_and_rel_returns_none_for_unknown_locale_segment() {
        let site = PathBuf::from("/site");
        let path = PathBuf::from("/site/de/page.html");
        let locales = vec!["en".to_string(), "fr".to_string()];
        assert!(resolve_locale_and_rel(&path, &site, &locales, None).is_none());
    }

    #[test]
    fn resolve_locale_and_rel_returns_none_for_bare_locale_dir() {
        // `/site/en` with no further path segment.
        let site = PathBuf::from("/site");
        let path = PathBuf::from("/site/en");
        let locales = vec!["en".to_string()];
        assert!(resolve_locale_and_rel(&path, &site, &locales, None).is_none());
    }

    // ── inject_lang_switcher (replace marker path) ──────────────────

    /// html-generator minifies some pages during generation, before any
    /// plugin runs, and minification strips comments — so the comment
    /// marker was gone by the time the switcher was injected, and the
    /// switcher silently vanished from every minified page. The element
    /// form survives.
    #[test]
    fn lang_switcher_element_marker_is_found_and_replaced() {
        let html =
            r#"<html><body><div data-ssg-lang-switcher></div></body></html>"#;
        let found = find_lang_switcher_element(html).expect("element found");
        assert_eq!(
            &html[found.0..found.1],
            "<div data-ssg-lang-switcher></div>"
        );
    }

    /// Minifiers reformat attributes and drop whitespace; the marker has
    /// to survive both.
    #[test]
    fn lang_switcher_element_marker_survives_reformatting() {
        for html in [
            r#"<nav data-ssg-lang-switcher></nav>"#,
            r#"<div class=x data-ssg-lang-switcher ></div>"#,
            "<div data-ssg-lang-switcher>\n  </div>",
        ] {
            assert!(
                find_lang_switcher_element(html).is_some(),
                "should match: {html}"
            );
        }
    }

    /// A non-empty element is content, not a placeholder — replacing it
    /// would destroy the author's markup.
    #[test]
    fn lang_switcher_element_marker_ignores_non_empty_elements() {
        let html = r#"<div data-ssg-lang-switcher>keep me</div>"#;
        assert!(find_lang_switcher_element(html).is_none());
    }

    /// The attribute must belong to the tag it was found inside.
    #[test]
    fn lang_switcher_element_marker_ignores_a_bare_mention() {
        let html = "<p>use data-ssg-lang-switcher in your template</p>";
        assert!(find_lang_switcher_element(html).is_none());
    }

    #[test]
    fn inject_lang_switcher_replaces_marker_when_present() {
        let html = "<html><body><!-- ssg:lang-switcher --></body></html>";
        let out = inject_lang_switcher(
            html,
            "en",
            &locale_map(&["en", "fr"], "index.html"),
            &locale_map(&["en", "fr"], "")
                .keys()
                .map(|l| (l.clone(), l.clone()))
                .collect(),
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert!(!out.contains("<!-- ssg:lang-switcher -->"));
        assert!(out.contains("lang-switcher"));
        assert!(out.contains("lang=\"fr\""));
    }

    #[test]
    fn inject_lang_switcher_without_marker_returns_unchanged() {
        let html = "<html><body>nothing here</body></html>";
        let out = inject_lang_switcher(
            html,
            "en",
            &locale_map(&["en"], "index.html"),
            &locale_map(&["en"], "en"),
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(out, html);
    }

    // ── rewrite_ap_lang_items edge cases ────────────────────────────

    #[test]
    fn rewrite_ap_lang_items_returns_unchanged_when_marker_absent() {
        let html = "<a href=\"/whatever\">link</a>";
        let locales = locale_map(&["en"], "page.html");
        let out = rewrite_ap_lang_items(
            html,
            &locales,
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(out, html);
    }

    #[test]
    fn rewrite_ap_lang_items_skips_unknown_lang() {
        // The data-lang attribute references a locale not in the page
        // matrix — link should be left alone.
        let input =
            "<a class=\"ap-lang-item\" href=\"/de/\" data-lang=\"de\">DE</a>";
        let locales = locale_map(&["en", "fr"], "page.html");
        let out = rewrite_ap_lang_items(
            input,
            &locales,
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(out, input);
    }

    #[test]
    fn rewrite_ap_lang_items_handles_unterminated_anchor() {
        // Anchor open `<a ` with no closing `>` — must not panic, must
        // return early.
        let input = "<a class=\"ap-lang-item\" data-lang=\"fr\"";
        let locales = locale_map(&["fr"], "page.html");
        let out = rewrite_ap_lang_items(
            input,
            &locales,
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(out, input);
    }

    #[test]
    fn rewrite_ap_lang_items_with_subdomain_strategy_strips_host() {
        let input =
            "<a class=\"ap-lang-item\" href=\"/fr/\" data-lang=\"fr\">F</a>";
        let locales = locale_map(&["fr"], "page.html");
        let out = rewrite_ap_lang_items(
            input,
            &locales,
            "https://example.com",
            &UrlPrefixStrategy::SubDomain,
            None,
        );
        // SubDomain produces https://fr.example.com/page.html, so href
        // becomes the path part only.
        assert!(out.contains("href=\"/page.html\""), "out={out}");
    }

    // ── parse_accept_language edge cases ────────────────────────────

    #[test]
    fn parse_accept_language_skips_empty_parts() {
        // Trailing comma / double comma produce empty parts after split.
        let out = parse_accept_language("en,,fr,");
        assert_eq!(out, vec!["en", "fr"]);
    }

    #[test]
    fn parse_accept_language_skips_empty_locale_with_quality() {
        // A part that's just `;q=0.5` (no locale) must be filtered out.
        let out = parse_accept_language(";q=0.5, en");
        assert_eq!(out, vec!["en"]);
    }

    #[test]
    fn parse_accept_language_quality_zero_sorts_last() {
        let out = parse_accept_language("fr;q=0.0, en;q=0.5, de;q=1.0");
        assert_eq!(out, vec!["de", "en", "fr"]);
    }

    #[test]
    fn parse_accept_language_unparseable_quality_defaults_to_one() {
        let out = parse_accept_language("en;q=foo, fr;q=0.5");
        // `en` has invalid q, defaults to 1.0 → sorts ahead of fr.
        assert_eq!(out, vec!["en", "fr"]);
    }

    #[test]
    fn parse_accept_language_missing_q_prefix_defaults_to_one() {
        // The segment after `;` doesn't start with `q=`, so
        // `strip_prefix("q=")` fails and quality falls back to 1.0
        // rather than being parsed from the bare number.
        let out = parse_accept_language("en;0.1, fr");
        assert_eq!(out.len(), 2);
        assert!(out.contains(&"en".to_string()));
        assert!(out.contains(&"fr".to_string()));
    }

    #[test]
    fn parse_accept_language_nan_quality_uses_equal_fallback_in_sort() {
        // "nan" parses successfully via f64::from_str to NaN. NaN's
        // partial_cmp always returns None, exercising the
        // `unwrap_or(Ordering::Equal)` fallback in the sort comparator.
        let out = parse_accept_language("en;q=nan, fr;q=0.5");
        assert_eq!(out.len(), 2);
        assert!(out.contains(&"en".to_string()));
        assert!(out.contains(&"fr".to_string()));
    }

    // ── ensure_matrix cache short-circuit ───────────────────────────

    #[test]
    fn ensure_matrix_short_circuits_when_site_dir_unchanged() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");

        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        let ctx = make_ctx(tmp.path());
        p.ensure_matrix(&ctx).unwrap();
        // Second call against the same dir hits the cache fast-path.
        p.ensure_matrix(&ctx).unwrap();
        let cache = p.matrix.read().unwrap();
        assert_eq!(cache.present_locales, vec!["en", "fr"]);
    }

    // ── ensure_matrix re-entry on different site_dir ────────────────

    #[test]
    fn ensure_matrix_rebuilds_when_site_dir_changes() {
        let tmp1 = tempdir().unwrap();
        let tmp2 = tempdir().unwrap();
        write_html(tmp1.path(), "en/index.html", "EN1");
        write_html(tmp1.path(), "fr/index.html", "FR1");
        write_html(tmp2.path(), "en/about.html", "EN2");
        write_html(tmp2.path(), "fr/about.html", "FR2");

        let cfg = I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        };
        let p = I18nPlugin::new(cfg);
        // Populate cache against tmp1.
        p.ensure_matrix(&make_ctx(tmp1.path())).unwrap();
        // Then re-populate against tmp2 — must NOT short-circuit.
        p.ensure_matrix(&make_ctx(tmp2.path())).unwrap();
        let cache = p.matrix.read().unwrap();
        assert_eq!(cache.site_dir.as_deref(), Some(tmp2.path()));
        assert!(cache.pages.contains_key("about.html"));
    }

    // ── coverage: error propagation + rarely-taken branches ─────────

    /// Builds a two-locale config (`en` default).
    fn cfg_en_fr() -> I18nConfig {
        I18nConfig {
            default_locale: "en".into(),
            locales: vec!["en".into(), "fr".into()],
            url_prefix: UrlPrefixStrategy::SubPath,
        }
    }

    #[test]
    fn ensure_matrix_double_check_returns_early_for_racing_fillers() {
        use std::sync::Arc;

        // Two threads race to fill the matrix while the test pins the
        // read side: both pass the read-lock fast path (cache empty),
        // queue on the write lock, and whichever loses the race takes
        // the double-checked early return.
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");

        let plugin = Arc::new(I18nPlugin::new(cfg_en_fr()));
        let read_guard = plugin
            .matrix
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut handles = Vec::new();
        for _ in 0..2 {
            let p = Arc::clone(&plugin);
            let site = tmp.path().to_path_buf();
            handles.push(std::thread::spawn(move || {
                p.ensure_matrix(&make_ctx(&site)).expect("fill succeeds");
            }));
        }
        // Let both workers pass the read check and block on the write
        // lock before releasing it.
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(read_guard);
        for h in handles {
            h.join().expect("worker thread");
        }

        let cache = plugin
            .matrix
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(cache.site_dir.as_deref(), Some(tmp.path()));
        assert_eq!(cache.present_locales, vec!["en", "fr"]);
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_unreadable_locale_dir_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");
        let fr = tmp.path().join("fr");
        fs::set_permissions(&fr, fs::Permissions::from_mode(0o000)).unwrap();

        let ctx = make_ctx(tmp.path());
        let result = I18nPlugin::new(cfg_en_fr()).after_compile(&ctx);
        fs::set_permissions(&fr, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "unreadable locale dir must be an Err");
    }

    #[test]
    #[cfg(unix)]
    fn transform_html_propagates_unreadable_locale_dir_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");
        let fr = tmp.path().join("fr");
        fs::set_permissions(&fr, fs::Permissions::from_mode(0o000)).unwrap();

        let ctx = make_ctx(tmp.path());
        let result = I18nPlugin::new(cfg_en_fr()).transform_html(
            "<html><head></head><body>EN</body></html>",
            &tmp.path().join("en/index.html"),
            &ctx,
        );
        fs::set_permissions(&fr, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "unreadable locale dir must be an Err");
    }

    #[test]
    fn after_compile_without_config_falls_back_to_example_base_url() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");

        // PluginContext::new leaves `config` unset, driving the
        // map_or_else fallback closure.
        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );
        I18nPlugin::new(cfg_en_fr()).after_compile(&ctx).unwrap();

        let html =
            fs::read_to_string(tmp.path().join("en/index.html")).unwrap();
        assert!(
            html.contains("https://example.com/en/index.html"),
            "fallback base url expected: {html}"
        );
    }

    #[test]
    fn transform_html_without_config_falls_back_to_example_base_url() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");

        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );
        let out = I18nPlugin::new(cfg_en_fr())
            .transform_html(
                "<html><head><title>T</title></head><body>EN</body></html>",
                &tmp.path().join("en/index.html"),
                &ctx,
            )
            .unwrap();
        assert!(
            out.contains("https://example.com/en/index.html"),
            "fallback base url expected: {out}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_page_read_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");
        let en_page = tmp.path().join("en/index.html");
        fs::set_permissions(&en_page, fs::Permissions::from_mode(0o000))
            .unwrap();

        let ctx = make_ctx(tmp.path());
        let result = I18nPlugin::new(cfg_en_fr()).after_compile(&ctx);
        fs::set_permissions(&en_page, fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(result.is_err(), "unreadable page must surface as Err");
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_page_write_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");
        // Read succeeds, the write-back of the injected page does not.
        let en_page = tmp.path().join("en/index.html");
        fs::set_permissions(&en_page, fs::Permissions::from_mode(0o444))
            .unwrap();

        let ctx = make_ctx(tmp.path());
        let result = I18nPlugin::new(cfg_en_fr()).after_compile(&ctx);
        fs::set_permissions(&en_page, fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(result.is_err(), "read-only page must surface as Err");
    }

    #[test]
    fn after_compile_propagates_sitemap_write_error() {
        // A directory squatting on sitemap-en.xml makes the sitemap
        // write fail after hreflang injection succeeded.
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");
        fs::create_dir(tmp.path().join("sitemap-en.xml")).unwrap();

        let ctx = make_ctx(tmp.path());
        let err = I18nPlugin::new(cfg_en_fr())
            .after_compile(&ctx)
            .expect_err("sitemap write over a directory must fail");
        assert!(format!("{err:?}").contains("Io"));
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_locale_redirect_write_error() {
        use std::os::unix::fs::PermissionsExt;

        // Injection + sitemaps succeed; the root redirect (an existing
        // ssg-generated redirect page, now read-only) cannot be
        // rewritten and must surface as Err.
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");
        let index = tmp.path().join("index.html");
        fs::write(&index, "<!-- ssg-locale-redirect -->").unwrap();
        fs::set_permissions(&index, fs::Permissions::from_mode(0o444)).unwrap();

        let ctx = make_ctx(tmp.path());
        let result = I18nPlugin::new(cfg_en_fr()).after_compile(&ctx);
        fs::set_permissions(&index, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(result.is_err(), "redirect rewrite must surface as Err");
    }

    #[test]
    fn resolve_locale_and_rel_site_dir_itself_returns_none() {
        // strip_prefix leaves an empty relative path, so the first
        // component lookup takes the `?` None branch.
        let site = Path::new("/site");
        assert!(
            resolve_locale_and_rel(site, site, &["en".into()], None).is_none()
        );
    }

    #[test]
    fn collect_locale_pages_skips_locale_without_directory() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");

        let (map, _keys) = collect_locale_pages(
            tmp.path(),
            &tmp.path().join(".meta"),
            &["en".into(), "ghost".into()],
            None,
        )
        .unwrap();
        assert_eq!(map.len(), 1);
        assert!(map["index.html"].contains_key("en"));
    }

    #[test]
    #[cfg(unix)]
    fn collect_html_files_recursive_nested_unreadable_dir_errors() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let root = tmp.path().join("en");
        let nested = root.join("locked");
        fs::create_dir_all(&nested).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o000))
            .unwrap();

        let mut found = Vec::new();
        let res =
            collect_html_files_recursive(&root, &root, "en", "en", &mut found);
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(res.is_err(), "nested unreadable dir must be an Err");
    }

    #[test]
    fn collect_locale_pages_ignores_non_html_files() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        fs::write(tmp.path().join("en/style.css"), "body{}").unwrap();

        let (map, _keys) = collect_locale_pages(
            tmp.path(),
            &tmp.path().join(".meta"),
            &["en".into()],
            None,
        )
        .unwrap();
        assert_eq!(map.len(), 1, "css files must not be collected");
    }

    #[test]
    fn inject_hreflang_all_skips_locale_missing_from_page_set() {
        // Page shared by en+fr; `de` is in the locale list but not in
        // the page's locale set, taking the first `continue`.
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/index.html", "EN");
        write_html(tmp.path(), "fr/index.html", "FR");

        let mut pages: LocaleMatrix = HashMap::new();
        let _ = pages.insert(
            "index.html".to_string(),
            locale_map(&["en", "fr"], "index.html"),
        );

        let ctx = make_ctx(tmp.path());
        inject_hreflang_all(
            &ctx,
            &pages,
            &["en".into(), "fr".into(), "de".into()],
            "en",
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        )
        .unwrap();

        let html =
            fs::read_to_string(tmp.path().join("en/index.html")).unwrap();
        assert!(html.contains(HREFLANG_MARKER));
    }

    #[test]
    fn inject_hreflang_all_skips_page_file_missing_on_disk() {
        // The pages map claims an `fr` copy that does not exist; the
        // `!file.exists()` continue must skip it without error.
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "en/ghost.html", "EN");

        let mut pages: LocaleMatrix = HashMap::new();
        let _ = pages.insert(
            "ghost.html".to_string(),
            locale_map(&["en", "fr"], "ghost.html"),
        );

        let ctx = make_ctx(tmp.path());
        inject_hreflang_all(
            &ctx,
            &pages,
            &["en".into(), "fr".into()],
            "en",
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        )
        .unwrap();

        let html =
            fs::read_to_string(tmp.path().join("en/ghost.html")).unwrap();
        assert!(html.contains(HREFLANG_MARKER));
    }

    #[test]
    fn inject_hreflang_all_keeps_headless_page_unrewritten_inline() {
        // A shared page without a real <head> element: the injection
        // shim returns None and the original html flows through.
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("en")).unwrap();
        fs::create_dir_all(tmp.path().join("fr")).unwrap();
        let raw = "<html><body>no head</body></html>";
        fs::write(tmp.path().join("en/nohead.html"), raw).unwrap();
        fs::write(tmp.path().join("fr/nohead.html"), raw).unwrap();

        let ctx = make_ctx(tmp.path());
        I18nPlugin::new(cfg_en_fr()).after_compile(&ctx).unwrap();

        let html =
            fs::read_to_string(tmp.path().join("en/nohead.html")).unwrap();
        assert!(
            !html.contains(HREFLANG_MARKER),
            "headless page must not receive hreflang links: {html}"
        );
    }

    #[test]
    fn inject_before_head_close_stray_end_tag_returns_none() {
        // The lowercase substring check passes but lol_html never sees
        // a real <head> element, so the rewrite is a no-op and the
        // shim reports None.
        assert!(
            inject_before_head_close("stray closer</head>", "<x/>").is_none()
        );
    }

    #[test]
    fn rewrite_ap_lang_items_single_quoted_attrs_and_relative_base() {
        let locales = locale_map(&["fr"], "index.html");
        // Single-quoted attributes drive the second quote-candidate
        // iteration for both data-lang and href; the empty base yields
        // a non-http URL that is used verbatim.
        let input = "<a class='ap-lang-item' data-lang='fr' href='/old'>F</a>";
        let out = rewrite_ap_lang_items(
            input,
            &locales,
            "",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert!(out.contains("href='/fr/index.html'"), "got: {out}");
    }

    #[test]
    fn rewrite_ap_lang_items_unterminated_data_lang_left_unchanged() {
        let locales = locale_map(&["fr"], "index.html");
        // The data-lang value quote never closes inside the tag, so no
        // language is extracted and the tag is left as-is.
        let input = "<a class=\"ap-lang-item\" data-lang=\"fr>F</a>";
        let out = rewrite_ap_lang_items(
            input,
            &locales,
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(out, input);
    }

    #[test]
    fn rewrite_ap_lang_items_without_data_lang_left_unchanged() {
        let locales = locale_map(&["fr"], "index.html");
        let input = "<a class=\"ap-lang-item\" href=\"/x\">F</a>";
        let out = rewrite_ap_lang_items(
            input,
            &locales,
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(out, input);
    }

    #[test]
    fn rewrite_ap_lang_items_unterminated_href_left_unchanged() {
        let locales = locale_map(&["fr"], "index.html");
        // data-lang parses but the href value quote never closes, so
        // the href rewrite loop exhausts both quote candidates.
        let input =
            "<a class=\"ap-lang-item\" data-lang=\"fr\" href=\"/old>F</a>";
        let out = rewrite_ap_lang_items(
            input,
            &locales,
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(out, input);
    }

    #[test]
    fn rewrite_ap_lang_items_ignores_plain_anchor_tags() {
        let locales = locale_map(&["fr"], "index.html");
        // The document mentions ap-lang-item (so the fast path does
        // not bail) but the anchor itself is a plain link.
        let input = "<span>ap-lang-item</span><a href=\"/plain\">keep me</a>";
        let out = rewrite_ap_lang_items(
            input,
            &locales,
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert_eq!(out, input);
    }
}
