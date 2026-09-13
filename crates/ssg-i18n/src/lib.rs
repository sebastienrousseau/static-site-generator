// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Locale negotiation, hreflang and language switchers for multi-locale
//! sites.
//!
//! Extracted from `ssg`'s i18n plugin (#588). Everything here is a pure
//! function of its arguments — strings, maps and a strategy in, a string
//! out. Nothing reads a file or knows what a plugin is, which is what
//! lets it live outside the generator: `ssg-search` and `ssg-a11y` are
//! the same shape, and it is the only shape that avoids a dependency
//! cycle, since a crate implementing `Plugin` would have to depend on
//! the crate that wants to call it.
//!
//! What stayed behind in `ssg` is everything that walks or writes the
//! filesystem: locale detection, page collection, sitemap emission, and
//! the `Plugin` implementation that drives them.
//!
//! ```
//! use ssg_i18n::{negotiate_locale, parse_accept_language};
//!
//! let wanted = parse_accept_language("fr-CA,fr;q=0.9,en;q=0.5");
//! let have = ["en".to_string(), "fr".to_string()];
//! assert_eq!(negotiate_locale(&wanted, &have, "en"), "fr");
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The README's examples are real doctests: every assertion in it is
/// compiled and run, so the documented output cannot drift from the code.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// Strategy for constructing locale-specific URLs.
///
/// Marked `#[non_exhaustive]` so future strategies (e.g. query-string,
/// custom plugin-driven mapping) can be added non-breakingly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
#[non_exhaustive]
pub enum UrlPrefixStrategy {
    /// Locale appears as a path prefix: `https://example.com/fr/about`
    #[default]
    SubPath,
    /// Locale appears as a subdomain: `https://fr.example.com/about`
    SubDomain,
}

/// Parsed `[i18n]` configuration section.
///
/// # Example (TOML)
///
/// ```toml
/// [i18n]
/// default_locale = "en"
/// locales = ["en", "fr", "de"]
/// url_prefix = "sub_path"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct I18nConfig {
    /// The default / fallback locale (used for `x-default`).
    pub default_locale: String,
    /// All supported locales.
    pub locales: Vec<String>,
    /// How locale URLs are constructed.
    #[serde(default)]
    pub url_prefix: UrlPrefixStrategy,
}

impl Default for I18nConfig {
    fn default() -> Self {
        Self {
            default_locale: "en".to_string(),
            locales: vec!["en".to_string()],
            url_prefix: UrlPrefixStrategy::default(),
        }
    }
}

// ── Plugin ───────────────────────────────────────────────────────────

/// Sidecar file names that could carry `site_rel`'s front matter, most
/// likely first.
pub fn sidecar_candidates(site_rel: &str) -> Vec<String> {
    let Some(stem) = site_rel.strip_suffix(".html") else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(2);
    // `about/index.html` is compiled from `about.md` in the common
    // case, and from `about/index.md` when both spellings exist.
    if let Some(dir) = stem.strip_suffix("/index") {
        out.push(format!("{dir}.meta.json"));
    }
    out.push(format!("{stem}.meta.json"));
    out
}

/// Sentinel substring used for idempotency checks.
pub const HREFLANG_MARKER: &str = "rel=\"alternate\" hreflang=";

/// Rewrites existing ap-lang-item links in the page to point to the exact localized path
pub fn rewrite_ap_lang_items(
    html: &str,
    locale_map: &BTreeMap<String, String>,
    base: &str,
    strategy: &UrlPrefixStrategy,
    root_locale: Option<&str>,
) -> String {
    if !html.contains("ap-lang-item") {
        return html.to_string();
    }

    let mut result = String::with_capacity(html.len());
    let mut remaining = html;

    while let Some(start_idx) = remaining.find("<a ") {
        result.push_str(&remaining[..start_idx]);
        let tag_content = &remaining[start_idx..];

        let Some(end_idx) = tag_content.find('>') else {
            result.push_str(remaining);
            return result;
        };

        let tag_inner = &tag_content[..end_idx + 1];
        let mut rewritten_tag = tag_inner.to_string();

        if tag_inner.contains("ap-lang-item") {
            let mut data_lang = None;
            for quote in ['"', '\''] {
                let pattern = format!("data-lang={quote}");
                if let Some(pos) = tag_inner.find(&pattern) {
                    let val_start = pos + pattern.len();
                    if let Some(val_end) = tag_inner[val_start..].find(quote) {
                        data_lang = Some(
                            tag_inner[val_start..val_start + val_end]
                                .trim()
                                .to_string(),
                        );
                        break;
                    }
                }
            }

            if let Some(lang) = data_lang {
                if let Some(rel_path) = locale_map.get(&lang) {
                    let full_url =
                        build_url(base, &lang, rel_path, strategy, root_locale);
                    let new_href = if full_url.starts_with("http://")
                        || full_url.starts_with("https://")
                    {
                        let after_scheme =
                            full_url.split("://").nth(1).unwrap_or("");
                        if let Some(slash_idx) = after_scheme.find('/') {
                            after_scheme[slash_idx..].to_string()
                        } else {
                            "/".to_string()
                        }
                    } else {
                        full_url
                    };

                    for quote in ['"', '\''] {
                        let href_pattern = format!("href={quote}");
                        if let Some(pos) = tag_inner.find(&href_pattern) {
                            let val_start = pos + href_pattern.len();
                            if let Some(val_end) =
                                tag_inner[val_start..].find(quote)
                            {
                                let before = &rewritten_tag[..val_start];
                                let after =
                                    &rewritten_tag[val_start + val_end..];
                                rewritten_tag =
                                    format!("{before}{new_href}{after}");
                                break;
                            }
                        }
                    }
                }
            }
        }

        result.push_str(&rewritten_tag);
        remaining = &tag_content[end_idx + 1..];
    }

    result.push_str(remaining);
    result
}

/// Marker comment embedded in templates where the language switcher
/// should be injected. Kept invisible in single-locale sites.
///
/// Prefer the element form below. HTML minifiers strip comments, and
/// `html-generator` minifies some pages during generation — before any
/// plugin runs — so a comment marker on those pages is gone by the time
/// this plugin looks for it. That is not a hypothetical: it silently
/// removed the language switcher from every minified page.
pub const LANG_SWITCHER_MARKER: &str = "<!-- ssg:lang-switcher -->";

/// Attribute that marks an element as the language-switcher placeholder.
/// Survives minification, because a minifier may reformat an element but
/// will not delete it.
pub const LANG_SWITCHER_ATTR: &str = "data-ssg-lang-switcher";

/// Finds the placeholder element carrying [`LANG_SWITCHER_ATTR`] and
/// returns its byte range, including the closing tag.
///
/// Deliberately not a regex: this crate has no regex dependency, and the
/// shape being matched is a single empty element, not a grammar.
pub fn find_lang_switcher_element(html: &str) -> Option<(usize, usize)> {
    let attr_at = html.find(LANG_SWITCHER_ATTR)?;
    // Walk back to the '<' that opens this element.
    let start = html[..attr_at].rfind('<')?;
    let name_start = start + 1;
    let name_end = html[name_start..]
        .find(|c: char| !c.is_ascii_alphanumeric())
        .map(|i| name_start + i)?;
    let name = &html[name_start..name_end];
    if name.is_empty() {
        return None;
    }
    // The attribute must belong to this tag, not to a later one.
    let open_end = html[start..].find('>')? + start + 1;
    if attr_at > open_end {
        return None;
    }
    let close = format!("</{name}>");
    let close_at = html[open_end..].find(&close)? + open_end;
    // Only an *empty* placeholder is replaced; anything else is content.
    if !html[open_end..close_at].trim().is_empty() {
        return None;
    }
    Some((start, close_at + close.len()))
}

/// Build the hreflang `<link>` block for a single page.
///
/// `locale_map` gives each locale's OWN path for this logical page, so
/// translated slugs (`/about/` ↔ `/fr/a-propos/`) resolve correctly;
/// `labels` gives each locale's `hreflang` value — ssg builds it with
/// its own `hreflang_labels`, which reads the site config and so stays
/// outside this crate.
///
/// The `x-default` alternate is emitted only when the default locale
/// actually serves the page — pointing it at a URL that does not exist
/// is worse than omitting an optional signal.
pub fn build_hreflang_links(
    locale_map: &BTreeMap<String, String>,
    labels: &BTreeMap<String, String>,
    default_locale: &str,
    base: &str,
    strategy: &UrlPrefixStrategy,
    root_locale: Option<&str>,
) -> String {
    let mut links = String::new();

    for (locale, rel_path) in locale_map {
        let href = build_url(base, locale, rel_path, strategy, root_locale);
        let hreflang = labels.get(locale).unwrap_or(locale);
        links.push_str(&format!(
            "    <link rel=\"alternate\" hreflang=\"{hreflang}\" href=\"{href}\" />\n"
        ));
    }

    if let Some(default_rel) = locale_map.get(default_locale) {
        let default_href =
            build_url(base, default_locale, default_rel, strategy, root_locale);
        links.push_str(&format!(
            "    <link rel=\"alternate\" hreflang=\"x-default\" href=\"{default_href}\" />\n"
        ));
    }

    links
}

/// Construct a full URL for a given locale + relative path.
///
/// `root_locale`, when it names `locale`, suppresses the locale segment
/// entirely: the root-hosted locale is served from `{base}/{rel_path}`
/// under either strategy.
pub fn build_url(
    base: &str,
    locale: &str,
    rel_path: &str,
    strategy: &UrlPrefixStrategy,
    root_locale: Option<&str>,
) -> String {
    if root_locale == Some(locale) {
        return format!("{base}/{rel_path}");
    }
    match strategy {
        UrlPrefixStrategy::SubPath => {
            format!("{base}/{locale}/{rel_path}")
        }
        UrlPrefixStrategy::SubDomain => {
            // Replace scheme://host with scheme://locale.host
            if let Some(idx) = base.find("://") {
                let (scheme, rest) = base.split_at(idx + 3);
                format!("{scheme}{locale}.{rest}/{rel_path}")
            } else {
                // Fallback: treat as sub-path.
                format!("{base}/{locale}/{rel_path}")
            }
        }
    }
}

/// Parses an Accept-Language header value into a sorted list of locale
/// preferences (highest quality first).
///
/// Example: "fr-CH, fr;q=0.9, en;q=0.8, de;q=0.7, *;q=0.5"
/// Returns: `["fr-CH", "fr", "en", "de", "*"]`
///
/// # Examples
///
/// ```rust
/// use ssg_i18n::parse_accept_language;
///
/// let locales = parse_accept_language("fr;q=0.9, en");
/// assert_eq!(locales[0], "en");
/// assert_eq!(locales[1], "fr");
/// ```
#[must_use]
pub fn parse_accept_language(header: &str) -> Vec<String> {
    if header.trim().is_empty() {
        return Vec::new();
    }

    let mut entries: Vec<(String, f64)> = header
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let mut segments = part.splitn(2, ';');
            let locale = segments.next()?.trim().to_string();
            if locale.is_empty() {
                return None;
            }
            let quality = segments
                .next()
                .and_then(|q| {
                    let q = q.trim();
                    q.strip_prefix("q=")
                        .and_then(|v| v.trim().parse::<f64>().ok())
                })
                .unwrap_or(1.0);
            Some((locale, quality))
        })
        .collect();

    // Sort by quality descending; stable sort preserves order for equal quality.
    entries.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
    });

    entries.into_iter().map(|(locale, _)| locale).collect()
}

/// Given a list of preferred locales (from Accept-Language) and a list
/// of available locales (directories on disk), returns the best match.
///
/// Matching rules:
/// 1. Exact match (e.g., "fr-CH" matches "fr-CH")
/// 2. Prefix match (e.g., "fr-CH" matches "fr")
/// 3. Default locale fallback
///
/// # Examples
///
/// ```rust
/// use ssg_i18n::negotiate_locale;
///
/// let pref = vec!["fr-CH".to_string(), "en".to_string()];
/// let avail = vec!["en".to_string(), "fr".to_string()];
/// assert_eq!(negotiate_locale(&pref, &avail, "en"), "fr");
/// ```
#[must_use]
pub fn negotiate_locale(
    preferred: &[String],
    available: &[String],
    default_locale: &str,
) -> String {
    let available_lower: Vec<String> =
        available.iter().map(|l| l.to_lowercase()).collect();

    for pref in preferred {
        // Skip wildcard
        if pref == "*" {
            continue;
        }
        let pref_lower = pref.to_lowercase();

        // Exact match
        if let Some(idx) = available_lower.iter().position(|a| *a == pref_lower)
        {
            return available[idx].clone();
        }

        // Prefix match: preferred "fr-CH" matches available "fr"
        let prefix = pref_lower.split('-').next().unwrap_or(&pref_lower);
        if let Some(idx) = available_lower.iter().position(|a| *a == prefix) {
            return available[idx].clone();
        }
    }

    default_locale.to_string()
}

// ── Language switcher helper ─────────────────────────────────────────

/// Generates an HTML snippet for a language switcher navigation.
///
/// This is a pure function that can be called from any plugin or template
/// helper to produce a `<nav>` block with links to all locale variants
/// of the current page.
///
/// # Arguments
///
/// * `locales` — All available locales.
/// * `current_locale` — The locale of the page being rendered.
/// * `current_path` — The relative path of the page (e.g. `about/index.html`).
/// * `base_url` — The site base URL.
/// * `strategy` — How locale URLs are constructed.
///
/// # Example
///
/// ```rust
/// use ssg_i18n::{generate_lang_switcher_html, UrlPrefixStrategy};
///
/// let html = generate_lang_switcher_html(
///     &["en".into(), "fr".into(), "de".into()],
///     "en",
///     "about/index.html",
///     "https://example.com",
///     &UrlPrefixStrategy::SubPath,
/// );
/// assert!(html.contains("lang=\"fr\""));
/// ```
#[must_use]
pub fn generate_lang_switcher_html(
    locales: &[String],
    current_locale: &str,
    current_path: &str,
    base_url: &str,
    strategy: &UrlPrefixStrategy,
) -> String {
    // Every locale serves the same path — the pre-`translation_key`
    // assumption, kept for this public helper's callers.
    let locale_map: BTreeMap<String, String> = locales
        .iter()
        .map(|l| (l.clone(), current_path.to_string()))
        .collect();
    let labels: BTreeMap<String, String> =
        locales.iter().map(|l| (l.clone(), l.clone())).collect();
    generate_lang_switcher_html_with_self_lang(
        &locale_map,
        &labels,
        current_locale,
        base_url,
        strategy,
        None,
    )
}

/// Builds a switcher whose entries link to each locale's own path.
///
/// Like [`generate_lang_switcher_html`], but takes the translation
/// matrix row for the page, so each entry links to that locale's OWN
/// (possibly translated) path rather than the current path under a
/// different prefix.
///
/// `labels` supplies the `lang=`/`hreflang=` value for each locale —
/// resolved through `seo::lang::resolve_page_lang` (spec A5, plan §2
/// 1.5) so the switcher agrees with the page's other language sinks.
pub fn generate_lang_switcher_html_with_self_lang(
    locale_map: &BTreeMap<String, String>,
    labels: &BTreeMap<String, String>,
    current_locale: &str,
    base_url: &str,
    strategy: &UrlPrefixStrategy,
    root_locale: Option<&str>,
) -> String {
    let base = base_url.trim_end_matches('/');
    let mut html = String::from(
        "<nav class=\"lang-switcher\" aria-label=\"Language\">\n  <ul>\n",
    );

    for (locale, rel_path) in locale_map {
        let href = build_url(base, locale, rel_path, strategy, root_locale);
        let lang_attr = labels.get(locale).unwrap_or(locale);
        let aria = if locale == current_locale {
            " aria-current=\"page\""
        } else {
            ""
        };
        html.push_str(&format!(
            "    <li><a href=\"{href}\" lang=\"{lang_attr}\" hreflang=\"{lang_attr}\"{aria}>{locale}</a></li>\n"
        ));
    }

    html.push_str("  </ul>\n</nav>\n");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quality values order the result, and the wildcard is not a locale.
    #[test]
    fn accept_language_is_ordered_by_quality() {
        let got = parse_accept_language("en;q=0.5,fr-CA,de;q=0.8,*;q=0.1");
        assert_eq!(got.first().map(String::as_str), Some("fr-CA"));
        assert!(
            got.iter().position(|l| l == "de")
                < got.iter().position(|l| l == "en"),
            "q=0.8 outranks q=0.5: {got:?}"
        );
    }

    #[test]
    fn negotiation_falls_back_to_the_default() {
        let have = ["en".to_string(), "fr".to_string()];
        assert_eq!(negotiate_locale(&["de".to_string()], &have, "en"), "en");
        assert_eq!(negotiate_locale(&["fr".to_string()], &have, "en"), "fr");
    }

    /// `fr-CA` should reach a site that only publishes `fr`.
    #[test]
    fn negotiation_matches_a_region_against_its_base_language() {
        let have = ["en".to_string(), "fr".to_string()];
        assert_eq!(negotiate_locale(&["fr-CA".to_string()], &have, "en"), "fr");
    }

    #[test]
    fn sub_path_puts_the_locale_in_the_path() {
        let url = build_url(
            "https://example.com",
            "fr",
            "about/",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert!(url.contains("/fr/"), "{url}");
    }

    #[test]
    fn sub_domain_puts_the_locale_in_the_host() {
        let url = build_url(
            "https://example.com",
            "fr",
            "about/",
            &UrlPrefixStrategy::SubDomain,
            None,
        );
        assert!(url.contains("fr."), "{url}");
        assert!(!url.contains("/fr/"), "not also in the path: {url}");
    }

    /// The root locale is served unprefixed, so its URLs must not gain one.
    #[test]
    fn the_root_locale_keeps_a_bare_path() {
        let url = build_url(
            "https://example.com",
            "en",
            "about/",
            &UrlPrefixStrategy::SubPath,
            Some("en"),
        );
        assert!(!url.contains("/en/"), "{url}");
    }

    #[test]
    fn hreflang_links_cover_every_locale_and_x_default() {
        let mut map = BTreeMap::new();
        let _ = map.insert("en".to_string(), "about/".to_string());
        let _ = map.insert("fr".to_string(), "a-propos/".to_string());
        let links = build_hreflang_links(
            &map,
            &BTreeMap::new(),
            "en",
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
            None,
        );
        assert!(links.contains("hreflang=\"en\""), "{links}");
        assert!(links.contains("hreflang=\"fr\""), "{links}");
        assert!(links.contains("x-default"), "{links}");
    }

    #[test]
    fn the_switcher_lists_every_locale() {
        let locales =
            vec!["en".to_string(), "fr".to_string(), "de".to_string()];
        let html = generate_lang_switcher_html(
            &locales,
            "en",
            "about/",
            "https://example.com",
            &UrlPrefixStrategy::SubPath,
        );
        for locale in &locales {
            assert!(html.contains(locale.as_str()), "{locale} missing: {html}");
        }
    }

    #[test]
    fn a_document_without_the_marker_has_no_switcher_element() {
        assert!(find_lang_switcher_element("<p>no switcher here</p>").is_none());
    }

    #[test]
    fn the_attribute_locates_the_whole_placeholder_element() {
        let html = format!("<p>x</p><nav {LANG_SWITCHER_ATTR}></nav><p>y</p>");
        let (start, end) = find_lang_switcher_element(&html)
            .expect("an empty placeholder is replaceable");
        assert_eq!(
            &html[start..end],
            format!("<nav {LANG_SWITCHER_ATTR}></nav>"),
            "the span must cover the element and nothing else"
        );
    }

    /// A placeholder the author has filled in is content, not a slot:
    /// replacing it would destroy their markup.
    #[test]
    fn a_non_empty_placeholder_is_left_alone() {
        let html = format!("<nav {LANG_SWITCHER_ATTR}>hand-written</nav>");
        assert!(find_lang_switcher_element(&html).is_none(), "{html}");
    }

    #[test]
    fn sidecar_candidates_are_derived_from_the_page_path() {
        let got = sidecar_candidates("about/index.html");
        assert!(!got.is_empty(), "{got:?}");
        assert!(
            got.iter().any(|c| c.contains("about")),
            "the page's own stem is a candidate: {got:?}"
        );
    }
}
