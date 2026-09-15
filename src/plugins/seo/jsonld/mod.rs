// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! JSON-LD structured data injection plugin.

use super::helpers::{
    extract_date_from_html, extract_description, extract_first_content_image,
    extract_meta_author, extract_meta_date, extract_title,
};
use super::lang::resolve_page_lang;
use crate::audit::gates::util::find_tag_end;
use crate::error::SsgError;
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use std::path::Path;

pub mod iso20022;
pub use iso20022::{
    from_frontmatter as iso20022_from_frontmatter,
    log_first_use_pointer as iso20022_log_first_use, validate_bic,
    validate_iban, validate_schema_org as validate_iso20022_schema_org,
    warn_invalid_fields as iso20022_warn_invalid_fields, BankAccount,
    DispatchError as Iso20022DispatchError, FinancialProduct,
    FinancialTransaction, Iso20022Entity, MonetaryAmount, PaymentInstrument,
    RegulatedFinancialInstitution, SchemaOrgError as Iso20022SchemaOrgError,
    ValidationOutcome,
};

/// Configuration for the JSON-LD structured data plugin.
#[derive(Debug, Clone)]
pub struct JsonLdConfig {
    /// Base URL of the site (for absolute URLs in JSON-LD).
    pub base_url: String,
    /// Organization name for Organization schema.
    pub org_name: String,
    /// Whether to generate `BreadcrumbList` for every page.
    pub breadcrumbs: bool,
}

/// Injects JSON-LD structured data into HTML files.
///
/// Auto-detects schema.org types from page metadata:
/// - Pages with `<article>` → `Article`
/// - All other pages → `WebPage`
/// - `BreadcrumbList` derived from URL path (opt-in)
///
/// Idempotent: skips files that already contain `application/ld+json`.
#[derive(Debug, Clone)]
pub struct JsonLdPlugin {
    pub(crate) config: JsonLdConfig,
}

impl JsonLdPlugin {
    /// Creates a new `JsonLdPlugin` with the given configuration.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::seo::{JsonLdConfig, JsonLdPlugin};
    /// use ssg::plugin::Plugin;
    ///
    /// let cfg = JsonLdConfig {
    ///     base_url: "https://example.com".into(),
    ///     org_name: "Demo".into(),
    ///     breadcrumbs: true,
    /// };
    /// let p = JsonLdPlugin::new(cfg);
    /// assert_eq!(p.name(), "json-ld");
    /// ```
    #[must_use]
    pub const fn new(config: JsonLdConfig) -> Self {
        Self { config }
    }

    /// Creates a `JsonLdPlugin` from site config values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::seo::JsonLdPlugin;
    /// use ssg::plugin::Plugin;
    ///
    /// let p = JsonLdPlugin::from_site("https://example.com", "Demo");
    /// assert_eq!(p.name(), "json-ld");
    /// ```
    #[must_use]
    pub fn from_site(base_url: &str, site_name: &str) -> Self {
        Self {
            config: JsonLdConfig {
                base_url: base_url.to_string(),
                org_name: site_name.to_string(),
                breadcrumbs: true,
            },
        }
    }
}

/// Builds an Article JSON-LD object from page metadata.
///
/// `lang` is the already-resolved page language from
/// [`resolve_page_lang`] (spec A5, plan §2 1.5) — never empty, so no
/// inline fallback is needed at this sink.
fn build_article_jsonld(
    title: &str,
    description: &str,
    page_url: &str,
    org_name: &str,
    author_name: &str,
    image_url: &str,
    date_published: Option<&String>,
    date_modified: Option<&String>,
    lang: &str,
) -> serde_json::Value {
    let mut article = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "Article",
        "headline": title,
        "description": description,
        "url": page_url,
        // spec A5: language comes from the single page-language
        // resolver — the hard-coded "en" fallback that used to live
        // here was the bug's signature (plan §2 1.5).
        "inLanguage": lang,
        "mainEntityOfPage": {
            "@type": "WebPage",
            "@id": page_url
        },
        "publisher": {
            "@type": "Organization",
            "name": org_name
        }
    });

    if !author_name.is_empty() {
        article["author"] = serde_json::json!({
            "@type": "Person",
            "name": author_name
        });
    }

    if !image_url.is_empty() {
        article["image"] = serde_json::json!({
            "@type": "ImageObject",
            "url": image_url
        });
    }

    if let Some(dp) = date_published {
        article["datePublished"] = serde_json::json!(dp);
    }
    if let Some(dm) = date_modified {
        article["dateModified"] = serde_json::json!(dm);
    } else if let Some(dp) = date_published {
        article["dateModified"] = serde_json::json!(dp);
    }

    article
}

/// Builds a `WebPage` JSON-LD object from page metadata.
///
/// `lang` is the already-resolved page language from
/// [`resolve_page_lang`] (spec A5, plan §2 1.5) — never empty, so no
/// inline fallback is needed at this sink.
fn build_webpage_jsonld(
    title: &str,
    description: &str,
    page_url: &str,
    author_name: &str,
    image_url: &str,
    date_published: Option<&String>,
    lang: &str,
) -> serde_json::Value {
    let mut webpage = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "WebPage",
        "name": title,
        "description": description,
        "url": page_url,
        // spec A5: language comes from the single page-language
        // resolver — the hard-coded "en" fallback that used to live
        // here was the bug's signature (plan §2 1.5).
        "inLanguage": lang
    });

    if !author_name.is_empty() {
        webpage["author"] = serde_json::json!({
            "@type": "Person",
            "name": author_name
        });
    }

    if !image_url.is_empty() {
        webpage["image"] = serde_json::json!({
            "@type": "ImageObject",
            "url": image_url
        });
    }

    if let Some(dp) = date_published {
        webpage["datePublished"] = serde_json::json!(dp);
    }

    webpage
}

/// Builds a `BreadcrumbList` JSON-LD object from the URL path, if applicable.
fn build_breadcrumb_jsonld(
    base: &str,
    rel_path: &str,
) -> Option<serde_json::Value> {
    let parts: Vec<&str> = rel_path
        .trim_matches('/')
        .split('/')
        .filter(|p| !p.is_empty() && *p != "index.html")
        .collect();

    if parts.is_empty() {
        return None;
    }

    let mut items = vec![serde_json::json!({
        "@type": "ListItem",
        "position": 1,
        "name": "Home",
        "item": format!("{}/", base)
    })];

    let mut accumulated = String::new();
    for (i, part) in parts.iter().enumerate() {
        accumulated = format!("{accumulated}/{part}");
        let name = part.trim_end_matches(".html").replace('-', " ");
        items.push(serde_json::json!({
            "@type": "ListItem",
            "position": i + 2,
            "name": name,
            "item": format!("{}{}", base, accumulated)
        }));
    }

    Some(serde_json::json!({
        "@context": "https://schema.org",
        "@type": "BreadcrumbList",
        "itemListElement": items
    }))
}

/// Builds all JSON-LD scripts for a single page.
///
/// `lang` is the canonical page language from [`resolve_page_lang`]
/// (spec A5, plan §2 1.5), resolved once by the caller so every
/// emitted block agrees.
fn build_jsonld_scripts(
    html: &str,
    base: &str,
    rel_path: &str,
    org_name: &str,
    breadcrumbs: bool,
    lang: &str,
) -> Vec<serde_json::Value> {
    let title = extract_title(html);
    let description = extract_description(html, 160);
    let page_url = format!("{base}/{rel_path}");
    let author_name = extract_meta_author(html);
    let image_url = extract_first_content_image(html);
    let date_published = extract_date_from_html(html, "datePublished")
        .or_else(|| extract_meta_date(html));
    let date_modified = extract_date_from_html(html, "dateModified");

    let mut scripts = Vec::new();

    if html.contains("<article") {
        scripts.push(build_article_jsonld(
            &title,
            &description,
            &page_url,
            org_name,
            &author_name,
            &image_url,
            date_published.as_ref(),
            date_modified.as_ref(),
            lang,
        ));
    } else {
        scripts.push(build_webpage_jsonld(
            &title,
            &description,
            &page_url,
            &author_name,
            &image_url,
            date_published.as_ref(),
            lang,
        ));
    }

    if breadcrumbs {
        if let Some(breadcrumb) = build_breadcrumb_jsonld(base, rel_path) {
            scripts.push(breadcrumb);
        }
    }

    scripts
}

impl Plugin for JsonLdPlugin {
    fn name(&self) -> &'static str {
        "json-ld"
    }

    fn has_transform(&self) -> bool {
        true
    }

    fn transform_html(
        &self,
        html: &str,
        path: &Path,
        ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        if html.contains("application/ld+json") {
            return Ok(html.to_string());
        }

        let base = self.config.base_url.trim_end_matches('/');
        let site_dir = &ctx.site_dir;

        let rel_path = path
            .strip_prefix(site_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        // spec A5 (plan §2 1.5): resolve the page language once so
        // every JSON-LD block agrees with the other language sinks.
        let lang = resolve_page_lang(html, path, ctx);

        let mut scripts = build_jsonld_scripts(
            html,
            base,
            &rel_path,
            &self.config.org_name,
            self.config.breadcrumbs,
            &lang,
        );

        // ── ISO 20022 / banking extension (opt-in via frontmatter) ──
        //
        // AC4: When no `iso20022` key is present in the frontmatter
        // sidecar (or no sidecar exists at all), this branch contributes
        // ZERO bytes to `scripts` — preserving the v0.0.43 emission
        // byte-for-byte.
        if let Some(extra) = build_iso20022_scripts(path, ctx, &rel_path) {
            scripts.extend(extra);
        }

        let mut injection = String::new();
        for script in &scripts {
            let json = script_to_json(script, path)?;
            injection.push_str(&format!(
                "<script type=\"application/ld+json\">{json}</script>\n"
            ));
        }

        Ok(inject_before_head_close(html, &injection))
    }

    fn after_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
        Ok(())
    }
}

/// Serialises one JSON-LD script blob, mapping any serialisation
/// failure onto [`SsgError::Io`] keyed by the page path.
///
/// Generic over the serialisable type so the (structurally
/// unreachable for `serde_json::Value`) error arm stays testable.
fn script_to_json<T: serde::Serialize>(
    script: &T,
    path: &Path,
) -> Result<String, SsgError> {
    fail_point!("jsonld::script-to-json", |_| {
        Err(SsgError::io(
            std::io::Error::other("injected: jsonld::script-to-json"),
            path,
        ))
    });
    serde_json::to_string(script).map_err(|e| SsgError::io(e, path))
}

/// Resolves the `.meta.json` sidecar path for a built HTML file and
/// returns its `iso20022` block, if any. Returns `None` when there is
/// no sidecar, when it does not parse, or when it carries no
/// `iso20022` key — preserving AC4 (zero behavioural drift on pages
/// that don't opt in).
fn read_iso20022_block(
    path: &Path,
    ctx: &PluginContext,
    rel_path: &str,
) -> Option<serde_json::Value> {
    // Sidecar lookup is shared with the page-language resolver
    // (spec A5) — see `seo::lang::read_page_sidecar` for the
    // three-location resolution order.
    super::lang::read_page_sidecar(path, ctx, rel_path)?
        .get("iso20022")
        .cloned()
}

/// Builds ISO 20022 JSON-LD scripts for a page, given its sidecar
/// frontmatter. Returns `None` (NOT `Some(vec![])`) when the page
/// has not opted in — callers can extend the script list without
/// allocation overhead.
fn build_iso20022_scripts(
    path: &Path,
    ctx: &PluginContext,
    rel_path: &str,
) -> Option<Vec<serde_json::Value>> {
    let block = read_iso20022_block(path, ctx, rel_path)?;
    iso20022_log_first_use();

    let page_label = path.display().to_string();

    // The block can be either a single object or an array of objects —
    // sites with multiple transactions per page (statements, ledgers)
    // benefit from the array form.
    let blocks: Vec<serde_json::Value> = if let Some(arr) = block.as_array() {
        arr.clone()
    } else {
        vec![block]
    };

    let mut scripts = Vec::new();
    for entry in blocks {
        match iso20022_from_frontmatter(&entry) {
            Ok(entity) => {
                let _ = iso20022_warn_invalid_fields(&entity, &page_label);
                scripts.push(entity.to_jsonld());
            }
            Err(e) => {
                log::warn!(
                    "[json-ld/iso20022] {page_label}: skipping iso20022 \
                     block — {e}"
                );
            }
        }
    }

    if scripts.is_empty() {
        None
    } else {
        Some(scripts)
    }
}

// =====================================================================
// JSON-LD validation (resolves #467)
// =====================================================================

/// A single validation failure against a JSON-LD block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonLdValidationError {
    /// The schema.org `@type` of the block (or "Unknown" if absent).
    pub schema_type: String,
    /// Required field that was missing or had the wrong shape.
    pub field: String,
    /// Human-readable reason.
    pub reason: String,
}

impl std::fmt::Display for JsonLdValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] missing/invalid `{}` — {}",
            self.schema_type, self.field, self.reason
        )
    }
}

/// Walks an HTML string, extracts every `<script type="application/ld+json">`
/// block, parses it as JSON, and validates required fields per
/// schema.org `@type`.
///
/// Supported types (with their required-field guards):
///
/// - **`Article`** — `headline`, `datePublished`, `author`, `image`
/// - **`WebPage`** — `name` (Google rich-results requirement; `url`
///   and `inLanguage` are Recommended only and not flagged here)
/// - **`BreadcrumbList`** — `itemListElement` (non-empty array)
/// - **`FAQPage`** — `mainEntity` (non-empty array of `Question`)
/// - **`LocalBusiness`** — `name`, `address`
/// - **`Organization`** — `name`, `url`
///
/// Returns the empty vector if every block parses and passes its
/// required-field check. Unknown `@type` values are treated as
/// pass-through (no required fields enforced) so user-extended
/// schemas don't trigger false negatives.
///
/// # Examples
///
/// ```rust
/// use ssg::seo::validate_jsonld;
///
/// let html = r#"<script type="application/ld+json">
/// {"@type":"Article","headline":"x","datePublished":"2024","author":"y","image":"i"}
/// </script>"#;
/// assert!(validate_jsonld(html).is_empty());
/// ```
#[must_use]
pub fn validate_jsonld(html: &str) -> Vec<JsonLdValidationError> {
    let mut errors = Vec::new();

    for block in extract_jsonld_blocks(html) {
        match serde_json::from_str::<serde_json::Value>(&block) {
            Ok(value) => validate_one(&value, &mut errors),
            Err(parse_err) => {
                errors.push(JsonLdValidationError {
                    schema_type: "Unparseable".to_string(),
                    field: "(payload)".to_string(),
                    reason: format!("invalid JSON: {parse_err}"),
                });
            }
        }
    }

    errors
}

/// Returns the inner JSON of every `<script type="application/ld+json">`
/// block. Tolerant of attribute order and whitespace.
///
/// Resolves audit items #4 + #5:
/// - `type` is parsed as a discrete attribute value rather than
///   substring-matched, so `type="application/ld+json/extra"` no
///   longer falsely qualifies.
/// - The `</script>` close finder is JSON-string-aware: a literal
///   `</script>` *inside* a JSON string value (e.g.
///   `"description": "code: </script>"`) is correctly skipped over.
///   The HTML5 spec actually forbids `</script>` inside script
///   bodies even in strings — most authors escape as `<\/script>`
///   — but our extractor handles either form gracefully.
fn extract_jsonld_blocks(html: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let lower = html.to_lowercase();
    let mut cursor = 0;

    while let Some(rel_open) = lower[cursor..].find("<script") {
        let abs_open = cursor + rel_open;
        // Use find_tag_end equivalent: advance past `>` while
        // skipping any `>` characters that appear inside quoted
        // attribute values. Without this, `<script type="text/x>y">`
        // would close prematurely at the inner `>`.
        let tag_end = find_tag_end(&lower, abs_open);
        let tag = &lower[abs_open..tag_end];
        cursor = tag_end;

        if !is_jsonld_script_tag(tag) {
            continue;
        }

        let Some(close) = find_script_close_skipping_strings(&html[cursor..])
        else {
            break;
        };
        // Use the original-case slice for the JSON payload —
        // schema.org values are case-sensitive.
        blocks.push(html[cursor..cursor + close].trim().to_string());
        cursor += close + "</script>".len();
    }

    blocks
}

/// Returns `true` if the `<script ...>` tag declares
/// `type="application/ld+json"` exactly (any quoting; no
/// substring match).
fn is_jsonld_script_tag(tag: &str) -> bool {
    extract_attr(tag, "type")
        .is_some_and(|v| v.eq_ignore_ascii_case("application/ld+json"))
}

/// Extracts the value of an HTML attribute from an open-tag string.
/// Tolerant of quoting and whitespace. Returns `None` if the
/// attribute is absent or has no value.
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let needle = format!("{}=", name.to_lowercase());
    let idx = lower.find(&needle)?;
    // Make sure the match starts at a token boundary (preceding
    // char is whitespace or `<` or the very start of `tag`).
    let pre = lower.as_bytes().get(idx.wrapping_sub(1));
    let boundary_ok = idx == 0
        || matches!(pre, Some(b) if b.is_ascii_whitespace() || *b == b'<');
    if !boundary_ok {
        return None;
    }
    let rest = &tag[idx + needle.len()..];
    let trimmed = rest.trim_start();
    if let Some(s) = trimmed.strip_prefix('"') {
        s.find('"').map(|e| s[..e].to_string())
    } else if let Some(s) = trimmed.strip_prefix('\'') {
        s.find('\'').map(|e| s[..e].to_string())
    } else {
        let end = trimmed
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(trimmed.len());
        Some(trimmed[..end].to_string())
    }
}

/// Returns the byte offset of `</script>` in `body` while ignoring
/// occurrences that appear *inside* a JSON string literal.
///
/// The walker tracks two pieces of state: whether we're currently
/// inside a `"..."` string, and whether the previous byte was the
/// JSON escape character `\`. Scanning is done in bytes (UTF-8 is
/// not relevant for the ASCII-only delimiters we care about).
fn find_script_close_skipping_strings(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let needle = b"</script>";
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < bytes.len() {
        if in_string {
            if escape {
                escape = false;
            } else if bytes[i] == b'\\' {
                escape = true;
            } else if bytes[i] == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        // Case-insensitive check for `</script>`.
        if i + needle.len() <= bytes.len()
            && bytes[i..i + needle.len()].eq_ignore_ascii_case(needle)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Validates a single parsed JSON-LD value (object or array).
fn validate_one(
    value: &serde_json::Value,
    errors: &mut Vec<JsonLdValidationError>,
) {
    // schema.org allows top-level @graph arrays; descend into them.
    if let Some(graph) = value.get("@graph").and_then(|v| v.as_array()) {
        for entry in graph {
            validate_one(entry, errors);
        }
        return;
    }

    // Array at top level — validate each entry.
    if let Some(array) = value.as_array() {
        for entry in array {
            validate_one(entry, errors);
        }
        return;
    }

    let schema_type = value
        .get("@type")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    // Required-field sets aligned with Google's rich-results
    // requirements (https://developers.google.com/search/docs/appearance/structured-data),
    // not the broader schema.org vocabulary. schema.org marks many
    // useful fields as `Recommended` rather than `Required` — this
    // validator only fires on truly-missing fields the search
    // engines actually penalise.
    let required: &[&str] = match schema_type.as_str() {
        "Article" | "NewsArticle" | "BlogPosting" => {
            // Google requires headline + datePublished + author +
            // image for Article rich results.
            &["headline", "datePublished", "author", "image"]
        }
        // WebPage's only hard requirement is `name`. `url` and
        // `inLanguage` are Recommended but not penalised when
        // absent — auto-generated stub pages (taxonomy indexes,
        // 404, offline) routinely omit them.
        "WebPage" => &["name"],
        "BreadcrumbList" => &["itemListElement"],
        "FAQPage" => &["mainEntity"],
        "LocalBusiness" | "Restaurant" | "Store" => &["name", "address"],
        "Organization" => &["name", "url"],
        // Unknown types: don't enforce required fields. Users may ship
        // custom @types that are still valid schema.org extensions.
        _ => return,
    };

    for field in required {
        match value.get(*field) {
            None => errors.push(JsonLdValidationError {
                schema_type: schema_type.clone(),
                field: (*field).to_string(),
                reason: "field absent".to_string(),
            }),
            Some(serde_json::Value::Null) => {
                errors.push(JsonLdValidationError {
                    schema_type: schema_type.clone(),
                    field: (*field).to_string(),
                    reason: "field is null".to_string(),
                });
            }
            Some(serde_json::Value::String(s)) if s.trim().is_empty() => {
                errors.push(JsonLdValidationError {
                    schema_type: schema_type.clone(),
                    field: (*field).to_string(),
                    reason: "field is empty string".to_string(),
                });
            }
            Some(serde_json::Value::Array(a)) if a.is_empty() => {
                errors.push(JsonLdValidationError {
                    schema_type: schema_type.clone(),
                    field: (*field).to_string(),
                    reason: "array is empty".to_string(),
                });
            }
            _ => {}
        }
    }

    // BreadcrumbList: itemListElement entries should each be ListItem
    // with a `position` and `name`. Catch the most common regression.
    if schema_type == "BreadcrumbList" {
        if let Some(items) =
            value.get("itemListElement").and_then(|v| v.as_array())
        {
            for (idx, item) in items.iter().enumerate() {
                if item.get("position").is_none() {
                    errors.push(JsonLdValidationError {
                        schema_type: schema_type.clone(),
                        field: format!("itemListElement[{idx}].position"),
                        reason: "ListItem missing position".to_string(),
                    });
                }
                if item.get("name").is_none() && item.get("item").is_none() {
                    errors.push(JsonLdValidationError {
                        schema_type: schema_type.clone(),
                        field: format!("itemListElement[{idx}].name|item"),
                        reason: "ListItem missing name and item".to_string(),
                    });
                }
            }
        }
    }
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

    fn cfg() -> JsonLdConfig {
        JsonLdConfig {
            base_url: "https://example.com".to_string(),
            org_name: "Example Org".to_string(),
            breadcrumbs: true,
        }
    }

    #[test]
    fn name_is_stable() {
        let p = JsonLdPlugin::new(cfg());
        assert_eq!(p.name(), "json-ld");
    }

    #[test]
    fn from_site_constructs_with_breadcrumbs_enabled() {
        let p = JsonLdPlugin::from_site("https://x.example", "X");
        assert_eq!(p.config.base_url, "https://x.example");
        assert_eq!(p.config.org_name, "X");
        assert!(p.config.breadcrumbs);
    }

    // ── build_article_jsonld ───────────────────────────────────

    #[test]
    fn article_includes_author_when_provided() {
        let v = build_article_jsonld(
            "T",
            "D",
            "https://x/p",
            "Org",
            "Jane",
            "",
            None,
            None,
            "en",
        );
        assert_eq!(v["author"]["name"], "Jane");
        assert_eq!(v["author"]["@type"], "Person");
    }

    #[test]
    fn article_omits_author_when_empty() {
        let v = build_article_jsonld(
            "T",
            "D",
            "https://x/p",
            "Org",
            "",
            "",
            None,
            None,
            "en",
        );
        assert!(v.get("author").is_none());
    }

    #[test]
    fn article_includes_image_when_url_present() {
        let v = build_article_jsonld(
            "T",
            "D",
            "https://x/p",
            "Org",
            "",
            "https://x/img.png",
            None,
            None,
            "en",
        );
        assert_eq!(v["image"]["@type"], "ImageObject");
        assert_eq!(v["image"]["url"], "https://x/img.png");
    }

    #[test]
    fn article_uses_date_published_for_date_modified_fallback() {
        let dp = "2025-01-01".to_string();
        let v = build_article_jsonld(
            "T",
            "D",
            "https://x/p",
            "Org",
            "",
            "",
            Some(&dp),
            None,
            "en",
        );
        assert_eq!(v["datePublished"], "2025-01-01");
        assert_eq!(
            v["dateModified"], "2025-01-01",
            "missing dateModified should fall back to datePublished"
        );
    }

    #[test]
    fn article_keeps_distinct_date_modified() {
        let dp = "2025-01-01".to_string();
        let dm = "2025-06-15".to_string();
        let v = build_article_jsonld(
            "T",
            "D",
            "https://x/p",
            "Org",
            "",
            "",
            Some(&dp),
            Some(&dm),
            "en",
        );
        assert_eq!(v["datePublished"], "2025-01-01");
        assert_eq!(v["dateModified"], "2025-06-15");
    }

    #[test]
    fn article_emits_resolved_lang_verbatim() {
        // spec A5: the inline "en" fallback is gone — the caller
        // passes the resolver's output and this sink echoes it.
        let v = build_article_jsonld(
            "T",
            "D",
            "https://x/p",
            "Org",
            "",
            "",
            None,
            None,
            "hi",
        );
        assert_eq!(v["inLanguage"], "hi");
    }

    // ── build_webpage_jsonld ───────────────────────────────────

    #[test]
    fn webpage_includes_author_image_date_when_present() {
        let dp = "2025-01-01".to_string();
        let v = build_webpage_jsonld(
            "T",
            "D",
            "https://x/p",
            "Jane",
            "https://x/i.png",
            Some(&dp),
            "fr",
        );
        assert_eq!(v["@type"], "WebPage");
        assert_eq!(v["author"]["name"], "Jane");
        assert_eq!(v["image"]["url"], "https://x/i.png");
        assert_eq!(v["datePublished"], "2025-01-01");
        assert_eq!(v["inLanguage"], "fr");
    }

    #[test]
    fn webpage_omits_optional_fields_when_empty() {
        let v =
            build_webpage_jsonld("T", "D", "https://x/p", "", "", None, "en");
        assert!(v.get("author").is_none());
        assert!(v.get("image").is_none());
        assert!(v.get("datePublished").is_none());
        assert_eq!(v["inLanguage"], "en");
    }

    // ── build_breadcrumb_jsonld ────────────────────────────────

    #[test]
    fn breadcrumb_returns_none_for_root_path() {
        // Just `index.html` (or empty path) → no breadcrumb chain.
        assert!(build_breadcrumb_jsonld("https://x", "/").is_none());
        assert!(build_breadcrumb_jsonld("https://x", "index.html").is_none());
    }

    #[test]
    fn breadcrumb_builds_chain_for_nested_path() {
        let v = build_breadcrumb_jsonld("https://x", "blog/my-post/index.html")
            .expect("should produce breadcrumb for nested path");
        assert_eq!(v["@type"], "BreadcrumbList");
        let items = v["itemListElement"].as_array().unwrap();
        assert_eq!(items.len(), 3); // Home + blog + my-post
        assert_eq!(items[0]["name"], "Home");
        assert_eq!(items[1]["name"], "blog");
        assert_eq!(items[2]["name"], "my post"); // hyphens → spaces
    }

    #[test]
    fn breadcrumb_handles_html_extension_in_part_name() {
        let v = build_breadcrumb_jsonld("https://x", "page.html").unwrap();
        let items = v["itemListElement"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["name"], "page");
    }

    // ── build_jsonld_scripts ───────────────────────────────────

    #[test]
    fn build_scripts_picks_article_when_article_tag_present() {
        let html = r#"<html><head><title>Post</title></head>
            <body><article>content</article></body></html>"#;
        let scripts =
            build_jsonld_scripts(html, "https://x", "p/", "Org", false, "en");
        assert_eq!(scripts[0]["@type"], "Article");
    }

    #[test]
    fn build_scripts_picks_webpage_when_no_article_tag() {
        let html = "<html><head><title>P</title></head><body>x</body></html>";
        let scripts =
            build_jsonld_scripts(html, "https://x", "p/", "Org", false, "en");
        assert_eq!(scripts[0]["@type"], "WebPage");
    }

    #[test]
    fn build_scripts_includes_breadcrumb_when_enabled() {
        let html = "<html><head><title>P</title></head><body>x</body></html>";
        let scripts = build_jsonld_scripts(
            html,
            "https://x",
            "blog/post/",
            "Org",
            true,
            "en",
        );
        assert!(
            scripts.iter().any(|s| s["@type"] == "BreadcrumbList"),
            "breadcrumb should be present when enabled and path nested"
        );
    }

    #[test]
    fn build_scripts_skips_breadcrumb_when_disabled() {
        let html = "<html><head><title>P</title></head><body>x</body></html>";
        let scripts = build_jsonld_scripts(
            html,
            "https://x",
            "blog/post/",
            "Org",
            false,
            "en",
        );
        assert!(!scripts.iter().any(|s| s["@type"] == "BreadcrumbList"));
    }

    // ── after_compile end-to-end ───────────────────────────────

    #[test]
    fn after_compile_no_op_when_site_missing() {
        let dir = tempdir().unwrap();
        let nope = dir.path().join("nope");
        JsonLdPlugin::new(cfg()).after_compile(&ctx(&nope)).unwrap();
    }

    #[test]
    fn transform_html_injects_jsonld() {
        let dir = tempdir().unwrap();
        let c = ctx(dir.path());
        let html = "<html><head><title>X</title></head><body>x</body></html>";
        let page_path = dir.path().join("index.html");
        let after = JsonLdPlugin::new(cfg())
            .transform_html(html, &page_path, &c)
            .unwrap();
        assert!(after.contains("application/ld+json"));
        assert!(after.contains("\"@type\":\"WebPage\""));
    }

    #[test]
    fn transform_html_skips_existing_jsonld() {
        let dir = tempdir().unwrap();
        let c = ctx(dir.path());
        let html = r#"<html><head><script type="application/ld+json">{"@type":"X"}</script><title>X</title></head></html>"#;
        let page_path = dir.path().join("p.html");
        let after = JsonLdPlugin::new(cfg())
            .transform_html(html, &page_path, &c)
            .unwrap();
        // Only one JSON-LD block — no duplicate injected.
        assert_eq!(after.matches("application/ld+json").count(), 1);
        assert!(after.contains(r#"{"@type":"X"}"#));
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn transform_html_skips_without_head_tag() {
        let dir = tempdir().unwrap();
        let c = ctx(dir.path());
        let raw = "<!doctype html><html><body>only</body></html>";
        let page_path = dir.path().join("frag.html");
        let after = JsonLdPlugin::new(cfg())
            .transform_html(raw, &page_path, &c)
            .unwrap();
        assert_eq!(after, raw);
    }

    // ── inLanguage via resolve_page_lang (spec A5, plan §2 1.5) ────

    /// Context with a site `language` and declared `[i18n]` locales,
    /// rooted so that sidecars under `<dir>/build/.meta` are found.
    #[cfg(feature = "i18n")]
    fn locale_ctx(
        dir: &Path,
        language: &str,
        locales: &[&str],
    ) -> PluginContext {
        let mut c = PluginContext::new(
            Path::new("content"),
            &dir.join("build"),
            &dir.join("site"),
            Path::new("templates"),
        );
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

    /// Extracts `inLanguage` from the first injected JSON-LD block.
    fn injected_in_language(html: &str) -> String {
        let block = extract_jsonld_blocks(html)
            .into_iter()
            .next()
            .expect("page should carry an injected JSON-LD block");
        let v: serde_json::Value = serde_json::from_str(&block).unwrap();
        v["inLanguage"].as_str().unwrap_or_default().to_string()
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn in_language_is_path_driven_on_locale_pages() {
        // The A5 signature bug: a /hi/… page whose template carries
        // the site-wide lang="en-GB" must emit inLanguage=hi.
        let dir = tempdir().unwrap();
        let c = locale_ctx(dir.path(), "en-GB", &["en", "hi", "fr"]);
        let html = r#"<html lang="en-GB"><head><title>नमस्ते</title></head><body>x</body></html>"#;
        let page = dir.path().join("site/hi/2026-06-01-post/index.html");
        let out = JsonLdPlugin::new(cfg())
            .transform_html(html, &page, &c)
            .unwrap();
        assert_eq!(injected_in_language(&out), "hi");
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn in_language_is_frontmatter_driven_when_sidecar_declares_language() {
        let dir = tempdir().unwrap();
        let c = locale_ctx(dir.path(), "en-GB", &["en", "hi"]);
        let sidecar = dir.path().join("build/.meta/hi/post/index.meta.json");
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(sidecar, r#"{"language":"fr"}"#).unwrap();

        let html = r#"<html lang="en-GB"><head><title>T</title></head><body>x</body></html>"#;
        let page = dir.path().join("site/hi/post/index.html");
        let out = JsonLdPlugin::new(cfg())
            .transform_html(html, &page, &c)
            .unwrap();
        assert_eq!(
            injected_in_language(&out),
            "fr",
            "front-matter `language` outranks the locale path prefix"
        );
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn in_language_is_default_driven_on_default_locale_pages() {
        // en-GB site, page outside any locale prefix, template lang
        // matches the site default: all sources agree on en-GB.
        let dir = tempdir().unwrap();
        let c = locale_ctx(dir.path(), "en-GB", &["en"]);
        let html = r#"<html lang="en-GB"><head><title>T</title></head><body>x</body></html>"#;
        let page = dir.path().join("site/about/index.html");
        let out = JsonLdPlugin::new(cfg())
            .transform_html(html, &page, &c)
            .unwrap();
        assert_eq!(injected_in_language(&out), "en-GB");
    }

    #[test]
    fn in_language_en_fallback_only_when_nothing_resolves() {
        // No config, no sidecar, no locale prefix, no <html lang>:
        // only then does the resolver's final "en" constant fire.
        let dir = tempdir().unwrap();
        let c = ctx(&dir.path().join("site"));
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let page = dir.path().join("site/index.html");
        let out = JsonLdPlugin::new(cfg())
            .transform_html(html, &page, &c)
            .unwrap();
        assert_eq!(injected_in_language(&out), "en");
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn in_language_validation_passes_on_locale_fixtures() {
        // Fixture pages for en, fr, hi and en-GB must build with zero
        // JSON-LD validation findings (spec A5 acceptance).
        let dir = tempdir().unwrap();
        let c = locale_ctx(dir.path(), "en-GB", &["en", "fr", "hi"]);
        for (rel, want) in [
            ("en/page/index.html", "en"),
            ("fr/page/index.html", "fr"),
            ("hi/page/index.html", "hi"),
            ("page/index.html", "en-GB"),
        ] {
            let html = r#"<html lang="en-GB"><head><title>T</title></head><body>x</body></html>"#;
            let page = dir.path().join("site").join(rel);
            let out = JsonLdPlugin::new(cfg())
                .transform_html(html, &page, &c)
                .unwrap();
            assert_eq!(injected_in_language(&out), want, "page {rel}");
            assert!(
                validate_jsonld(&out).is_empty(),
                "page {rel} must emit zero JSON-LD validation findings"
            );
        }
    }

    // ── JSON-LD validation (issue #467) ────────────────────────────

    #[test]
    fn validate_extracts_block() {
        let html = r#"<html><head>
            <script type="application/ld+json">
            {"@context":"https://schema.org","@type":"WebPage",
             "name":"Hi","url":"https://x.test/","inLanguage":"en"}
            </script></head><body></body></html>"#;
        assert!(validate_jsonld(html).is_empty());
    }

    #[test]
    fn validate_flags_missing_required_field_on_article() {
        let html = r#"<script type="application/ld+json">
            {"@context":"https://schema.org","@type":"Article",
             "headline":"H","datePublished":"2026-05-10","author":"A"}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(
            errs.iter()
                .any(|e| e.schema_type == "Article" && e.field == "image"),
            "expected Article.image violation, got {errs:?}"
        );
    }

    #[test]
    fn validate_flags_empty_breadcrumb_list() {
        let html = r#"<script type="application/ld+json">
            {"@context":"https://schema.org","@type":"BreadcrumbList",
             "itemListElement":[]}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(
            errs.iter().any(|e| e.field == "itemListElement"),
            "expected itemListElement empty-array error, got {errs:?}"
        );
    }

    #[test]
    fn validate_breadcrumb_listitem_missing_position() {
        let html = r#"<script type="application/ld+json">
            {"@type":"BreadcrumbList",
             "itemListElement":[{"name":"Home","item":"https://x/"}]}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(
            errs.iter()
                .any(|e| e.field == "itemListElement[0].position"),
            "expected position-missing error, got {errs:?}"
        );
    }

    #[test]
    fn validate_unparseable_json() {
        let html = r#"<script type="application/ld+json">{not json}</script>"#;
        let errs = validate_jsonld(html);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].schema_type, "Unparseable");
    }

    #[test]
    fn validate_descends_into_graph() {
        // Article inside @graph missing required fields exercises the
        // descent path. Article has 4 required fields; this provides 1.
        let html = r#"<script type="application/ld+json">
            {"@context":"https://schema.org","@graph":[
                {"@type":"Article","headline":"H"}
            ]}
        </script>"#;
        let errs = validate_jsonld(html);
        // Article requires headline + datePublished + author + image;
        // we only provided headline, so the other 3 fire.
        assert!(errs
            .iter()
            .any(|e| e.schema_type == "Article" && e.field == "datePublished"));
        assert!(errs
            .iter()
            .any(|e| e.schema_type == "Article" && e.field == "author"));
        assert!(errs
            .iter()
            .any(|e| e.schema_type == "Article" && e.field == "image"));
    }

    #[test]
    fn validate_unknown_type_passes_through() {
        let html = r#"<script type="application/ld+json">
            {"@type":"CustomThing","foo":"bar"}
        </script>"#;
        assert!(validate_jsonld(html).is_empty());
    }

    #[test]
    fn validate_handles_multiple_blocks() {
        let html = r#"
            <script type="application/ld+json">{"@type":"Organization","name":"O","url":"https://o/"}</script>
            <script type="application/ld+json">{"@type":"Article","headline":"H"}</script>
        "#;
        let errs = validate_jsonld(html);
        // Org passes; Article missing 3 of 4 required.
        assert_eq!(
            errs.iter()
                .filter(|e| e.schema_type == "Organization")
                .count(),
            0
        );
        assert!(
            errs.iter().filter(|e| e.schema_type == "Article").count() >= 3
        );
    }

    // ── Strict type-attribute parsing (audit fix item #4) ──────────

    #[test]
    fn validate_skips_extra_qualified_type() {
        // `application/ld+json/extra` must NOT be treated as JSON-LD.
        // Pre-fix: `tag.contains("application/ld+json")` falsely
        // matched this.
        let html = r#"<script type="application/ld+json/extra">
            {"@type":"Article"}
        </script>"#;
        assert!(
            validate_jsonld(html).is_empty(),
            "non-JSON-LD type must not be validated"
        );
    }

    #[test]
    fn validate_recognises_type_with_single_quotes() {
        let html = r#"<script type='application/ld+json'>
            {"@type":"Organization","name":"O","url":"https://o/"}
        </script>"#;
        assert!(validate_jsonld(html).is_empty());
    }

    #[test]
    fn validate_recognises_type_after_other_attrs() {
        let html = r#"<script id="ld1" type="application/ld+json">
            {"@type":"Organization","name":"O","url":"https://o/"}
        </script>"#;
        assert!(validate_jsonld(html).is_empty());
    }

    // ── String-literal-aware </script> finder (audit fix item #5) ──

    #[test]
    fn validate_handles_close_script_inside_json_string() {
        // The old extractor truncated at the first `</script>` inside
        // a string value, producing parse-failure noise. The fixed
        // extractor only honours `</script>` outside JSON strings.
        let html = r#"<script type="application/ld+json">
            {"@type":"Article",
             "headline":"H","datePublished":"2026-01-01",
             "author":"A","image":"https://x/i.png",
             "description":"this contains a </script> inside the string and is still valid JSON"}
        </script>"#;
        let errs = validate_jsonld(html);
        // Article has all 4 required fields. The pre-fix bug would
        // have produced an Unparseable error because the extractor
        // would close at the inner `</script>`, leaving truncated
        // JSON.
        assert!(errs.is_empty(), "no errors expected, got {errs:?}");
    }

    #[test]
    fn extract_attr_returns_none_when_attribute_absent() {
        assert_eq!(extract_attr("<script src=x>", "type"), None);
    }

    #[test]
    fn extract_attr_handles_double_quoted_value() {
        assert_eq!(
            extract_attr(r#"<script type="application/ld+json">"#, "type"),
            Some("application/ld+json".to_string())
        );
    }

    #[test]
    fn extract_attr_rejects_substring_match_in_other_attribute() {
        // `data-mytype="foo"` must NOT match a `type=` query.
        assert_eq!(extract_attr(r#"<script data-mytype="foo">"#, "type"), None);
    }

    #[test]
    fn extract_attr_quoting_and_boundaries() {
        assert_eq!(
            extract_attr("<script type=\"foo\"", "type"),
            Some("foo".to_string())
        );
        assert_eq!(
            extract_attr("<script type='bar'", "type"),
            Some("bar".to_string())
        );
        assert_eq!(
            extract_attr("<script type=baz", "type"),
            Some("baz".to_string())
        );
        // Missing close quote
        assert_eq!(extract_attr("<script type=\"foo", "type"), None);
        assert_eq!(extract_attr("<script type='bar", "type"), None);
        // Not a boundary
        assert_eq!(extract_attr("<script subtype=\"foo\"", "type"), None);
    }

    #[test]
    fn test_find_script_close_escaped_quotes() {
        let body = r#"{"msg":"escaped \" quote"}</script>"#;
        assert_eq!(find_script_close_skipping_strings(body), Some(26));
    }

    // ── Display + extractor edge shapes ─────────────────────────────

    #[test]
    fn validation_error_display_includes_all_fields() {
        let e = JsonLdValidationError {
            schema_type: "Article".to_string(),
            field: "headline".to_string(),
            reason: "field absent".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("[Article]"));
        assert!(s.contains("`headline`"));
        assert!(s.contains("field absent"));
    }

    #[test]
    fn validation_error_partial_eq_covers_equal_and_unequal_tail_field() {
        // `JsonLdValidationError` derives `PartialEq`/`Eq`, but the rest
        // of this test module only ever inspects individual fields
        // (`e.field == "..."`), never the struct as a whole — so the
        // derived `eq` was otherwise never exercised. Compare on a
        // difference in the *last* field so the generated `&&` chain
        // runs past the first two comparisons too.
        let a = JsonLdValidationError {
            schema_type: "Article".to_string(),
            field: "headline".to_string(),
            reason: "field absent".to_string(),
        };
        let b = a.clone();
        let mut c = a.clone();
        c.reason = "field is null".to_string();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn extractor_ignores_unterminated_jsonld_script() {
        // No closing </script> — the extractor must bail without
        // yielding a truncated block.
        let html = r#"<script type="application/ld+json">{"@type":"WebPage""#;
        assert!(extract_jsonld_blocks(html).is_empty());
    }

    #[test]
    fn find_html_tag_end_without_closing_bracket_returns_len() {
        let html = "<script type=\"application/ld+json\"";
        assert_eq!(find_tag_end(html, 0), html.len());
    }

    // ── validate_one: remaining shapes ──────────────────────────────

    #[test]
    fn validator_descends_into_top_level_array() {
        let html = r#"<script type="application/ld+json">
            [{"@type":"WebPage","name":"A"},{"@type":"WebPage"}]
        </script>"#;
        let errs = validate_jsonld(html);
        assert_eq!(errs.len(), 1, "only the second entry is invalid: {errs:?}");
        assert_eq!(errs[0].field, "name");
    }

    #[test]
    fn validator_flags_faq_page_missing_main_entity() {
        let html = r#"<script type="application/ld+json">
            {"@type":"FAQPage"}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(errs.iter().any(|e| e.field == "mainEntity"), "{errs:?}");
    }

    #[test]
    fn validator_checks_restaurant_and_store_literals() {
        let html = r#"<script type="application/ld+json">
            {"@type":"Restaurant","name":"R"}
        </script>
        <script type="application/ld+json">
            {"@type":"Store","address":"1 Main St"}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(
            errs.iter()
                .any(|e| e.schema_type == "Restaurant" && e.field == "address"),
            "{errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| e.schema_type == "Store" && e.field == "name"),
            "{errs:?}"
        );
    }

    #[test]
    fn validator_flags_null_required_field() {
        let html = r#"<script type="application/ld+json">
            {"@type":"WebPage","name":null}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(errs.iter().any(|e| e.reason == "field is null"), "{errs:?}");
    }

    #[test]
    fn validator_flags_whitespace_only_required_field() {
        let html = r#"<script type="application/ld+json">
            {"@type":"WebPage","name":"   "}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(
            errs.iter().any(|e| e.reason == "field is empty string"),
            "{errs:?}"
        );
    }

    #[test]
    fn validator_flags_list_item_missing_name_and_item() {
        let html = r#"<script type="application/ld+json">
            {"@type":"BreadcrumbList","itemListElement":[{"position":1}]}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(
            errs.iter()
                .any(|e| e.field == "itemListElement[0].name|item"),
            "{errs:?}"
        );
    }

    #[test]
    fn validator_tolerates_non_array_item_list_element() {
        // `itemListElement` present but not an array — the per-item
        // ListItem walk is skipped without panicking.
        let html = r#"<script type="application/ld+json">
            {"@type":"BreadcrumbList","itemListElement":"oops"}
        </script>"#;
        let errs = validate_jsonld(html);
        assert!(
            !errs.iter().any(|e| e.field.starts_with("itemListElement[")),
            "{errs:?}"
        );
    }

    // ── ISO 20022 sidecar opt-in (AC4) ──────────────────────────────

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

    /// Context rooted in `dir` so `<dir>/build/.meta` sidecars are
    /// found for pages under `<dir>/site`.
    fn rooted_ctx(dir: &Path) -> PluginContext {
        PluginContext::new(
            Path::new("content"),
            &dir.join("build"),
            &dir.join("site"),
            Path::new("templates"),
        )
    }

    #[test]
    fn iso20022_object_block_is_injected() {
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "acct/index.html",
            r#"{"iso20022":{"type":"BankAccount","iban":"GB29NWBK60161331926819"}}"#,
        );
        let c = rooted_ctx(dir.path());
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let page = dir.path().join("site/acct/index.html");
        let out = JsonLdPlugin::new(cfg())
            .transform_html(html, &page, &c)
            .unwrap();
        assert!(
            out.contains(r#""@type":"BankAccount""#),
            "BankAccount block should be injected: {out}"
        );
        assert!(out.contains("GB29NWBK60161331926819"));
    }

    #[test]
    fn iso20022_array_block_skips_invalid_entries() {
        // Array form: one valid entity plus one entry without a type —
        // the invalid entry is skipped with a warning, the valid one
        // is still emitted.
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "mix/index.html",
            r#"{"iso20022":[
                {"type":"PaymentInstrument","instrument_type":"card"},
                {"no":"type"}
            ]}"#,
        );
        let c = rooted_ctx(dir.path());
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let page = dir.path().join("site/mix/index.html");
        let out = JsonLdPlugin::new(cfg())
            .transform_html(html, &page, &c)
            .unwrap();
        assert!(
            out.contains(r#""@type":"PaymentService""#),
            "valid array entry should be injected: {out}"
        );
    }

    #[test]
    fn iso20022_all_invalid_entries_injects_nothing_extra() {
        // Every entry fails dispatch → the extension contributes zero
        // scripts (None, not Some(vec![])).
        let dir = tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "bad/index.html",
            r#"{"iso20022":{"type":"NotAThing"}}"#,
        );
        let c = rooted_ctx(dir.path());
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let page = dir.path().join("site/bad/index.html");
        let out = JsonLdPlugin::new(cfg())
            .transform_html(html, &page, &c)
            .unwrap();
        assert!(
            !out.contains("iso20022"),
            "invalid block must contribute nothing: {out}"
        );
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn locale_ctx_with_empty_locale_set_defaults_to_en() {
        // Zero declared locales: the helper's default-locale fallback
        // kicks in and no path prefix can qualify as a locale.
        let dir = tempdir().unwrap();
        let c = locale_ctx(dir.path(), "en", &[]);
        let i18n = c.config.as_ref().unwrap().i18n.as_ref().unwrap();
        assert_eq!(i18n.default_locale, "en");
        assert!(i18n.locales.is_empty());
    }

    #[test]
    fn script_to_json_maps_serde_failure_to_io_error() {
        // JSON object keys must be strings — a tuple-keyed map fails.
        let bad: std::collections::BTreeMap<(u8, u8), u8> =
            std::iter::once(((1, 2), 3)).collect();
        let err = script_to_json(&bad, Path::new("page.html"))
            .expect_err("non-string map keys must fail serialisation");
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == Path::new("page.html"))
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
    fn transform_html_propagates_script_serialisation_failure() {
        let _guard = FailGuard("jsonld::script-to-json");
        fail::cfg("jsonld::script-to-json", "return").unwrap();

        let dir = tempdir().unwrap();
        let c = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            dir.path(),
            Path::new("templates"),
        );
        let plugin = JsonLdPlugin::new(JsonLdConfig {
            base_url: "https://example.com".to_string(),
            org_name: "Org".to_string(),
            breadcrumbs: false,
        });
        let html = "<html><head><title>T</title></head><body>x</body></html>";
        let err = plugin
            .transform_html(html, Path::new("page.html"), &c)
            .expect_err("failpoint must abort script serialisation");
        assert!(
            err.to_string().contains("jsonld::script-to-json"),
            "injected error should surface: {err}"
        );
    }
}
