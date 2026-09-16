// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Render-time page-language precedence (spec A5, plan §2 1.5).
//!
//! The template engine emits `<html lang="{{ site.language }}">` for
//! every page (scaffold `templates/tera/base.html`, user templates).
//! Spec A5 requires the *page's* resolved language to win there, so
//! that `<html lang>` agrees with the other three language sinks
//! (JSON-LD `inLanguage`, `og:locale`, hreflang self-reference), all
//! of which resolve through
//! `crate::seo::lang::resolve_page_lang`.
//!
//! `resolve_page_lang` needs a *built* page (its on-disk path, its
//! rendered HTML, a [`PluginContext`](crate::plugin::PluginContext)),
//! none of which exist at template-render time — the render context
//! only has the page's front matter and the site globals. This module
//! is therefore the emitter-side subset of the same precedence chain:
//! steps 1 (front-matter `language`), 2 (front-matter `hreflang`),
//! 5 (site default) and 6 (the `"en"` constant). Steps 3 and 4
//! (locale path prefix, existing `<html lang>`) only apply to built
//! pages and remain in `seo::lang`, which the SEO sinks and the
//! language-mismatch audit gate keep using.
//!
//! ## Coordination note (Wave 2)
//!
//! [`normalize_bcp47`] is the single normalisation implementation:
//! the SEO-side `seo::lang::normalize_bcp47` is a thin wrapper over
//! this function so the render-time engine and the SEO sinks can
//! never disagree on canonical BCP-47 form.

// Crate-internal on purpose: this helper backs the template engine
// and must not become public API before the resolver work settles.
// rustc's `unreachable_pub` wants `pub(crate)` here while clippy's
// nursery `redundant_pub_crate` wants plain `pub` — keep the honest
// `pub(crate)` spelling and silence the nursery lint (same pattern as
// `seo::lang`).
#![allow(clippy::redundant_pub_crate)]

#[cfg(feature = "templates")]
use std::collections::HashMap;

/// Final constant fallback when no other source resolves (spec A5).
///
/// Mirrors `seo::lang::DEFAULT_PAGE_LANG`. Gated on `templates`: its
/// only consumer, [`resolve_render_lang`], is only reachable from
/// `template_engine::render_page`, itself feature-gated — without
/// this, `--no-default-features` sees it as dead code.
#[cfg(feature = "templates")]
pub(crate) const DEFAULT_PAGE_LANG: &str = "en";

/// Resolves the language a page should be *rendered* with, from the
/// information available inside a template render context.
///
/// Precedence, first match wins (the emitter-side subset of
/// `seo::lang::resolve_page_lang` — see the module docs):
///
/// 1. Front-matter `language`.
/// 2. Front-matter `hreflang`.
/// 3. The site default language (`site.language` global).
/// 4. [`DEFAULT_PAGE_LANG`] (`"en"`).
///
/// Every returned value is normalised to BCP-47 hyphen form
/// (`EN_gb` → `en-GB`), matching what the SEO sinks publish.
///
/// Gated on `templates`: its sole caller is
/// `template_engine::render_page`, itself feature-gated. Note
/// `normalize_bcp47` below is NOT gated — `seo::lang::normalize_bcp47`
/// delegates to it unconditionally.
#[cfg(feature = "templates")]
pub(crate) fn resolve_render_lang(
    frontmatter: &HashMap<String, serde_json::Value>,
    site_language: Option<&str>,
) -> String {
    for key in ["language", "hreflang"] {
        if let Some(lang) = frontmatter
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(normalize_bcp47)
        {
            return lang;
        }
    }
    site_language
        .and_then(normalize_bcp47)
        .unwrap_or_else(|| DEFAULT_PAGE_LANG.to_string())
}

/// Normalises a raw language tag into canonical BCP-47 hyphen form.
///
/// Accepts underscore-separated input (`en_GB`), lowercases the
/// primary subtag, uppercases two-letter regions and title-cases
/// four-letter scripts (`zh-hans` → `zh-Hans`). Returns `None` when
/// the value is empty or not shaped like a language tag, so callers
/// fall through to the next resolver source.
///
/// Byte-identical to `seo::lang::normalize_bcp47` — see the module
/// docs' coordination note.
/// Writing direction for a BCP-47 language tag: `"rtl"` or `"ltr"`.
///
/// Templates emit this as `<html lang="{{ site.language }}"
/// dir="{{ site.direction }}">`. Without it an Arabic or Hebrew page
/// renders left-to-right, which is not a subtle degradation — the
/// layout is simply wrong, and nothing in a build log says so.
///
/// Resolution follows BCP-47 precedence: an explicit script subtag
/// wins, because it is the thing that actually determines direction.
/// `az-Arab` is right-to-left while plain `az` is not, and `ku-Latn` is
/// left-to-right while `ku` alone is commonly Sorani and is not.
/// Without a script subtag the base language decides.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(text_direction("ar"), "rtl");
/// assert_eq!(text_direction("he-IL"), "rtl");
/// assert_eq!(text_direction("az-Arab"), "rtl");
/// assert_eq!(text_direction("az"), "ltr");
/// assert_eq!(text_direction("ku-Latn"), "ltr");
/// assert_eq!(text_direction("ja"), "ltr");
/// ```
#[must_use]
pub(crate) fn text_direction(lang: &str) -> &'static str {
    /// Scripts written right-to-left, by ISO 15924 subtag.
    const RTL_SCRIPTS: &[&str] = &[
        "adlm", "arab", "aran", "hebr", "mand", "nkoo", "rohg", "samr", "syrc",
        "thaa", "yiii",
    ];
    /// Languages whose default script is right-to-left.
    /// Deliberately excludes `ha` and plain `ku`. Both appear in a
    /// widely-copied RTL list, and both are wrong for the common case:
    /// modern Hausa is written in Latin (Boko), and plain `ku` is
    /// Kurmanji, also Latin. CLDR treats both as left-to-right. Sorani
    /// Kurdish is `ckb`, which is listed, and Ajami Hausa would be
    /// `ha-Arab`, which the script rule above catches.
    const RTL_LANGS: &[&str] = &[
        "ar", "arc", "ckb", "dv", "fa", "he", "iw", "ji", "ks", "ps", "sd",
        "ug", "ur", "yi",
    ];

    let lower = lang.trim().to_ascii_lowercase();
    let mut parts = lower.split(['-', '_']).filter(|p| !p.is_empty());
    let Some(base) = parts.next() else {
        return "ltr";
    };

    // A script subtag is exactly four letters. It overrides the base
    // language, in both directions.
    for part in parts {
        if part.len() == 4 && part.chars().all(|c| c.is_ascii_alphabetic()) {
            return if RTL_SCRIPTS.contains(&part) {
                "rtl"
            } else {
                "ltr"
            };
        }
    }

    if RTL_LANGS.contains(&base) {
        "rtl"
    } else {
        "ltr"
    }
}

pub(crate) fn normalize_bcp47(raw: &str) -> Option<String> {
    let cleaned = raw.trim().replace('_', "-");
    let mut parts = cleaned.split('-');

    // `split` always yields at least one item, so the fallback is
    // purely defensive: an empty primary fails the length check below.
    let primary = parts.next().unwrap_or_default();
    if !(2..=3).contains(&primary.len())
        || !primary.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return None;
    }

    let mut out = primary.to_ascii_lowercase();
    for sub in parts {
        if sub.is_empty() || !sub.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        out.push('-');
        match sub.len() {
            // Two-letter region: en-GB.
            2 if sub.bytes().all(|b| b.is_ascii_alphabetic()) => {
                out.push_str(&sub.to_ascii_uppercase());
            }
            // Four-letter script: zh-Hans.
            4 if sub.bytes().all(|b| b.is_ascii_alphabetic()) => {
                let (head, tail) = sub.split_at(1);
                out.push_str(&head.to_ascii_uppercase());
                out.push_str(&tail.to_ascii_lowercase());
            }
            // Anything else (numeric regions, variants): keep lowercase.
            _ => out.push_str(&sub.to_ascii_lowercase()),
        }
    }
    Some(out)
}

// Gated on `templates` to match the items under test: `HashMap` is
// imported behind that feature (line 41) and `resolve_render_lang` is
// defined behind it, so without the gate this module does not compile
// with default features off. It never had one, and nothing noticed —
// the feature-powerset job runs `cargo check`, which does not build
// test code, so `cargo test --lib --no-default-features` had simply
// never been compiled.
#[cfg(all(test, feature = "templates"))]
mod tests {
    use super::*;

    fn fm(pairs: &[(&str, &str)]) -> HashMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    (*k).to_string(),
                    serde_json::Value::String((*v).to_string()),
                )
            })
            .collect()
    }

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

    #[test]
    fn normalize_keeps_numeric_and_variant_subtags_lowercase() {
        // Numeric UN M.49 region (len 3, not alphabetic) and long
        // variant subtags take the catch-all lowercase arm.
        assert_eq!(normalize_bcp47("ES-419").as_deref(), Some("es-419"));
        assert_eq!(
            normalize_bcp47("en-GB-OXENDICT").as_deref(),
            Some("en-GB-oxendict")
        );
        // Two-char subtag that is not purely alphabetic also falls
        // through to the lowercase arm rather than region uppercasing.
        assert_eq!(normalize_bcp47("en-a1").as_deref(), Some("en-a1"));
    }

    #[test]
    fn frontmatter_language_beats_site_default() {
        let lang =
            resolve_render_lang(&fm(&[("language", "hi")]), Some("en-GB"));
        assert_eq!(lang, "hi");
    }

    #[test]
    fn frontmatter_hreflang_used_when_language_absent() {
        let lang =
            resolve_render_lang(&fm(&[("hreflang", "en_gb")]), Some("en"));
        assert_eq!(lang, "en-GB");
    }

    #[test]
    fn invalid_frontmatter_language_falls_through() {
        let lang = resolve_render_lang(
            &fm(&[("language", "not a lang")]),
            Some("fr-FR"),
        );
        assert_eq!(lang, "fr-FR");
    }

    #[test]
    fn site_default_used_when_no_frontmatter_signal() {
        let lang = resolve_render_lang(&HashMap::new(), Some("en-GB"));
        assert_eq!(lang, "en-GB");
    }

    #[test]
    fn en_constant_when_nothing_resolves() {
        assert_eq!(resolve_render_lang(&HashMap::new(), None), "en");
        assert_eq!(
            resolve_render_lang(&HashMap::new(), Some("")),
            DEFAULT_PAGE_LANG
        );
    }

    #[test]
    fn non_string_frontmatter_values_are_ignored() {
        let mut map = HashMap::new();
        let _ = map.insert("language".to_string(), serde_json::json!(42));
        assert_eq!(resolve_render_lang(&map, Some("de")), "de");
    }

    // ── text_direction ─────────────────────────────────────────

    #[test]
    fn right_to_left_languages_are_recognised() {
        for tag in ["ar", "he", "fa", "ur", "ps", "sd", "ug", "yi", "dv", "ckb"]
        {
            assert_eq!(text_direction(tag), "rtl", "{tag} should be rtl");
        }
    }

    #[test]
    fn left_to_right_languages_are_the_default() {
        for tag in ["en", "fr", "de", "es", "it", "pt", "ja", "zh", "ko", "ru"]
        {
            assert_eq!(text_direction(tag), "ltr", "{tag} should be ltr");
        }
    }

    #[test]
    fn a_region_subtag_does_not_change_direction() {
        assert_eq!(text_direction("he-IL"), "rtl");
        assert_eq!(text_direction("ar-EG"), "rtl");
        assert_eq!(text_direction("en-GB"), "ltr");
        assert_eq!(text_direction("zh-Hans-CN"), "ltr");
    }

    /// A script subtag decides, because it is the thing that actually
    /// determines direction. Azerbaijani in Arabic script is
    /// right-to-left; in Latin it is not.
    #[test]
    fn a_script_subtag_overrides_the_base_language() {
        assert_eq!(text_direction("az-Arab"), "rtl");
        assert_eq!(text_direction("az"), "ltr");
        assert_eq!(text_direction("ha-Arab"), "rtl");
        assert_eq!(text_direction("ku-Arab"), "rtl");
        // ...and in the other direction too.
        assert_eq!(text_direction("ar-Latn"), "ltr");
        assert_eq!(text_direction("ckb-Latn"), "ltr");
    }

    /// Hausa and plain Kurmanji Kurdish appear in a widely-copied RTL
    /// list and are wrong in it: modern Hausa is written in Latin
    /// (Boko), and plain `ku` is Kurmanji, also Latin. CLDR treats both
    /// as left-to-right.
    #[test]
    fn hausa_and_kurmanji_are_left_to_right() {
        assert_eq!(text_direction("ha"), "ltr");
        assert_eq!(text_direction("ku"), "ltr");
    }

    #[test]
    fn direction_is_case_and_separator_insensitive() {
        assert_eq!(text_direction("AR"), "rtl");
        assert_eq!(text_direction("he_IL"), "rtl");
        assert_eq!(text_direction("  ar  "), "rtl");
    }

    #[test]
    fn an_empty_tag_falls_back_to_left_to_right() {
        assert_eq!(text_direction(""), "ltr");
        assert_eq!(text_direction("   "), "ltr");
        assert_eq!(text_direction("-"), "ltr");
    }
}
