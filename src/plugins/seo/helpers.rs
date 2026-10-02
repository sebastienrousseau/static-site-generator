// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Internal helper functions for SEO plugins.

use crate::error::SsgError;
use crate::util::head_dom::extract_head_meta;
use std::path::{Path, PathBuf};

/// Extract the page title from the `<title>` tag.
///
/// Backed by [`extract_head_meta`] so a comment containing
/// `<!-- <title>…</title> -->` no longer fools the extractor and quoting
/// / attribute-order variants on `<title>` (extremely rare but legal)
/// are handled by the parser.
///
/// # Examples
///
/// ```rust
/// use ssg::seo::helpers::extract_title;
///
/// let html = "<html><head><title>Hello</title></head></html>";
/// assert_eq!(extract_title(html), "Hello");
/// ```
pub fn extract_title(html: &str) -> String {
    extract_head_meta(html).title
}

/// Extract plain text from the page content, strip tags, and truncate to
/// `max_len` characters.
///
/// Prefers `<main>` content if present. Falls back to `<body>` with nav,
/// header, footer, script, and style blocks removed.
pub(super) fn extract_description(html: &str, max_len: usize) -> String {
    let content = extract_main_content(html);

    let clean = strip_inline_tags(&content, &["script", "style"]);

    let text = strip_tags(&clean);
    let trimmed = text.trim();
    truncate_at_word_boundary(trimmed, max_len)
}

/// Extracts the inner content of `<main>`, or falls back to `<body>` with
/// non-content elements removed.
fn extract_main_content(html: &str) -> String {
    if let Some(inner) = extract_tag_inner(html, "main") {
        return inner;
    }

    let body =
        extract_tag_inner(html, "body").unwrap_or_else(|| html.to_string());
    strip_inline_tags(&body, &["script", "style", "nav", "header", "footer"])
}

/// Extracts the inner HTML of the first occurrence of `<tag_name>...</tag_name>`.
fn extract_tag_inner(html: &str, tag_name: &str) -> Option<String> {
    let open = format!("<{tag_name}");
    let close = format!("</{tag_name}>");
    let start = html.find(&open)?;
    let after = &html[start..];
    let gt = after.find('>')?;
    let inner = &after[gt + 1..];
    if let Some(end) = inner.find(&close) {
        Some(inner[..end].to_string())
    } else {
        Some(inner.to_string())
    }
}

/// Removes matched `<tag>...</tag>` blocks for each tag name in `tags`.
fn strip_inline_tags(html: &str, tags: &[&str]) -> String {
    let mut clean = html.to_string();
    for tag in tags {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        while let Some(start) = clean.find(&open) {
            if let Some(end) = clean[start..].find(&close) {
                clean.replace_range(start..start + end + close.len(), " ");
            } else {
                break;
            }
        }
    }
    clean
}

/// Truncates text to `max_len` at a word boundary.
fn truncate_at_word_boundary(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = &text[..end];
    if let Some(last_space) = truncated.rfind(' ') {
        truncated[..last_space].to_string()
    } else {
        truncated.to_string()
    }
}

/// Remove all HTML tags and collapse whitespace.
pub(super) fn strip_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                result.push(' ');
            }
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    // Collapse whitespace
    let mut collapsed = String::with_capacity(result.len());
    let mut prev_space = false;
    for ch in result.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                collapsed.push(' ');
                prev_space = true;
            }
        } else {
            collapsed.push(ch);
            prev_space = false;
        }
    }
    collapsed.trim().to_string()
}

/// Collect all `.html` files under `dir` (delegates to `crate::walk`).
#[allow(dead_code)] // used only by tests in seo::mod
pub(super) fn collect_html_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files(dir, "html")
}

/// Longest HTML5 named reference (`CounterClockwiseContourIntegral`) is 31
/// characters, so a well-formed `&name;` never exceeds 32 bytes past the `&`.
const MAX_ENTITY_BYTES: usize = 32;

/// If `bytes[start]` (a `&`) begins a well-formed character reference,
/// return the index just past its `;`.
///
/// Accepts `&name;`, `&#NN;` and `&#xNN;`. The cap bounds the scan so a
/// stray `&` in prose cannot walk the rest of the attribute.
fn scan_existing_entity(bytes: &[u8], start: usize) -> Option<usize> {
    let limit = bytes.len().min(start + MAX_ENTITY_BYTES);
    let mut i = start + 1;
    if i >= limit {
        return None;
    }
    let numeric = bytes[i] == b'#';
    if numeric {
        i += 1;
        if i < limit && (bytes[i] == b'x' || bytes[i] == b'X') {
            i += 1;
        }
    }
    let value_start = i;
    while i < limit {
        match bytes[i] {
            b';' if i > value_start => return Some(i + 1),
            b'0'..=b'9' => i += 1,
            b'a'..=b'z' | b'A'..=b'Z' if !numeric => i += 1,
            b'a'..=b'f' | b'A'..=b'F' if numeric => i += 1,
            _ => return None,
        }
    }
    None
}

/// Escape a string for safe inclusion in an HTML attribute value.
///
/// **Idempotent**: an already-formed character reference (`&amp;`, `&#39;`,
/// `&#x27;`) is preserved verbatim rather than having its leading `&`
/// re-escaped. This mirrors the contract `staticweaver` adopted for
/// [ssg#589]; without it the two passes compose into `&amp;amp;`, because
/// this function runs over values the template layer has already escaped.
///
/// That is not hypothetical — it shipped. `og:title` and `twitter:title` are
/// built from already-escaped values, so a title containing `&` reached the
/// page as `&amp;amp;` in 0.0.52 and 0.0.53 (see #706). The naive version
/// looked correct in isolation and was covered by a test that only ever fed
/// it raw characters.
///
/// [ssg#589]: https://github.com/sebastienrousseau/static-site-generator/issues/589
pub(super) fn escape_attr(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut start = 0;
    while i < bytes.len() {
        let replacement = match bytes[i] {
            b'&' => {
                if let Some(end) = scan_existing_entity(bytes, i) {
                    // Already a character reference — copy it through.
                    i = end;
                    continue;
                }
                "&amp;"
            }
            b'"' => "&quot;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => {
                i += 1;
                continue;
            }
        };
        if start < i {
            out.push_str(&s[start..i]);
        }
        out.push_str(replacement);
        i += 1;
        start = i;
    }
    if start < s.len() {
        out.push_str(&s[start..]);
    }
    out
}

/// Check for an actual `<meta` tag (not just an HTML comment marker).
///
/// Staticdatagen may emit empty comment blocks like:
/// ```html
/// <!-- # Start Open Graph / Facebook Meta Tags -->
/// <!-- # End Open Graph / Facebook Meta Tags -->
/// ```
/// These should NOT count as "tag present" — only real `<meta` tags do.
///
/// # Examples
///
/// ```rust
/// use ssg::seo::helpers::has_meta_tag;
///
/// let html = r#"<meta property="og:title" content="x">"#;
/// assert!(has_meta_tag(html, "og:title"));
/// assert!(!has_meta_tag("<!-- og:image -->", "og:image"));
/// ```
pub fn has_meta_tag(html: &str, attr: &str) -> bool {
    html.contains(&format!("<meta property=\"{attr}\""))
        || html.contains(&format!("<meta property='{attr}'"))
        || html.contains(&format!("<meta name=\"{attr}\""))
        || html.contains(&format!("<meta name='{attr}'"))
}

/// Extract the canonical URL from a `<link rel="canonical">` tag.
///
/// Backed by [`extract_head_meta`] so the result no longer depends on
/// attribute order or quoting, and `<pre>` content containing a literal
/// canonical link does not leak into the result.
pub(super) fn extract_canonical(html: &str) -> String {
    extract_head_meta(html).canonical
}

/// Extract the content of a specific meta tag by name or property.
///
/// Attribute-based matching: tolerant of attribute order, quoting style
/// (double, single, unquoted), and case — minified HTML emits forms
/// like `<meta content=x name=twitter:image>` that literal prefix
/// matching misses.
pub(super) fn extract_existing_meta(html: &str, attr: &str) -> String {
    use std::cell::RefCell;
    use std::rc::Rc;

    use lol_html::element;

    use crate::util::html_rewriter::rewrite_html;

    // A real parser, not a `<meta` scan. The scan matched the literal bytes
    // wherever they appeared — including inside an HTML comment, so a
    // commented-out `<meta name="twitter:image">` left over from an edit
    // won the lookup over the live tag that followed it. `lol_html` never
    // reports comment contents as elements, so the class goes away rather
    // than being special-cased.
    let found: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let sink = Rc::clone(&found);
    let want = attr.to_ascii_lowercase();

    let handlers = vec![element!("meta", move |el| {
        if !sink.borrow().is_empty() {
            return Ok(());
        }
        // Either `name` or `property` may carry the key; both are matched
        // case-insensitively because the parser preserves author casing.
        let keyed = ["name", "property"].iter().any(|k| {
            el.get_attribute(k)
                .is_some_and(|v| v.eq_ignore_ascii_case(&want))
        });
        if keyed {
            if let Some(content) = el.get_attribute("content") {
                let value = content.trim();
                if !value.is_empty() {
                    sink.borrow_mut().push_str(value);
                }
            }
        }
        Ok(())
    })];

    let _ = rewrite_html(html, handlers);

    let out = found.borrow().clone();
    out
}

/// Extract the `lang` attribute from the `<html>` tag.
///
/// Backed by [`extract_head_meta`] so a `<pre>` block containing
/// `<html lang="…">` markup no longer confuses the extractor.
pub(super) fn extract_html_lang(html: &str) -> String {
    extract_head_meta(html).lang
}

/// Extract the first image URL from `<main>` or `<article>` content.
pub(super) fn extract_first_content_image(html: &str) -> String {
    // Look in <main> or <article> first
    let search_region = if let Some(start) = html.find("<main") {
        &html[start..]
    } else if let Some(start) = html.find("<article") {
        &html[start..]
    } else {
        return String::new();
    };

    if let Some(img_pos) = search_region.find("<img") {
        let after_img = &search_region[img_pos..];
        // Scan at most 500 bytes of the tag, stepping back to a char
        // boundary: text in the tag may be multi-byte (a Tamil `alt`).
        let mut tag_end = after_img.find('>').unwrap_or(500).min(500);
        while !after_img.is_char_boundary(tag_end) {
            tag_end -= 1;
        }
        let img_tag = &after_img[..tag_end];
        if let Some(src_pos) = img_tag.find("src=\"") {
            let after_src = &img_tag[src_pos + 5..];
            if let Some(end) = after_src.find('"') {
                return after_src[..end].to_string();
            }
        }
    }
    String::new()
}

/// Extract the author name from `<meta name="author">` or byline markup.
pub(super) fn extract_meta_author(html: &str) -> String {
    // Try meta tag first
    let from_meta = extract_existing_meta(html, "author");
    if !from_meta.is_empty() {
        return from_meta;
    }
    // Try <span class="author"> or similar byline patterns
    for pattern in &["class=\"author\">", "class='author'>", "rel=\"author\">"]
    {
        if let Some(pos) = html.find(pattern) {
            let after = &html[pos + pattern.len()..];
            if let Some(end) = after.find('<') {
                let name = after[..end].trim();
                // Strip "by " prefix
                let name = name.strip_prefix("by ").unwrap_or(name).trim();
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    String::new()
}

/// Extract a date from an existing JSON-LD block in the HTML.
pub(super) fn extract_date_from_html(
    html: &str,
    field: &str,
) -> Option<String> {
    let pattern = format!("\"{field}\":\"");
    if let Some(pos) = html.find(&pattern) {
        let after = &html[pos + pattern.len()..];
        if let Some(end) = after.find('"') {
            let date = &after[..end];
            if !date.is_empty() {
                return Some(date.to_string());
            }
        }
    }
    None
}

/// Extract a date from `<time datetime="...">` or `<meta property="article:published_time">`.
pub(super) fn extract_meta_date(html: &str) -> Option<String> {
    // Try article:published_time meta
    let meta = extract_existing_meta(html, "article:published_time");
    if !meta.is_empty() {
        return Some(meta);
    }
    // Try first <time datetime="..."> in the page
    if let Some(pos) = html.find("datetime=\"") {
        let after = &html[pos + 10..];
        if let Some(end) = after.find('"') {
            let date = &after[..end];
            if !date.is_empty() {
                return Some(date.to_string());
            }
        }
    }
    None
}

/// Recursively collects HTML files (delegates to `crate::walk`).
#[allow(dead_code)] // used only by tests in seo::mod
pub(super) fn collect_html_files_recursive(
    dir: &Path,
) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files(dir, "html")
}

#[cfg(test)]
mod tests {

    // ---- ssg#539 acceptance criteria -----------------------------------

    /// AC1: a `<title>` inside an HTML comment must not win.
    #[test]
    fn ac1_title_ignores_commented_title() {
        let html = "<html><head><!-- <title>Old</title> --><title>Real</title></head><body></body></html>";
        assert_eq!(extract_title(html), "Real");
    }

    /// AC3: canonical is read from the real `<link rel=canonical>`.
    #[test]
    fn ac3_canonical_is_detected() {
        let html = r#"<html><head><link rel="canonical" href="https://x"></head><body></body></html>"#;
        assert_eq!(extract_canonical(html), "https://x");
    }

    /// A `<meta>` inside a comment must not be read as a real meta tag.
    #[test]
    fn ac_meta_ignores_commented_meta() {
        let html = concat!(
            "<html><head>",
            "<!-- <meta name=\"twitter:image\" content=\"COMMENTED\"> -->",
            "<meta name=\"twitter:image\" content=\"REAL\">",
            "</head><body></body></html>"
        );
        assert_eq!(extract_existing_meta(html, "twitter:image"), "REAL");
    }

    /// A `<meta>` shown as escaped example text inside `<pre>` must not win.
    #[test]
    fn ac_meta_ignores_meta_in_pre_block() {
        let html = concat!(
            "<html><head><meta name=\"description\" content=\"REAL\"></head>",
            "<body><pre>&lt;meta name=\"description\" content=\"EXAMPLE\"&gt;</pre></body></html>"
        );
        assert_eq!(extract_existing_meta(html, "description"), "REAL");
    }

    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn extract_title_from_html() {
        let html = "<html><head><title>Test Page</title></head></html>";
        assert_eq!(extract_title(html), "Test Page");
    }

    #[test]
    fn extract_existing_meta_minified_unquoted_and_reordered() {
        // Regression: minified HTML emits unquoted values with
        // `content` before `name`/`property`.
        let html = "<head><meta content=https://ex.test/img.png \
                    name=twitter:image></head>";
        assert_eq!(
            extract_existing_meta(html, "twitter:image"),
            "https://ex.test/img.png"
        );
        let html2 =
            "<head><meta content=\"https://ex.test/og.png\" property=og:image></head>";
        assert_eq!(
            extract_existing_meta(html2, "og:image"),
            "https://ex.test/og.png"
        );
    }

    #[test]
    fn extract_existing_meta_quoted_forms_still_work() {
        let html = r#"<meta name="author" content="Alice">"#;
        assert_eq!(extract_existing_meta(html, "author"), "Alice");
        let html2 = r#"<meta property='og:image' content='/x.png'>"#;
        assert_eq!(extract_existing_meta(html2, "og:image"), "/x.png");
    }

    #[test]
    fn extract_existing_meta_absent_returns_empty() {
        let html = "<head><meta name=viewport content=width=device-width>\
                    </head>";
        assert_eq!(extract_existing_meta(html, "og:image"), "");
    }

    #[test]
    fn extract_existing_meta_skips_empty_content_and_keeps_scanning() {
        let html = "<meta name=author content=\"\">\
                    <meta name=author content=\"Bea\">";
        assert_eq!(extract_existing_meta(html, "author"), "Bea");
    }

    #[test]
    fn extract_title_empty_no_tag() {
        let html = "<html><head></head><body>Hello</body></html>";
        assert_eq!(extract_title(html), "");
    }

    #[test]
    fn extract_title_empty_tag() {
        let html = "<html><head><title></title></head></html>";
        assert_eq!(extract_title(html), "");
    }

    #[test]
    fn extract_title_nested_tags() {
        let html = "<title><span>Inner</span></title>";
        // strip_tags removes the inner span, leaving "Inner"
        assert_eq!(extract_title(html), "Inner");
    }

    #[test]
    fn extract_description_from_body() {
        let html = "<html><body><main><p>Short description here.</p></main></body></html>";
        let desc = extract_description(html, 200);
        assert!(desc.contains("Short description here"));
    }

    #[test]
    fn extract_description_truncation() {
        let long_text = "word ".repeat(100);
        let html = format!("<main><p>{long_text}</p></main>");
        let desc = extract_description(&html, 50);
        assert!(desc.len() <= 50);
    }

    #[test]
    fn strip_tags_basic() {
        assert_eq!(strip_tags("<p>Hello <b>world</b></p>"), "Hello world");
    }

    #[test]
    fn strip_tags_empty() {
        assert_eq!(strip_tags(""), "");
    }

    #[test]
    fn strip_tags_no_tags() {
        assert_eq!(strip_tags("plain text"), "plain text");
    }

    #[test]
    fn strip_tags_self_closing() {
        let result = strip_tags("<img src=\"x\"/>text");
        assert!(result.contains("text"));
        assert!(!result.contains("img"));
    }

    #[test]
    fn truncate_short_text_unchanged() {
        assert_eq!(truncate_at_word_boundary("short", 100), "short");
    }

    #[test]
    fn truncate_long_text_at_word() {
        let text = "one two three four five six";
        let result = truncate_at_word_boundary(text, 15);
        assert!(result.len() <= 15);
        // Should cut at a space
        assert!(!result.ends_with(' '));
        assert_eq!(result, "one two three");
    }

    #[test]
    fn truncate_unicode() {
        let text = "日本語 テスト データ";
        let result = truncate_at_word_boundary(text, 15);
        // Must not panic on multi-byte boundaries
        assert!(result.len() <= 15);
    }

    #[test]
    fn collect_html_files_finds_files() {
        let tmp = tempdir().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(tmp.path().join("index.html"), "<html></html>").unwrap();
        fs::write(sub.join("page.html"), "<html></html>").unwrap();

        let files = collect_html_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn collect_html_files_recursive_finds_files() {
        let tmp = tempdir().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(tmp.path().join("index.html"), "<html></html>").unwrap();
        fs::write(sub.join("page.html"), "<html></html>").unwrap();
        fs::write(sub.join("style.css"), "body{}").unwrap();

        let files = collect_html_files_recursive(tmp.path()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|p| p.extension().unwrap() == "html"));
    }

    #[test]
    fn collect_html_files_recursive_empty_dir() {
        let tmp = tempdir().unwrap();
        let files = collect_html_files_recursive(tmp.path()).unwrap();
        assert!(files.is_empty(), "{:?}", files);
    }

    #[test]
    fn escape_attr_special_chars() {
        assert_eq!(escape_attr("a&b<c>d\"e"), "a&amp;b&lt;c&gt;d&quot;e");
    }

    /// #706: `og:title` / `twitter:title` are built from values the template
    /// layer already escaped. A naive pass re-escapes the `&` of `&amp;` and
    /// ships `&amp;amp;` to the page — 1,494 pages on one real corpus.
    #[test]
    fn escape_attr_preserves_existing_entities() {
        assert_eq!(escape_attr("A &amp; B"), "A &amp; B");
        assert_eq!(escape_attr("A &lt; B"), "A &lt; B");
        assert_eq!(escape_attr("A &#39; B"), "A &#39; B");
        assert_eq!(escape_attr("A &#x27; B"), "A &#x27; B");
        assert_eq!(escape_attr("&nbsp;"), "&nbsp;");
    }

    /// The property the old test could not see, because it only ever fed
    /// `escape_attr` raw characters: escaping twice must equal escaping once.
    #[test]
    fn escape_attr_is_idempotent() {
        for input in [
            "AI, Payments & Post-Quantum Cryptography",
            "a&b<c>d\"e",
            "Tom & Jerry's <b>show</b>",
            "already &amp; escaped",
            "mixed & already &amp; both",
            "",
            "no metacharacters at all",
        ] {
            let once = escape_attr(input);
            let twice = escape_attr(&once);
            assert_eq!(once, twice, "not idempotent for {input:?}");
        }
    }

    /// A bare `&` in prose is still escaped — the entity scan must not treat
    /// arbitrary following text as a reference, or it would silently emit
    /// invalid markup.
    #[test]
    fn escape_attr_still_escapes_bare_ampersands() {
        assert_eq!(escape_attr("Tom & Jerry"), "Tom &amp; Jerry");
        assert_eq!(escape_attr("a & b & c"), "a &amp; b &amp; c");
        // Unterminated / malformed references are not references.
        assert_eq!(escape_attr("&amp"), "&amp;amp");
        assert_eq!(escape_attr("&;"), "&amp;;");
        assert_eq!(escape_attr("&#;"), "&amp;#;");
        // Longer than the 32-byte cap: not a reference.
        let long = format!("&{};", "a".repeat(40));
        assert!(escape_attr(&long).starts_with("&amp;"));
    }

    #[test]
    fn has_meta_tag_present() {
        let html = r#"<meta property="og:title" content="Hi">"#;
        assert!(has_meta_tag(html, "og:title"));
    }

    #[test]
    fn has_meta_tag_absent() {
        let html = "<html><head></head></html>";
        assert!(!has_meta_tag(html, "og:title"));
    }

    #[test]
    fn extract_canonical_found() {
        let html = r#"<link rel="canonical" href="https://example.com/page">"#;
        assert_eq!(extract_canonical(html), "https://example.com/page");
    }

    #[test]
    fn extract_canonical_missing() {
        let html = "<html><head></head></html>";
        assert_eq!(extract_canonical(html), "");
    }

    #[test]
    fn extract_existing_meta_by_name() {
        let html = r#"<meta name="author" content="Alice">"#;
        assert_eq!(extract_existing_meta(html, "author"), "Alice");
    }

    #[test]
    fn extract_html_lang_found() {
        let html = r#"<html lang="fr"><head></head></html>"#;
        assert_eq!(extract_html_lang(html), "fr");
    }

    #[test]
    fn extract_html_lang_missing() {
        let html = "<html><head></head></html>";
        assert_eq!(extract_html_lang(html), "");
    }

    #[test]
    fn extract_date_from_html_found() {
        let html = r#"{"datePublished":"2025-01-15"}"#;
        assert_eq!(
            extract_date_from_html(html, "datePublished"),
            Some("2025-01-15".to_string())
        );
    }

    #[test]
    fn extract_date_from_html_missing() {
        assert_eq!(
            extract_date_from_html("<html></html>", "datePublished"),
            None
        );
    }

    #[test]
    fn extract_existing_meta_skips_tag_without_content_attribute() {
        // A matching meta tag that carries no `content` attribute at
        // all must be skipped (scanning continues past it).
        let html = "<meta name=\"author\">\
                    <meta name=\"author\" content=\"Cid\">";
        assert_eq!(extract_existing_meta(html, "author"), "Cid");
    }

    #[test]
    fn extract_existing_meta_no_content_anywhere_returns_empty() {
        let html = "<meta name=\"author\">";
        assert_eq!(extract_existing_meta(html, "author"), "");
    }

    #[test]
    fn extract_first_content_image_src_without_closing_quote() {
        // `src="` opened but never closed before the tag ends — the
        // extractor must fall through to the empty result.
        let html = "<main><img src=\"broken.png</main>";
        assert_eq!(extract_first_content_image(html), "");
    }

    #[test]
    fn extract_first_content_image_img_without_src_attribute() {
        let html = "<main><img alt=\"decorative\"></main>";
        assert_eq!(extract_first_content_image(html), "");
    }

    #[test]
    fn extract_meta_author_byline_with_empty_name_returns_empty() {
        // `class="author">` immediately followed by a closing tag —
        // the byline text is empty and must not be returned.
        let html = "<html><body><span class=\"author\"></span></body></html>";
        assert_eq!(extract_meta_author(html), "");
    }

    #[test]
    fn extract_meta_author_byline_without_following_tag_returns_empty() {
        // Pattern matches at the very end of the document, so there is
        // no `<` terminating the byline text.
        let html = "<span class=\"author\">Dana";
        assert_eq!(extract_meta_author(html), "");
    }

    #[test]
    fn extract_date_from_html_empty_date_returns_none() {
        let html = r#"{"datePublished":""}"#;
        assert_eq!(extract_date_from_html(html, "datePublished"), None);
    }

    #[test]
    fn extract_date_from_html_unterminated_value_returns_none() {
        // Opening quote never closed — no closing `"` after the value.
        let html = r#"{"datePublished":"2026-01-01"#;
        assert_eq!(extract_date_from_html(html, "datePublished"), None);
    }

    #[test]
    fn extract_meta_date_empty_datetime_returns_none() {
        let html = r#"<time datetime="">x</time>"#;
        assert_eq!(extract_meta_date(html), None);
    }

    #[test]
    fn extract_meta_date_unterminated_datetime_returns_none() {
        let html = r#"<time datetime="2026-01-01"#;
        assert_eq!(extract_meta_date(html), None);
    }

    // ---- multi-byte text in an <img> tag -------------------------------
    //
    // The tag scan is capped at 500 bytes. With no `>` inside that window,
    // byte 500 can fall inside a multi-byte character, and slicing there
    // panicked (`end byte index 500 is not a char boundary`). Found
    // building a Tamil page, where each character is three bytes.

    /// `src` comes first and the long Tamil `alt` runs past the cap.
    #[test]
    fn first_content_image_src_before_long_multibyte_alt() {
        let html = format!(
            "<main><img src=\"/ta.png\" alt=\"{}\"></main>",
            "ட".repeat(200)
        );
        assert_eq!(extract_first_content_image(&html), "/ta.png");
    }

    /// `src` sits beyond the cap: no image, and no panic.
    #[test]
    fn first_content_image_src_past_cap_inside_multibyte_run() {
        // `<img alt="` is 10 bytes; 10 + 3k never equals 500.
        let html = format!(
            "<main><img alt=\"{}\" src=\"/late.png\"></main>",
            "ட".repeat(200)
        );
        assert_eq!(extract_first_content_image(&html), "");
    }

    /// A short, unterminated tag: no `>` and fewer than 500 bytes left,
    /// so the cap itself lies past the end of the string.
    #[test]
    fn first_content_image_unterminated_short_tag() {
        assert_eq!(
            extract_first_content_image("<main><img src=\"/a.png\""),
            "/a.png"
        );
    }
}
