#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![forbid(unsafe_code)]
// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! # ssg-core — Platform-independent SSG compilation pipeline
//!
//! This crate contains the pure-logic core of SSG, with no system
//! dependencies (`rayon`, `http-handle`). It compiles to
//! `wasm32-wasi` and `wasm32-unknown-unknown` (via `wasm-bindgen`).
//!
//! ## Features
//!
//! - Markdown → HTML compilation (pulldown-cmark with GFM extensions)
//! - Frontmatter parsing (TOML/JSON/YAML)
//! - Template rendering (when `minijinja` is enabled)
//! - Shortcode expansion
//! - SEO metadata generation
//! - Search index generation

pub mod content_provider;
pub mod isr_manifest;

pub use content_provider::{
    ContentProvider, FsContentProvider, MemoryContentProvider, ProviderError,
    ProviderResult,
};
pub use isr_manifest::{
    build_entry, hash_sources, CachePolicy, Manifest, ManifestEntry,
    DEFAULT_SWR, DEFAULT_S_MAXAGE, MANIFEST_VERSION,
};

use std::collections::HashMap;
use std::fmt;

/// The error type for ssg-core operations.
///
/// # Examples
///
/// ```
/// use ssg_core::Error;
///
/// let err = Error::InvalidSlug { input: "@@@".to_string() };
/// assert!(err.to_string().contains("Invalid slug input"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// TOML/YAML/JSON parsing failures.
    FrontmatterParse {
        /// The syntax format (e.g. "toml", "yaml", "json") or parse error detail.
        syntax: String,
    },
    /// Markdown rendering bugs.
    MarkdownCompile {
        /// Detail about what failed.
        source: String,
    },
    /// Slugification layout validation failures.
    InvalidSlug {
        /// The invalid input string.
        input: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrontmatterParse { syntax } => {
                write!(f, "Frontmatter parse error: {syntax}")
            }
            Self::MarkdownCompile { source } => {
                write!(f, "Markdown compilation error: {source}")
            }
            Self::InvalidSlug { input } => {
                write!(f, "Invalid slug input: {input}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Specialized Result type for ssg-core operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Compile a Markdown string to HTML.
///
/// Supports GitHub Flavored Markdown: tables, strikethrough, task lists.
///
/// # Example
///
/// ```
/// let html = ssg_core::compile_markdown("# Hello\n\nWorld");
/// assert!(html.contains("<h1>Hello</h1>"));
/// assert!(html.contains("<p>World</p>"));
/// ```
#[must_use]
pub fn compile_markdown(input: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;

    let parser = Parser::new_ext(input, options);
    let mut html_output = String::with_capacity(input.len() * 2);
    html::push_html(&mut html_output, parser);
    html_output
}

/// Parse frontmatter from a Markdown file.
///
/// Supports TOML (`+++`), YAML (`---`), and JSON (`{`) delimiters.
/// Returns `(frontmatter_map, body_without_frontmatter)`.
///
/// # Example
///
/// ```
/// let input = "---\ntitle: Hello\n---\n# Body";
/// let (fm, body) = ssg_core::parse_frontmatter(input);
/// assert_eq!(fm.get("title").and_then(|v| v.as_str()), Some("Hello"));
/// assert!(body.contains("# Body"));
/// ```
pub fn parse_frontmatter(
    input: &str,
) -> (HashMap<String, serde_json::Value>, String) {
    // Zero-copy core (issue #578, plan §4 3.1): the body is sliced out
    // of `input` exactly once and materialised exactly once here — no
    // per-branch `to_string()` and no metadata-map clone rebuilds.
    let (map, body) = parse_frontmatter_borrowed(input);
    (map, body.to_string())
}

/// Borrowed-body core of [`parse_frontmatter`].
///
/// Returns the metadata map by *moving* parsed entries (never cloning
/// them) and the body as a slice of `input`, leaving the single owned
/// materialisation to the public wrapper (issue #578, plan §4 3.1).
fn parse_frontmatter_borrowed(
    input: &str,
) -> (HashMap<String, serde_json::Value>, &str) {
    let trimmed = input.trim_start();

    // TOML frontmatter: +++...+++
    if let Some(after) = trimmed.strip_prefix("+++") {
        if let Some(end) = after.find("+++") {
            let fm_str = &after[..end];
            let body = &after[end + 3..];
            if let Ok(serde_json::Value::Object(map)) =
                toml::from_str::<serde_json::Value>(fm_str)
            {
                // Move the parsed entries into the final map — the
                // previous `(k.clone(), v.clone())` rebuild is gone.
                return (map.into_iter().collect(), body);
            }
            return (HashMap::new(), body);
        }
    }

    // YAML frontmatter: ---...---
    if let Some(after) = trimmed.strip_prefix("---") {
        if let Some(end) = after.find("---") {
            let fm_str = &after[..end];
            let body = &after[end + 3..];
            match noyalib::from_str::<serde_json::Value>(fm_str) {
                Ok(serde_json::Value::Object(map)) => {
                    return (map.into_iter().collect(), body);
                }
                Ok(_) => {
                    // Top-level non-mapping (e.g. a bare list or scalar)
                    // — preserve the body but emit no globals.
                    return (HashMap::new(), body);
                }
                Err(e) => {
                    log::warn!("YAML frontmatter parse error: {e}");
                    return (HashMap::new(), body);
                }
            }
        }
    }

    // JSON frontmatter: {...}
    if trimmed.starts_with('{') {
        // Find matching closing brace
        let mut depth = 0;
        let mut end = None;
        for (i, c) in trimmed.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(end_pos) = end {
            let fm_str = &trimmed[..end_pos];
            let body = &trimmed[end_pos..];
            if let Ok(map) = serde_json::from_str::<
                HashMap<String, serde_json::Value>,
            >(fm_str)
            {
                return (map, body);
            }
        }
    }

    (HashMap::new(), input)
}

/// Compile a complete page: parse frontmatter, render Markdown to HTML.
///
/// Returns `(frontmatter, html_body)`.
///
/// # Errors
/// Currently infallible — returns `Ok` for every input. The `Result`
/// signature is preserved so that future stricter validation can
/// surface failures without a breaking API change.
///
/// # Examples
///
/// ```
/// let input = "---\ntitle: Test\n---\n# Heading";
/// let (fm, html) = ssg_core::compile_page(input).unwrap();
/// assert_eq!(fm.get("title").and_then(|v| v.as_str()), Some("Test"));
/// assert!(html.contains("<h1>Heading</h1>"));
/// ```
pub fn compile_page(
    input: &str,
) -> Result<(HashMap<String, serde_json::Value>, String)> {
    let (frontmatter, body) = parse_frontmatter(input);
    let html = compile_markdown(&body);
    Ok((frontmatter, html))
}

/// Generate a search index entry from HTML content.
///
/// # Examples
///
/// ```
/// let entry = ssg_core::SearchEntry {
///     title: "Hi".to_string(),
///     url: "/".to_string(),
///     content: "hello".to_string(),
/// };
/// let json = serde_json::to_string(&entry).unwrap();
/// assert!(json.contains("\"title\":\"Hi\""));
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchEntry {
    /// Page title.
    pub title: String,
    /// Page URL.
    pub url: String,
    /// Plain text content for search matching.
    pub content: String,
}

/// Strip HTML tags from a string (simple implementation).
///
/// # Examples
///
/// ```
/// let plain = ssg_core::strip_html_tags("<p>Hello <b>world</b></p>");
/// assert_eq!(plain, "Hello world");
/// ```
#[must_use]
pub fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;

    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }

    result
}

/// Build a search index entry from HTML content and metadata.
///
/// # Examples
///
/// ```
/// let entry = ssg_core::build_search_entry(
///     "Welcome",
///     "/index.html",
///     "<p>Hello <b>world</b></p>",
/// );
/// assert_eq!(entry.title, "Welcome");
/// assert_eq!(entry.url, "/index.html");
/// assert_eq!(entry.content, "Hello world");
/// ```
#[must_use]
pub fn build_search_entry(title: &str, url: &str, html: &str) -> SearchEntry {
    let content = strip_html_tags(html);
    // Collapse whitespace for compact index
    let content: String =
        content.split_whitespace().collect::<Vec<_>>().join(" ");
    SearchEntry {
        title: title.to_string(),
        url: url.to_string(),
        content,
    }
}

/// Estimates reading time in minutes from text content.
///
/// Uses 200 words-per-minute average, with a minimum of 1 minute.
///
/// # Examples
///
/// ```
/// assert_eq!(ssg_core::reading_time("a short article"), 1);
/// let long = "word ".repeat(600);
/// assert_eq!(ssg_core::reading_time(&long), 3);
/// ```
#[must_use]
pub fn reading_time(text: &str) -> usize {
    (text.split_whitespace().count() / 200).max(1)
}

/// Separators recognised when splitting a front-matter term list.
///
/// ASCII `,` plus the comma each writing system actually uses. A locale
/// post written in Arabic separates its tags with `،` (U+060C) and one in
/// Japanese with `、` (U+3001); splitting on ASCII alone collapses the whole
/// list into a single term, which then slugifies into one enormous path
/// component. Recognising the others costs nothing for ASCII input and makes
/// a multilingual corpus behave the way its authors wrote it.
const TERM_SEPARATORS: [char; 5] = [
    ',',        // ASCII
    '\u{060C}', // ، Arabic comma
    '\u{FF0C}', // ， fullwidth comma (CJK)
    '\u{3001}', // 、 ideographic comma (CJK enumeration)
    ';',        // occasionally used in hand-authored lists
];

/// Splits a front-matter term list into trimmed, non-empty terms.
///
/// Accepts a comma or semicolon, plus the Arabic comma (`،`), fullwidth
/// comma (`，`) and ideographic comma (`、`), so a tag list keeps its terms
/// whatever script it was written in.
///
/// # Examples
///
/// ```
/// assert_eq!(ssg_core::split_terms("a, b,c"), vec!["a", "b", "c"]);
/// // Arabic comma — one list of three, not one term.
/// assert_eq!(ssg_core::split_terms("أ، ب، ج").len(), 3);
/// assert!(ssg_core::split_terms(" , ,").is_empty());
/// ```
#[must_use]
pub fn split_terms(input: &str) -> Vec<String> {
    input
        .split(TERM_SEPARATORS)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Maximum slug length in **bytes**.
///
/// Slugs become path components, and the common Linux filesystems (ext4,
/// btrfs, xfs) cap a single component at 255 *bytes*. `is_alphanumeric` is
/// Unicode-aware, so non-Latin scripts survive slugification at 2-4 bytes per
/// character: a 230-character Arabic term is 348 bytes and the build dies with
/// ENAMETOOLONG. macOS (APFS) counts *characters*, so the same input succeeds
/// there — which is how this reaches CI without any contributor seeing it.
///
/// 200 leaves headroom for the extensions and suffixes callers append.
const MAX_SLUG_BYTES: usize = 200;

/// Converts a string to a URL-safe slug.
///
/// Lowercases ASCII letters, replaces non-alphanumeric runs with a
/// single `-`, and trims leading/trailing separators. The result is
/// truncated to 200 bytes on a character boundary, so a slug is always a
/// legal path component on Linux filesystems.
///
/// # Examples
///
/// ```
/// assert_eq!(ssg_core::slugify("Hello World!"), "hello-world");
/// assert_eq!(ssg_core::slugify("Rust & Web"), "rust-web");
/// assert_eq!(ssg_core::slugify("--leading--"), "leading");
/// // Long terms are capped in bytes, not characters.
/// assert!(ssg_core::slugify(&"ا".repeat(400)).len() <= 200);
/// ```
#[must_use]
pub fn slugify(input: &str) -> String {
    let slug = input
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if slug.len() <= MAX_SLUG_BYTES {
        return slug;
    }

    // Truncate on a char boundary — byte-slicing a multi-byte sequence
    // panics — then trim any separator the cut leaves dangling.
    let mut end = MAX_SLUG_BYTES;
    while end > 0 && !slug.is_char_boundary(end) {
        end -= 1;
    }
    slug[..end].trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {

    /// YAML front matter of `levels` anchored flow sequences, each holding
    /// `width` aliases of the one before: `width^levels` scalars once
    /// expanded, from a document of a few hundred bytes.
    fn alias_bomb_front_matter(levels: usize, width: usize) -> String {
        let mut doc = String::from("---\n");
        for level in 0..levels {
            let item = if level == 0 {
                "x".to_string()
            } else {
                format!("*l{}", level - 1)
            };
            let items = vec![item; width].join(", ");
            doc.push_str(&format!("l{level}: &l{level} [{items}]\n"));
        }
        doc.push_str("---\n# Body\n");
        doc
    }

    /// RUSTSEC-2026-0333: noyalib before 0.0.53 skipped its alias
    /// budgets on the typed path `parse_frontmatter` uses, so 576 bytes
    /// of front matter expanded to 3.4 MB of JSON. The document must be
    /// refused (no metadata) and the body kept.
    #[test]
    fn yaml_front_matter_alias_amplification_is_refused() {
        for (levels, width) in [(4, 30), (6, 10)] {
            let doc = alias_bomb_front_matter(levels, width);
            let (meta, body) = parse_frontmatter(&doc);
            assert!(
                meta.is_empty(),
                "{}-byte front matter was accepted with {} keys",
                doc.len(),
                meta.len()
            );
            assert_eq!(body.trim(), "# Body");
        }
    }

    /// Control: anchors and aliases inside the budgets still parse, so
    /// the test above fails for the amplification, not for aliases.
    #[test]
    fn yaml_front_matter_modest_aliases_still_parse() {
        let (meta, body) =
            parse_frontmatter("---\nbase: &b [a, b]\ncopy: *b\n---\n# Body");
        assert_eq!(meta.get("copy"), meta.get("base"));
        assert!(meta.contains_key("base"));
        assert_eq!(body.trim(), "# Body");
    }

    /// A code span containing HTML must be escaped, not emitted as markup.
    ///
    /// The blog example carries an accessibility checklist whose second item
    /// is "Every `<img>` has a meaningful `alt`". Rendered through the legacy
    /// `staticdatagen` compiler that sentence produces a real, attribute-less
    /// `<img>` element — so a page about alt text ships an image without one,
    /// and `tests/example_outputs.rs` fails on it.
    ///
    /// `compile_markdown` is the replacement path (WS1 of the v0.0.58 plan
    /// retires `staticdatagen`). This pins the correct behaviour so the fix
    /// arrives with the migration and cannot regress afterwards.
    #[test]
    fn code_spans_escape_html_tags() {
        let html = compile_markdown(
            "Every `<img>` has a meaningful `alt` (or `alt=\"\"`).",
        );
        assert!(
            html.contains("<code>&lt;img&gt;</code>"),
            "code span was not escaped: {html}"
        );
        assert!(
            !html.contains("<code><img></code>"),
            "code span emitted a real <img> element: {html}"
        );
    }

    #[test]
    fn fenced_blocks_escape_html_tags() {
        let html = compile_markdown("```\n<script>alert(1)</script>\n```\n");
        assert!(
            html.contains("&lt;script&gt;"),
            "fenced block was not escaped: {html}"
        );
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "fenced block emitted executable markup: {html}"
        );
    }

    use super::*;

    #[test]
    fn slugify_caps_length_in_bytes_not_characters() {
        // The regression: a real Arabic tag list collapsed into one term is
        // 230 characters but 348 UTF-8 bytes. ext4 caps a path component at
        // 255 bytes, so the build died with ENAMETOOLONG; APFS counts
        // characters, so macOS never saw it.
        let arabic = "\u{0622}\u{0641}\u{0627}\u{0642} ".repeat(60);
        let slug = slugify(&arabic);
        assert!(
            slug.len() <= MAX_SLUG_BYTES,
            "slug is {} bytes, over the {MAX_SLUG_BYTES}-byte cap",
            slug.len()
        );
        // Still a usable slug, not an empty string.
        assert!(!slug.is_empty());
        assert!(!slug.ends_with('-'), "cut left a dangling separator");
    }

    #[test]
    fn slugify_truncates_on_a_char_boundary() {
        // Byte-slicing a multi-byte sequence panics; the cut must land on a
        // boundary for every offset a long multi-byte input can produce.
        for n in 90..140 {
            let slug = slugify(&"\u{3042}".repeat(n)); // hiragana A, 3 bytes
            assert!(slug.len() <= MAX_SLUG_BYTES);
            assert!(std::str::from_utf8(slug.as_bytes()).is_ok());
        }
    }

    #[test]
    fn slugify_leaves_short_slugs_untouched() {
        assert_eq!(slugify("Hello World!"), "hello-world");
        assert_eq!(slugify("Rust & Web"), "rust-web");
    }

    #[test]
    fn split_terms_handles_non_ascii_separators() {
        // Each script's own comma. Splitting on ASCII alone yields one term.
        assert_eq!(split_terms("a, b, c").len(), 3);
        assert_eq!(
            split_terms("\u{0623}\u{060C} \u{0628}\u{060C} \u{062C}").len(),
            3
        );
        assert_eq!(
            split_terms("\u{3042}\u{3001}\u{3044}\u{3001}\u{3046}").len(),
            3
        );
        assert_eq!(split_terms("\u{7532}\u{FF0C}\u{4E59}").len(), 2);
        assert_eq!(split_terms("a; b").len(), 2);
    }

    #[test]
    fn split_terms_trims_and_drops_empties() {
        assert_eq!(split_terms("  a  ,,  b  ,"), vec!["a", "b"]);
        assert!(split_terms(" , , ").is_empty());
        assert!(split_terms("").is_empty());
    }

    #[test]
    fn split_terms_then_slugify_stays_within_the_byte_cap() {
        // The two fixes together: the real corpus shape. Each term is short,
        // so nothing is truncated and no path component can overflow.
        let list = "\u{0623}\u{0644}\u{0623}\u{0639}\u{0645}\u{0627}\u{0644}\u{060C} \u{0627}\u{0644}\u{062A}\u{062C}\u{0627}\u{0631}\u{0629}\u{060C} DORA";
        let slugs: Vec<String> =
            split_terms(list).iter().map(|t| slugify(t)).collect();
        assert_eq!(slugs.len(), 3);
        for s in &slugs {
            assert!(s.len() <= MAX_SLUG_BYTES);
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn compile_markdown_basic() {
        let html = compile_markdown("# Hello\n\nParagraph.");
        assert!(html.contains("<h1>Hello</h1>"));
        assert!(html.contains("<p>Paragraph.</p>"));
    }

    #[test]
    fn compile_markdown_gfm_tables() {
        let input = "| A | B |\n|---|---|\n| 1 | 2 |";
        let html = compile_markdown(input);
        assert!(html.contains("<table>"));
    }

    #[test]
    fn compile_markdown_strikethrough() {
        let html = compile_markdown("~~deleted~~");
        assert!(html.contains("<del>deleted</del>"));
    }

    #[test]
    fn parse_frontmatter_yaml() {
        let (fm, body) = parse_frontmatter(
            "---\ntitle: Hello\ndate: 2026-01-01\n---\n# Body",
        );
        assert_eq!(fm.get("title").and_then(|v| v.as_str()), Some("Hello"));
        assert!(body.contains("# Body"));
    }

    #[test]
    fn parse_frontmatter_toml() {
        let (fm, body) =
            parse_frontmatter("+++\ntitle = \"Hello\"\n+++\n# Body");
        assert_eq!(fm.get("title").and_then(|v| v.as_str()), Some("Hello"));
        assert!(body.contains("# Body"));
    }

    #[test]
    fn parse_frontmatter_json() {
        let (fm, body) = parse_frontmatter("{\"title\": \"Hello\"}\n# Body");
        assert_eq!(fm.get("title").and_then(|v| v.as_str()), Some("Hello"));
        assert!(body.contains("# Body"));
    }

    #[test]
    fn parse_frontmatter_none() {
        let (fm, body) = parse_frontmatter("Just content");
        assert!(fm.is_empty());
        assert_eq!(body, "Just content");
    }

    #[test]
    fn compile_page_full() {
        let input = "---\ntitle: Test\n---\n# Hello\n\nWorld";
        let (fm, html) = compile_page(input).unwrap();
        assert_eq!(fm.get("title").and_then(|v| v.as_str()), Some("Test"));
        assert!(html.contains("<h1>Hello</h1>"));
    }

    #[test]
    fn strip_html_tags_basic() {
        assert_eq!(strip_html_tags("<p>Hello <b>world</b></p>"), "Hello world");
    }

    #[test]
    fn strip_html_tags_empty() {
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn build_search_entry_strips_tags() {
        let entry =
            build_search_entry("Title", "/page", "<p>Hello <b>world</b></p>");
        assert_eq!(entry.title, "Title");
        assert_eq!(entry.content, "Hello world");
    }

    #[test]
    fn reading_time_short() {
        assert_eq!(reading_time("one two three"), 1);
    }

    #[test]
    fn reading_time_long() {
        let text = "word ".repeat(600);
        assert_eq!(reading_time(&text), 3);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World!"), "hello-world");
        assert_eq!(slugify("Rust & Web"), "rust-web");
    }

    #[test]
    fn error_display_frontmatter_parse_variant() {
        let e = Error::FrontmatterParse {
            syntax: "yaml mismatch".to_string(),
        };
        let s = format!("{e}");
        assert!(s.contains("Frontmatter parse error"));
        assert!(s.contains("yaml mismatch"));
    }

    #[test]
    fn error_display_markdown_compile_variant() {
        let e = Error::MarkdownCompile {
            source: "broken markdown".to_string(),
        };
        let s = format!("{e}");
        assert!(s.contains("Markdown compilation error"));
        assert!(s.contains("broken markdown"));
    }

    #[test]
    fn error_display_invalid_slug_variant() {
        let e = Error::InvalidSlug {
            input: "@@@".to_string(),
        };
        let s = format!("{e}");
        assert!(s.contains("Invalid slug input"));
        assert!(s.contains("@@@"));
    }

    #[test]
    fn error_is_std_error_trait_object() {
        // Smoke-tests the `impl std::error::Error for Error {}` block.
        let e: Box<dyn std::error::Error> = Box::new(Error::InvalidSlug {
            input: "x".to_string(),
        });
        assert!(!e.to_string().is_empty());
        // No source by default.
        assert!(std::error::Error::source(&*e).is_none());
    }

    #[test]
    fn error_debug_impl_executes_for_each_variant() {
        let e1 = Error::FrontmatterParse {
            syntax: "a".to_string(),
        };
        let e2 = Error::MarkdownCompile {
            source: "b".to_string(),
        };
        let e3 = Error::InvalidSlug {
            input: "c".to_string(),
        };
        for e in [&e1, &e2, &e3] {
            let s = format!("{e:?}");
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn search_entry_serialization_roundtrip() {
        let e = SearchEntry {
            title: "T".to_string(),
            url: "/u".to_string(),
            content: "C".to_string(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"title\":\"T\""));
        let back: SearchEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.url, "/u");
        assert_eq!(back.content, "C");
        // Debug + Clone are derived; exercise them.
        let _ = format!("{back:?}");
        let _ = back.clone();
    }

    #[test]
    fn compile_page_yields_empty_frontmatter_when_absent() {
        let (fm, html) = compile_page("# Heading\n\nBody").unwrap();
        assert!(fm.is_empty());
        assert!(html.contains("<h1>Heading</h1>"));
    }

    #[test]
    fn slugify_collapses_consecutive_separators() {
        assert_eq!(slugify("foo!!!bar"), "foo-bar");
        assert_eq!(slugify("--leading--"), "leading");
    }

    #[test]
    fn slugify_empty_input_yields_empty() {
        assert_eq!(slugify(""), "");
        assert_eq!(slugify("???"), "");
    }
}
