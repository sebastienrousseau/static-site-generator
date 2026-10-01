// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! HTML fix plugin.

use super::helpers::rfc2822_to_iso8601;
use crate::error::SsgError;
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use crate::util::html_rewriter::rewrite_html;
use anyhow::Result;
use lol_html::element;
use std::path::Path;

/// Repairs HTML output:
/// - Fix 7: Upgrades JSON-LD `@context` from `http://schema.org/` to
///   `https://schema.org`.
/// - Fix 9: Repairs broken `.class=` image syntax where `<p` is
///   injected into `<img>` tags.
#[derive(Debug, Clone, Copy)]
pub struct HtmlFixPlugin;

impl Plugin for HtmlFixPlugin {
    fn name(&self) -> &'static str {
        "html-fix"
    }

    fn has_transform(&self) -> bool {
        true
    }

    fn transform_html(
        &self,
        html: &str,
        _path: &Path,
        _ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        Ok(apply_html_fixes(html))
    }

    fn after_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
        Ok(())
    }
}

/// Applies all HTML fixes to a single page and returns the modified content.
fn apply_html_fixes(html: &str) -> String {
    let mut modified = html.to_string();

    if needs_schema_context_fix(&modified) {
        modified = modified
            .replace("\"http://schema.org/\"", "\"https://schema.org\"")
            .replace("\"http://schema.org\"", "\"https://schema.org\"");
    }

    if modified.contains("application/ld+json") {
        modified = fix_jsonld_dates(&modified);
    }

    if modified.contains("<p src=") {
        modified = fix_broken_img_tags(&modified);
    }

    if needs_class_syntax_fix(&modified) {
        modified = fix_literal_class_syntax(&modified);
    }

    if needs_mobile_web_app_capable_meta(&modified) {
        modified = inject_mobile_web_app_capable_meta(&modified);
    }

    if has_empty_preload(&modified) {
        modified = remove_empty_preload_links(&modified);
    }

    if modified.contains("align=") {
        modified = replace_table_align_attrs(&modified);
    }

    if modified.contains("<th") {
        modified = add_table_header_scope(&modified);
    }

    if modified.contains("<table") {
        modified = wrap_tables_for_reflow(&modified);
    }

    if modified.contains("<code><") {
        modified = escape_markup_inside_code_spans(&modified);
    }

    modified
}

/// Escapes raw tags that the legacy compiler leaves inside `<code>` spans.
///
/// Markdown renders `` `<img>` `` as a code span whose text is the literal
/// characters `<img>`; the correct HTML is `<code>&lt;img&gt;</code>`. The
/// `staticdatagen` renderer escapes quotes but not angle brackets, so it emits
/// `<code><img></code>` — a real, attribute-less element.
///
/// The blog example is the case that surfaced it: an accessibility checklist
/// reading "Every `<img>` has a meaningful `alt`" shipped a page containing an
/// `<img>` with no `alt`, which `tests/example_outputs.rs` correctly failed.
/// Beyond the irony, any documentation quoting `<script>` in prose would have
/// emitted a live script element.
///
/// `ssg_core::compile_markdown` gets this right (it uses pulldown-cmark's own
/// HTML writer), so this repair exists only until WS1 retires the legacy
/// compiler — at which point the guard above stops matching and the pass costs
/// nothing.
fn escape_markup_inside_code_spans(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(start) = rest.find("<code>") {
        let after_open = start + "<code>".len();
        let Some(close_rel) = rest[after_open..].find("</code>") else {
            break;
        };
        let close = after_open + close_rel;

        // A bare `<code>` opening a `<pre>` is a code *block*, not an inline
        // span, and a theme may have authored real markup inside it -- Voxt
        // ships a hand-highlighted Rust sample built from `<span class=
        // "code-kw">` and friends. Escaping those printed the tags on the
        // page as text. Markdown-derived blocks are unaffected either way:
        // the highlighter emits `<code class="language-x">`, which this
        // function never matches because it looks for the bare string.
        //
        // Inline spans -- the case this repair exists for -- are never
        // preceded by `<pre>`, so they still get escaped.
        let preceding = rest[..start].trim_end();
        let opens_a_pre_block = preceding.ends_with('>')
            && preceding.rfind("<pre").is_some_and(|i| {
                preceding[i..]
                    .find('>')
                    .is_some_and(|j| i + j + 1 == preceding.len())
            });
        if opens_a_pre_block {
            let end = close + "</code>".len();
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }

        out.push_str(&rest[..after_open]);
        // Only the span's text is escaped; the surrounding markup, including
        // any attributes on the <code> element itself, is left untouched.
        for ch in rest[after_open..close].chars() {
            match ch {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                other => out.push(other),
            }
        }
        out.push_str("</code>");
        rest = &rest[close + "</code>".len()..];
    }

    out.push_str(rest);
    out
}

/// Gives every `<th>` a `scope`.
///
/// Markdown emits bare `<th>` elements. Without `scope`, a screen reader
/// has to guess which cells a header governs, and on anything wider than
/// two columns it guesses wrong — WCAG 1.3.1, technique H63.
///
/// A header inside `<thead>` labels its column; one inside `<tbody>` is a
/// row header, which is the shape Markdown produces for a leading label
/// column. An author-supplied `scope` is never overwritten.
fn add_table_header_scope(html: &str) -> String {
    rewrite_html(
        html,
        vec![
            element!("thead th:not([scope])", |el| {
                el.set_attribute("scope", "col")?;
                Ok(())
            }),
            element!("tbody th:not([scope])", |el| {
                el.set_attribute("scope", "row")?;
                Ok(())
            }),
            // A `<th>` in neither section still governs its column.
            element!("table > tr th:not([scope])", |el| {
                el.set_attribute("scope", "col")?;
                Ok(())
            }),
        ],
    )
    .unwrap_or_else(|_| html.to_string())
}

/// Wraps every `<table>` in a horizontally scrollable container.
///
/// A table is the one element that legitimately cannot reflow: its columns
/// have a minimum width, and below that the table pushes the document
/// wider than the viewport. That is a WCAG 1.4.10 (Reflow) failure, and it
/// is what a Markdown table does on a phone — measured at 320px, a
/// five-column table made the document 588px wide.
///
/// Scrolling *inside* a container is the accepted fix, so the table gets
/// its own scroll context and the page stops scrolling sideways. Applied
/// here rather than in a theme because Markdown-generated tables have no
/// wrapper to style: only a post-process pass can reach them.
///
/// `role="region"` plus a label makes the scrollable area focusable and
/// announced, which is what lets a keyboard user reach the overflowed
/// columns at all.
fn wrap_tables_for_reflow(html: &str) -> String {
    use lol_html::html_content::ContentType;

    let already = "ssg-table-scroll";
    if html.contains(already) {
        return html.to_string();
    }

    rewrite_html(
        html,
        vec![element!("table", move |el| {
            el.before(
                &format!(
                    "<div class=\"table-wrap {already}\" role=\"region\" \
                     aria-label=\"Table, scrollable horizontally\" tabindex=\"0\">"
                ),
                ContentType::Html,
            );
            el.after("</div>", ContentType::Html);
            Ok(())
        })],
    )
    .unwrap_or_else(|_| html.to_string())
}

/// Replaces the obsolete presentational `align` attribute on table cells
/// with an equivalent `text-*` class (issue #618).
///
/// Markdown column-alignment syntax (`:---`, `---:`, `:---:`) is rendered
/// downstream as `<th align="left">` / `<td align="right">`. `align` has
/// been obsolete since HTML5 and pa11y flags it as
/// `WCAG2AAA.Principle1.Guideline1_3.1_3_1.H49.AlignAttr`.
///
/// The alignment itself is meaningful, so it is preserved rather than
/// dropped: each cell gains `text-left` / `text-center` / `text-right`,
/// which is the class the renderer already emits on `<td>` alongside the
/// attribute. `<th>` previously received the attribute and no class, so
/// this is also what makes header alignment stylable at all.
///
/// Uses a real parser rather than string surgery: an `align=` literal can
/// legitimately appear inside a `<pre>` block or a comment, and only a
/// parser knows the difference.
fn replace_table_align_attrs(html: &str) -> String {
    let handler = |el: &mut lol_html::html_content::Element<'_, '_>| {
        let Some(align) = el.get_attribute("align") else {
            return Ok(());
        };
        el.remove_attribute("align");

        let class = match align.trim().to_ascii_lowercase().as_str() {
            "left" => "text-left",
            "center" | "centre" => "text-center",
            "right" => "text-right",
            // `justify`, `char`, or anything unrecognised: the attribute is
            // still obsolete and must go, but inventing a class for it would
            // be guessing at intent.
            _ => return Ok(()),
        };

        let existing = el.get_attribute("class").unwrap_or_default();
        if existing.split_whitespace().any(|c| c == class) {
            return Ok(());
        }
        let merged = if existing.is_empty() {
            class.to_string()
        } else {
            format!("{existing} {class}")
        };
        el.set_attribute("class", &merged)?;
        Ok(())
    };

    rewrite_html(
        html,
        vec![
            element!("th[align]", handler),
            element!("td[align]", handler),
        ],
    )
    .unwrap_or_else(|_| html.to_string())
}

/// Returns `true` if the HTML contains `http://schema.org` context that needs upgrading.
fn needs_schema_context_fix(html: &str) -> bool {
    html.contains("\"http://schema.org/\"")
        || html.contains("\"http://schema.org\"")
}

/// Returns `true` if the HTML contains literal `.class=` syntax to fix.
fn needs_class_syntax_fix(html: &str) -> bool {
    html.contains(".class=&quot;") || html.contains(".class=\"")
}

/// Returns `true` if the HTML appears to contain a `<link rel="preload">`
/// tag whose `href` is empty or absent. Chrome logs
/// "<link rel=preload> has an invalid href value" for these. The check
/// is intentionally cheap; `remove_empty_preload_links` does the precise
/// per-tag work only if this returns `true`.
fn has_empty_preload(html: &str) -> bool {
    // The cheapest signal of "preload + no real href" is `href` followed
    // immediately by space or `>` (bare attribute) anywhere in the same
    // document, *and* a preload link somewhere too. False positives just
    // trigger the precise rewriter, which is idempotent.
    let has_preload = html.contains("rel=preload")
        || html.contains("rel=\"preload\"")
        || html.contains("rel='preload'");
    let has_empty_href = html.contains("href=\"\"")
        || html.contains("href=''")
        || html.contains(" href ")
        || html.contains(" href>")
        || html.contains(" href/>");
    has_preload && has_empty_href
}

/// Removes any `<link>` tag that declares `rel="preload"` and has an empty
/// or missing `href`. Idempotent.
pub(super) fn remove_empty_preload_links(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while cursor < html.len() {
        // Find the next `<link` (case-insensitive) starting at cursor.
        let Some(rel_offset) =
            html[cursor..].to_ascii_lowercase().find("<link")
        else {
            out.push_str(&html[cursor..]);
            break;
        };
        let tag_start = cursor + rel_offset;
        out.push_str(&html[cursor..tag_start]);

        // Walk forward to the closing `>`, respecting quoted attribute values.
        let bytes = html.as_bytes();
        let mut j = tag_start;
        let mut quote: Option<u8> = None;
        while j < bytes.len() {
            let b = bytes[j];
            match quote {
                Some(q) if b == q => quote = None,
                Some(_) => {}
                None => match b {
                    b'"' | b'\'' => quote = Some(b),
                    b'>' => break,
                    _ => {}
                },
            }
            j += 1;
        }
        let tag_end = (j + 1).min(html.len());
        let tag = &html[tag_start..tag_end];
        let lower = tag.to_ascii_lowercase();
        let is_preload = lower.contains("rel=\"preload\"")
            || lower.contains("rel='preload'")
            || lower.contains("rel=preload");
        let has_real_href = href_is_present_and_non_empty(&lower);
        // Drop only empty-href preload tags; keep everything else.
        if !is_preload || has_real_href {
            out.push_str(tag);
        }
        cursor = tag_end;
    }
    out
}

/// Returns `true` if a (lowercased) tag string has a `href` attribute that
/// is present and non-empty. Tolerates double, single, and unquoted forms.
fn href_is_present_and_non_empty(lower_tag: &str) -> bool {
    if lower_tag.contains("href=\"\"") || lower_tag.contains("href=''") {
        return false;
    }
    let Some(idx) = lower_tag.find("href") else {
        return false;
    };
    // Must be followed by `=`, possibly with surrounding whitespace.
    let after = lower_tag[idx + 4..].trim_start();
    let Some(rest) = after.strip_prefix('=') else {
        return false;
    };
    let rest = rest.trim_start();
    // NB: `trim_start` above means the next char can never be
    // whitespace, so no dedicated whitespace arm is needed.
    match rest.chars().next() {
        None | Some('>') => false,
        Some('"') => rest.len() > 1 && !rest.starts_with("\"\""),
        Some('\'') => rest.len() > 1 && !rest.starts_with("''"),
        Some(_) => true,
    }
}

/// Returns `true` if the HTML emits the legacy
/// `apple-mobile-web-app-capable` meta but lacks the modern
/// `mobile-web-app-capable` meta that Chrome now requires. Tolerates
/// quoted, single-quoted, or unquoted attribute values (post-minify HTML
/// often drops quotes around short values like `yes`).
fn needs_mobile_web_app_capable_meta(html: &str) -> bool {
    let has_legacy = html.contains("apple-mobile-web-app-capable");
    let has_modern = find_modern_mobile_web_app_capable(html).is_some();
    has_legacy && !has_modern
}

/// Returns the byte offset of a `name=...mobile-web-app-capable...` meta
/// attribute that is **not** the apple variant, or `None` if none found.
fn find_modern_mobile_web_app_capable(html: &str) -> Option<usize> {
    // Search for the bare attribute name in any of the three quoting
    // styles, then verify it isn't preceded by `apple-` (which would make
    // it the legacy variant).
    let needles = [
        "name=\"mobile-web-app-capable\"",
        "name='mobile-web-app-capable'",
        "name=mobile-web-app-capable",
    ];
    for n in &needles {
        if let Some(pos) = html.find(n) {
            return Some(pos);
        }
    }
    None
}

/// Injects `<meta name="mobile-web-app-capable" content="yes">` immediately
/// after the legacy Apple variant so installed-PWA support works in Chrome
/// without console deprecation warnings. Handles minified HTML where the
/// `name=` attribute may be unquoted and may appear after `content=`.
///
/// When the legacy meta is HTML-escaped (e.g. `&lt;meta
/// name=&quot;apple-mobile-web-app-capable&quot;…&gt;` leaked into body
/// content via a misconfigured template), no usable anchor exists. In that
/// case, fall back to injecting the modern meta into `<head>` so the
/// modern-companion validator still passes.
pub(super) fn inject_mobile_web_app_capable_meta(html: &str) -> String {
    let modern = "<meta name=\"mobile-web-app-capable\" content=\"yes\">";
    // Find the apple-variant attribute name. Tolerate quoted/unquoted forms.
    let candidates = [
        "name=\"apple-mobile-web-app-capable\"",
        "name='apple-mobile-web-app-capable'",
        "name=apple-mobile-web-app-capable",
    ];
    let name_pos = candidates.iter().find_map(|n| html.find(n));
    if let Some(name_pos) = name_pos {
        // Walk forward to the next `>` that closes this <meta> tag.
        let after = &html[name_pos..];
        if let Some(rel_close) = after.find('>') {
            let insert_at = name_pos + rel_close + 1;
            return format!(
                "{}{modern}{}",
                &html[..insert_at],
                &html[insert_at..]
            );
        }
    }
    // Fallback: the legacy meta is present only in escaped form (no real
    // anchor in the source). Inject the modern meta into <head> so Chrome
    // gets the companion and the modern-companion validator passes.
    inject_modern_meta_into_head(html, modern)
}

/// Injects a `<meta>` tag just before `</head>`, or just after `<head>` if
/// the close tag isn't present. If neither anchor exists, prepends the meta
/// so the document still contains the marker — but in practice every
/// well-formed page has a `<head>` element.
fn inject_modern_meta_into_head(html: &str, meta: &str) -> String {
    // Prefer inserting right before </head> so it lives in the head block
    // regardless of where the apple-meta string appeared.
    let lower = html.to_ascii_lowercase();
    if lower.contains("</head>") {
        let injected = inject_before_head_close(html, meta);
        if injected != html {
            return injected;
        }
    }
    if let Some(pos) = lower.find("<head>") {
        let insert_at = pos + "<head>".len();
        return format!("{}{meta}{}", &html[..insert_at], &html[insert_at..]);
    }
    // No <head> at all — prepend so the substring is at least present.
    format!("{meta}{html}")
}

/// Fix JSON-LD date fields from RFC 2822 to ISO 8601.
pub(super) fn fix_jsonld_dates(html: &str) -> String {
    let mut result = html.to_string();

    // Match "datePublished":"..." and "dateModified":"..." patterns
    for field in &["datePublished", "dateModified"] {
        let pattern = format!("\"{field}\":\"");
        let mut search_from = 0;
        while let Some(start) = result[search_from..].find(&pattern) {
            let abs_start = search_from + start + pattern.len();
            if let Some(end) = result[abs_start..].find('"') {
                let date_str = &result[abs_start..abs_start + end];
                // Only convert if it looks like RFC 2822 (starts with
                // a day abbreviation like "Mon," "Tue,", etc.)
                if date_str.len() > 5
                    && date_str.as_bytes()[3] == b','
                    && date_str.as_bytes()[0].is_ascii_alphabetic()
                {
                    let iso = rfc2822_to_iso8601(date_str);
                    if iso != date_str {
                        result = format!(
                            "{}{}{}",
                            &result[..abs_start],
                            iso,
                            &result[abs_start + end..]
                        );
                    }
                }
                search_from = abs_start + 1;
            } else {
                break;
            }
        }
    }

    result
}

/// Repair broken `<img ... <p src="...">` patterns by reconstructing
/// valid `<img>` tags.
pub(super) fn fix_broken_img_tags(html: &str) -> String {
    let mut result = html.to_string();
    // Pattern: <img ... <p src="URL">
    // Replace with: <img ... src="URL" />
    while let Some(p_pos) = result.find("<p src=") {
        // Look backwards for the <img tag start
        let before = &result[..p_pos];
        if let Some(img_start) = before.rfind("<img") {
            // Extract the src value from <p src="...">
            let after_p = &result[p_pos..]; // includes "<p src="
            if let Some(quote_start) = after_p.find("src=\"") {
                let val_start = quote_start + 5; // skip src="
                let remaining = &after_p[val_start..];
                if let Some(quote_end) = remaining.find('"') {
                    let src_value = remaining[..quote_end].to_string();
                    // Find the closing > of this broken tag
                    let close_offset = remaining[quote_end..]
                        .find('>')
                        .map_or(result.len(), |i| {
                            p_pos + val_start + quote_end + i + 1
                        });

                    // Extract existing attributes from the img tag portion
                    let img_attrs = result[img_start + 4..p_pos].trim();
                    let img_attrs_clean =
                        img_attrs.trim_end_matches(|c: char| {
                            c.is_whitespace() || c == '<'
                        });

                    let new_img = format!(
                        "<img {img_attrs_clean} src=\"{src_value}\" />"
                    );
                    result = format!(
                        "{}{}{}",
                        &result[..img_start],
                        new_img,
                        &result[close_offset..]
                    );
                    continue;
                }
            }
        }
        // If we can't parse, skip to avoid infinite loop
        break;
    }
    result
}

/// Remove literal `.class=&quot;...&quot;` or `.class="..."` from HTML
/// and apply them as actual class attributes.
pub(super) fn fix_literal_class_syntax(html: &str) -> String {
    let mut result = html.to_string();

    // Handle .class=&quot;...&quot; (HTML-encoded quotes)
    result = fix_class_syntax_variant(&result, ".class=&quot;", "&quot;");
    // Handle .class="..." (literal quotes)
    result = fix_class_syntax_variant(&result, ".class=\"", "\"");

    result
}

/// Handles one variant of the `.class=` syntax fix.
fn fix_class_syntax_variant(
    html: &str,
    open_pattern: &str,
    close_pattern: &str,
) -> String {
    let mut result = html.to_string();
    while let Some(start) = result.find(open_pattern) {
        let after = &result[start + open_pattern.len()..];
        if let Some(end) = after.find(close_pattern) {
            let class_value = after[..end].to_string();
            let remove_end =
                start + open_pattern.len() + end + close_pattern.len();
            result = format!("{}{}", &result[..start], &result[remove_end..]);
            inject_class_attr(&mut result, start, &class_value);
        } else {
            break;
        }
    }
    result
}

/// Injects a class attribute into the nearest preceding tag if it doesn't already have one.
fn inject_class_attr(html: &mut String, pos: usize, class_value: &str) {
    if let Some(tag_end) = html[..pos].rfind('>') {
        if let Some(tag_start) = html[..tag_end].rfind('<') {
            let tag = &html[tag_start..tag_end];
            if !tag.contains("class=") {
                let insert_pos = tag_end;
                *html = format!(
                    "{} class=\"{}\"{}",
                    &html[..insert_pos],
                    class_value,
                    &html[insert_pos..]
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Markup quoted in prose stays text.
    ///
    /// An article explaining an element writes it as text: the source has
    /// `&lt;div&gt;`, and the reader must see `<div>`. A pass that
    /// "repaired" escaped tags from a fixed list turned that text into a
    /// live element (and, matching by prefix, `&lt;embed` via `&lt;em` and
    /// `&lt;link` via `&lt;li`). It existed to undo staticdatagen escaping
    /// whole page bodies, which staticdatagen 0.0.21 no longer does.
    #[test]
    fn quoted_markup_in_prose_is_left_as_text() {
        for quoted in [
            "&lt;div class=&quot;card&quot;&gt;",
            "&lt;section&gt;",
            "&lt;embed src=&quot;x.swf&quot;&gt;",
            "&lt;link rel=&quot;stylesheet&quot; href=&quot;x.css&quot;&gt;",
        ] {
            let html = format!(
                "<html><head></head><body><main><p>Write {quoted} to wrap it.</p></main></body></html>"
            );
            let out = apply_html_fixes(&html);
            assert!(
                out.contains(quoted),
                "quoted markup {quoted:?} must stay escaped, got: {out}"
            );
        }
    }

    /// A themed code block keeps its markup.
    ///
    /// Regression: v0.0.58 escaped every bare `<code>`, including the ones
    /// opening a `<pre>`. Voxt ships a hand-highlighted Rust sample, and the
    /// release printed `&lt;span class="code-kw"&gt;` on the page as visible
    /// text. Production (built with 0.0.56) rendered it correctly, so this
    /// shipped as a regression in a release.
    #[test]
    fn a_pre_block_keeps_authored_markup() {
        let html = "<pre class=\"editor-code\"><code>\
<span class=\"code-kw\">pub fn</span> main()</code></pre>";
        let out = escape_markup_inside_code_spans(html);
        assert_eq!(out, html, "a <pre><code> block must pass through intact");
        assert!(!out.contains("&lt;span"), "spans must not be escaped");
    }

    /// An inline code span is still repaired -- the case the pass exists for.
    #[test]
    fn an_inline_code_span_is_still_escaped() {
        let html = "<p>Every <code><img></code> needs alt text.</p>";
        let out = escape_markup_inside_code_spans(html);
        assert!(
            out.contains("<code>&lt;img&gt;</code>"),
            "inline spans must still be escaped, got: {out}"
        );
    }

    /// The discriminator is `<pre>` immediately before, not merely present.
    #[test]
    fn an_inline_span_after_an_earlier_pre_is_still_escaped() {
        let html = "<pre><code>x</code></pre><p>Use <code><br></code>.</p>";
        let out = escape_markup_inside_code_spans(html);
        assert!(
            out.contains("<code>&lt;br&gt;</code>"),
            "a later inline span must still be escaped, got: {out}"
        );
        assert!(
            out.contains("<pre><code>x</code></pre>"),
            "the earlier block must be untouched, got: {out}"
        );
    }

    #[test]
    fn escapes_a_tag_left_raw_inside_a_code_span() {
        // The blog example's accessibility checklist: a page about alt text
        // was shipping an <img> with no alt, because the code span rendered
        // as markup.
        let html = "<li>Every <code><img></code> has a meaningful <code>alt</code></li>";
        let out = apply_html_fixes(html);
        assert!(out.contains("<code>&lt;img&gt;</code>"), "got: {out}");
        assert!(!out.contains("<code><img></code>"), "got: {out}");
        // The neighbouring, already-correct span is untouched.
        assert!(out.contains("<code>alt</code>"), "got: {out}");
    }

    #[test]
    fn escaping_code_spans_leaves_surrounding_markup_alone() {
        let html = "<p>Before</p><code><b>x</b></code><p>After <em>y</em></p>";
        let out = apply_html_fixes(html);
        assert!(
            out.contains("<code>&lt;b&gt;x&lt;/b&gt;</code>"),
            "got: {out}"
        );
        assert!(out.contains("<p>Before</p>"), "got: {out}");
        assert!(out.contains("<em>y</em>"), "got: {out}");
    }

    #[test]
    fn already_escaped_code_spans_are_not_double_escaped() {
        // Idempotence matters: the pass runs on every page, and a second
        // application must not turn &lt; into &amp;lt;.
        let html = "<code>&lt;img&gt;</code>";
        let once = apply_html_fixes(html);
        let twice = apply_html_fixes(&once);
        assert_eq!(once, twice, "pass is not idempotent");
        assert!(!twice.contains("&amp;lt;"), "double-escaped: {twice}");
    }

    #[test]
    fn unterminated_code_span_does_not_truncate_the_document() {
        // A malformed document must come through unchanged rather than lose
        // everything after the opening tag.
        let html = "<p>keep</p><code><img>";
        let out = apply_html_fixes(html);
        assert!(out.contains("<p>keep</p>"), "content lost: {out}");
    }

    use super::*;
    use crate::plugin::PluginContext;
    use std::path::Path;
    use tempfile::tempdir;

    /// Covers the *wiring*, not just the function: the unit tests above
    /// call `replace_table_align_attrs` directly, so removing its call
    /// from `apply_html_fixes` left them all green. This one goes through
    /// the pipeline entry point.
    #[test]
    fn apply_html_fixes_strips_table_align_attrs() {
        let out = apply_html_fixes(
            r#"<table><tr><td align="right">7</td></tr></table>"#,
        );
        assert!(
            !out.contains("align="),
            "not wired into the pipeline: {out}"
        );
        assert!(out.contains("text-right"), "{out}");
    }

    /// Markdown emits bare `<th>`. Without `scope` a screen reader guesses
    /// which cells a header governs, and gets it wrong on anything wider
    /// than two columns (WCAG 1.3.1, technique H63).
    #[test]
    fn table_headers_gain_a_scope() {
        let out = add_table_header_scope(concat!(
            "<table><thead><tr><th>Plan</th></tr></thead>",
            "<tbody><tr><th>Starter</th><td>Free</td></tr></tbody></table>",
        ));
        assert!(out.contains(r#"<th scope="col">Plan"#), "{out}");
        assert!(out.contains(r#"<th scope="row">Starter"#), "{out}");
    }

    /// An author who scoped a header deliberately keeps their value.
    #[test]
    fn table_header_scope_does_not_overwrite_an_author_value() {
        let out = add_table_header_scope(
            r#"<table><thead><tr><th scope="rowgroup">X</th></tr></thead></table>"#,
        );
        assert!(out.contains(r#"scope="rowgroup""#), "{out}");
        assert_eq!(out.matches("scope=").count(), 1, "{out}");
    }

    /// The wrapper is what stops a table widening the page; it must be
    /// reachable by keyboard, or the overflowed columns are unreachable.
    #[test]
    fn tables_are_wrapped_in_a_focusable_scroll_region() {
        let out = wrap_tables_for_reflow("<table><tr><td>x</td></tr></table>");
        assert!(out.contains("table-wrap"), "{out}");
        assert!(out.contains(r#"role="region""#), "{out}");
        assert!(out.contains(r#"tabindex="0""#), "{out}");
        assert!(out.contains("aria-label"), "{out}");
    }

    /// Wrapping twice would nest scroll containers on a rebuild.
    #[test]
    fn table_wrapping_is_idempotent() {
        let once = wrap_tables_for_reflow("<table><tr><td>x</td></tr></table>");
        assert_eq!(wrap_tables_for_reflow(&once), once);
    }

    /// Regression for #618: Markdown column-alignment syntax rendered
    /// obsolete `align` attributes, which pa11y flags as
    /// `WCAG2AAA.Principle1.Guideline1_3.1_3_1.H49.AlignAttr`.
    #[test]
    fn table_align_attrs_become_text_classes() {
        let html = concat!(
            "<table><thead><tr>",
            r#"<th align="left">Layer</th>"#,
            r#"<th align="center">Maturity</th>"#,
            r#"<th align="right">Metric</th>"#,
            "</tr></thead></table>",
        );
        let out = replace_table_align_attrs(html);

        assert!(
            !out.contains("align="),
            "obsolete attribute survived: {out}"
        );
        assert!(out.contains("text-left"), "{out}");
        assert!(out.contains("text-center"), "{out}");
        assert!(out.contains("text-right"), "{out}");
    }

    /// The renderer already emits `class="text-*"` on `<td>` beside the
    /// attribute; merging must not duplicate it.
    #[test]
    fn table_align_does_not_duplicate_an_existing_class() {
        let html = r#"<td align="right" class="text-right num">7</td>"#;
        let out = replace_table_align_attrs(html);

        assert!(!out.contains("align="), "{out}");
        assert_eq!(out.matches("text-right").count(), 1, "duplicated: {out}");
        assert!(out.contains("num"), "existing classes dropped: {out}");
    }

    /// An `align` value with no sensible class equivalent still loses the
    /// obsolete attribute — inventing a class would be guessing at intent.
    #[test]
    fn table_align_unrecognised_value_drops_attribute_without_a_class() {
        let out = replace_table_align_attrs(r#"<td align="justify">x</td>"#);
        assert!(!out.contains("align="), "{out}");
        assert!(!out.contains("text-"), "invented a class: {out}");
    }

    /// `align=` inside a `<pre>` block is content, not markup, and a
    /// string-replacement implementation would corrupt it.
    #[test]
    fn table_align_leaves_literal_text_in_pre_alone() {
        let html = r#"<pre><code>&lt;td align="left"&gt;</code></pre>"#;
        assert_eq!(replace_table_align_attrs(html), html);
    }

    /// Non-table elements keep their `align` — the fix is scoped to the
    /// cells the Markdown renderer emits, not a blanket attribute sweep.
    #[test]
    fn table_align_ignores_non_cell_elements() {
        let html = r#"<div align="center">x</div>"#;
        assert_eq!(replace_table_align_attrs(html), html);
    }

    fn test_ctx(site_dir: &Path) -> PluginContext {
        crate::test_support::init_logger();
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            site_dir,
            Path::new("templates"),
        )
    }

    #[test]
    fn test_html_fix_upgrades_jsonld_context() -> Result<()> {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());

        let html = r#"<html><head>
<script type="application/ld+json">
{"@context":"http://schema.org/","@type":"WebPage"}
</script>
</head><body></body></html>"#;

        let result = HtmlFixPlugin
            .transform_html(html, Path::new("index.html"), &ctx)
            .unwrap();
        assert!(result.contains("\"https://schema.org\""));
        assert!(!result.contains("\"http://schema.org/\""));
        Ok(())
    }

    #[test]
    fn test_html_fix_converts_jsonld_dates() -> Result<()> {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());

        let html = r#"<html><head>
<script type="application/ld+json">
{"@context":"https://schema.org","@type":"Article","datePublished":"Thu, 11 Apr 2026 06:06:06 +0000","dateModified":"Mon, 01 Sep 2025 06:06:06 +0000"}
</script>
</head><body></body></html>"#;

        let result = HtmlFixPlugin
            .transform_html(html, Path::new("article.html"), &ctx)
            .unwrap();
        assert!(
            result.contains("\"datePublished\":\"2026-04-11"),
            "Expected ISO date, got: {result}"
        );
        assert!(
            result.contains("\"dateModified\":\"2025-09-01"),
            "Expected ISO date, got: {result}"
        );
        assert!(!result.contains("Thu, 11 Apr"));
        Ok(())
    }

    #[test]
    fn test_fix_broken_img_tags() {
        let input =
            r#"<img alt="test" class="w-25" title="test" <p src="image.jpg">"#;
        let result = fix_broken_img_tags(input);
        assert!(result.contains("src=\"image.jpg\""));
        assert!(!result.contains("<p src="));
    }

    #[test]
    fn test_fix_literal_class_syntax() {
        let input = r#"<img alt="test" src="img.jpg">.class=&quot;w-25 float-start&quot;"#;
        let result = fix_literal_class_syntax(input);
        assert!(!result.contains(".class=&quot;"));
    }

    // -----------------------------------------------------------------
    // fix_jsonld_dates
    // -----------------------------------------------------------------

    #[test]
    fn test_fix_jsonld_dates_iso_passthrough() {
        let input =
            r#"{"datePublished":"2026-04-11","dateModified":"2025-09-01"}"#;
        let result = fix_jsonld_dates(input);
        assert_eq!(result, input, "ISO dates should pass through unchanged");
    }

    #[test]
    fn test_fix_jsonld_dates_converts_rfc2822() {
        let input = r#"{"datePublished":"Thu, 11 Apr 2026 06:06:06 +0000"}"#;
        let result = fix_jsonld_dates(input);
        assert!(
            result.contains("\"datePublished\":\"2026-04-11T06:06:06+00:00\""),
            "Should convert RFC 2822 to ISO 8601, got: {result}"
        );
    }

    #[test]
    fn test_fix_jsonld_dates_both_fields() {
        let input = r#"{"datePublished":"Mon, 01 Sep 2025 12:00:00 +0000","dateModified":"Tue, 02 Sep 2025 14:30:00 +0000"}"#;
        let result = fix_jsonld_dates(input);
        assert!(result.contains("2025-09-01T12:00:00+00:00"));
        assert!(result.contains("2025-09-02T14:30:00+00:00"));
    }

    // -----------------------------------------------------------------
    // fix_broken_img_tags
    // -----------------------------------------------------------------

    #[test]
    fn test_fix_broken_img_tags_multiple() {
        let input =
            r#"<img alt="a" <p src="one.jpg"><img alt="b" <p src="two.jpg">"#;
        let result = fix_broken_img_tags(input);
        assert!(result.contains("src=\"one.jpg\""), "first img: {result}");
        assert!(result.contains("src=\"two.jpg\""), "second img: {result}");
        assert!(
            !result.contains("<p src="),
            "no broken tags remain: {result}"
        );
    }

    #[test]
    fn test_fix_broken_img_tags_none() {
        let input = r#"<img alt="ok" src="good.jpg" />"#;
        let result = fix_broken_img_tags(input);
        assert_eq!(
            result, input,
            "No broken tags should leave input unchanged"
        );
    }

    // -----------------------------------------------------------------
    // fix_literal_class_syntax
    // -----------------------------------------------------------------

    #[test]
    fn test_fix_literal_class_syntax_html_encoded() {
        let input =
            r#"<img src="img.jpg">.class=&quot;w-25 float-start&quot; rest"#;
        let result = fix_literal_class_syntax(input);
        assert!(
            !result.contains(".class=&quot;"),
            "should remove .class=&quot;"
        );
        assert!(
            result.contains("class=\"w-25 float-start\""),
            "should inject class attr, got: {result}"
        );
    }

    #[test]
    fn test_fix_literal_class_syntax_literal_quotes() {
        let input = r#"<img src="img.jpg">.class="my-class" rest"#;
        let result = fix_literal_class_syntax(input);
        assert!(
            !result.contains(".class=\""),
            "should remove .class=\", got: {result}"
        );
        assert!(
            result.contains("class=\"my-class\""),
            "should inject class attr, got: {result}"
        );
    }

    #[test]
    fn test_fix_literal_class_syntax_no_class() {
        let input = r#"<img src="img.jpg"> some text"#;
        let result = fix_literal_class_syntax(input);
        assert_eq!(result, input, "No .class= should leave input unchanged");
    }

    // -----------------------------------------------------------------
    // inject_mobile_web_app_capable_meta
    // -----------------------------------------------------------------

    #[test]
    fn test_inject_mobile_web_app_capable_meta_added() {
        let input = r#"<head><meta name="apple-mobile-web-app-capable" content="yes"></head>"#;
        let result = inject_mobile_web_app_capable_meta(input);
        assert!(
            result.contains(
                r#"<meta name="mobile-web-app-capable" content="yes">"#
            ),
            "modern meta should be injected, got: {result}"
        );
        assert!(
            result.contains(
                r#"<meta name="apple-mobile-web-app-capable" content="yes">"#
            ),
            "legacy meta must remain for backwards compatibility"
        );
    }

    // -----------------------------------------------------------------
    // remove_empty_preload_links
    // -----------------------------------------------------------------

    #[test]
    fn test_remove_empty_preload_drops_bare_href() {
        let input = r#"<head><link as=image fetchpriority=high href rel=preload type=image/webp><title>x</title></head>"#;
        let result = remove_empty_preload_links(input);
        assert!(
            !result.contains("rel=preload"),
            "empty preload should be removed, got: {result}"
        );
        assert!(result.contains("<title>x</title>"), "rest preserved");
    }

    #[test]
    fn test_remove_empty_preload_drops_quoted_empty_href() {
        let input = r#"<link rel="preload" href="" as="image">"#;
        let result = remove_empty_preload_links(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_remove_empty_preload_keeps_valid_preload() {
        let input = r#"<link rel="preload" href="/banner.webp" as="image">"#;
        let result = remove_empty_preload_links(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_remove_empty_preload_preserves_utf8() {
        let input = r#"<title>日本語</title><link rel=preload href as=image><p>テスト</p>"#;
        let result = remove_empty_preload_links(input);
        assert!(result.contains("日本語"));
        assert!(result.contains("テスト"));
        assert!(!result.contains("rel=preload"));
    }

    #[test]
    fn test_apply_html_fixes_idempotent_on_modern_meta() {
        let input = r#"<head><meta name="apple-mobile-web-app-capable" content="yes"><meta name="mobile-web-app-capable" content="yes"></head>"#;
        let result = apply_html_fixes(input);
        // Should not double-inject when modern meta already exists.
        let count = result.matches("name=\"mobile-web-app-capable\"").count();
        assert_eq!(count, 1, "no duplicate injection, got: {result}");
    }

    #[test]
    fn test_apply_html_fixes_idempotent_on_modern_meta_single_quotes() {
        let input = r#"<head><meta name="apple-mobile-web-app-capable" content="yes"><meta name='mobile-web-app-capable' content="yes"></head>"#;
        let result = apply_html_fixes(input);
        assert!(
            !result.contains("name=\"mobile-web-app-capable\""),
            "Should not inject modern meta when single quoted one exists"
        );
    }

    #[test]
    fn test_apply_html_fixes_idempotent_on_modern_meta_unquoted() {
        let input = r#"<head><meta name="apple-mobile-web-app-capable" content="yes"><meta name=mobile-web-app-capable content="yes"></head>"#;
        let result = apply_html_fixes(input);
        assert!(
            !result.contains("name=\"mobile-web-app-capable\""),
            "Should not inject modern meta when unquoted one exists"
        );
    }

    #[test]
    fn test_html_fix_plugin_metadata() {
        assert_eq!(HtmlFixPlugin.name(), "html-fix");
        assert!(HtmlFixPlugin.has_transform());
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        assert!(HtmlFixPlugin.after_compile(&ctx).is_ok());
    }

    #[test]
    fn test_needs_schema_context_fix() {
        assert!(needs_schema_context_fix("\"http://schema.org/\""));
        assert!(needs_schema_context_fix("\"http://schema.org\""));
        assert!(!needs_schema_context_fix("\"https://schema.org\""));
    }

    #[test]
    fn test_needs_class_syntax_fix() {
        assert!(needs_class_syntax_fix(".class=&quot;foo&quot;"));
        assert!(needs_class_syntax_fix(".class=\"foo\""));
        assert!(!needs_class_syntax_fix("class=\"foo\""));
    }

    #[test]
    fn test_has_empty_preload() {
        assert!(has_empty_preload("<link rel=\"preload\" href=\"\">"));
        assert!(has_empty_preload("<link rel='preload' href=''>"));
        assert!(has_empty_preload("<link rel=preload href>"));
        assert!(!has_empty_preload("<link rel=\"preload\" href=\"/foo\">"));
        assert!(!has_empty_preload("<link rel=\"stylesheet\" href=\"\">"));
    }

    #[test]
    fn test_remove_empty_preload_unclosed_tag() {
        let input = "<link rel=\"preload\" href=\"\"";
        let result = remove_empty_preload_links(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_remove_empty_preload_unclosed_quotes() {
        let input = "<link rel=\"preload href=\"\" >";
        let result = remove_empty_preload_links(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_href_is_present_and_non_empty_edge_cases() {
        assert!(!href_is_present_and_non_empty(""));
        assert!(!href_is_present_and_non_empty("src=foo"));
        assert!(!href_is_present_and_non_empty("href"));
        assert!(!href_is_present_and_non_empty("href  "));
        assert!(!href_is_present_and_non_empty("href = >"));
        assert!(!href_is_present_and_non_empty("href = \"\""));
        assert!(!href_is_present_and_non_empty("href = ''"));
        assert!(!href_is_present_and_non_empty("href =  "));
        assert!(!href_is_present_and_non_empty("href="));
        assert!(!href_is_present_and_non_empty("href=>"));
        assert!(!href_is_present_and_non_empty("href=  "));
        assert!(!href_is_present_and_non_empty("href=\""));
        assert!(!href_is_present_and_non_empty("href='"));
        assert!(href_is_present_and_non_empty("href = \"/a\""));
        assert!(href_is_present_and_non_empty("href = '/a'"));
        assert!(href_is_present_and_non_empty("href=foo"));
    }

    #[test]
    fn test_needs_mobile_web_app_capable_meta() {
        assert!(needs_mobile_web_app_capable_meta(
            "apple-mobile-web-app-capable"
        ));
        assert!(!needs_mobile_web_app_capable_meta(
            "apple-mobile-web-app-capable and name=\"mobile-web-app-capable\""
        ));
        assert!(!needs_mobile_web_app_capable_meta("no legacy meta"));
    }

    #[test]
    fn test_inject_mobile_web_app_capable_meta_edge_cases() {
        // Missing apple meta — no <head> either, must inject to keep
        // substring present (caller only invokes this when the legacy
        // substring is present somewhere in the document).
        let no_head = inject_mobile_web_app_capable_meta("plain text");
        assert!(
            no_head.contains("name=\"mobile-web-app-capable\""),
            "fallback should inject modern meta: {no_head}"
        );

        // Unclosed apple meta tag — no closing `>` so the primary anchor
        // path cannot insert; falls through to head-injection. Since
        // there's also no <head>, the meta is prepended.
        let unclosed = inject_mobile_web_app_capable_meta(
            "<meta name=\"apple-mobile-web-app-capable\"",
        );
        assert!(
            unclosed.contains("name=\"mobile-web-app-capable\""),
            "fallback should inject modern meta: {unclosed}"
        );
    }

    #[test]
    fn test_inject_modern_meta_fallback_when_apple_meta_is_escaped() {
        // Regression for PR #511 / feat/v0.0.41: staticdatagen-rendered
        // pages can leak the legacy apple meta as fully HTML-escaped body
        // text (`&lt;meta name=&quot;apple-…&quot;…&gt;`). The injector
        // cannot find a `name="apple-…"` anchor, so it must fall back to
        // injecting the modern companion into <head> instead.
        let html = "<html><head><title>x</title></head><body>\
                    &lt;meta name=&quot;apple-mobile-web-app-capable&quot; \
                    content=&quot;yes&quot;&gt;</body></html>";
        let result = apply_html_fixes(html);
        assert!(
            result.contains("name=\"mobile-web-app-capable\""),
            "modern companion must be injected even when legacy is escaped"
        );
        // And it should land inside <head>, not after the escaped body text.
        let modern_pos =
            result.find("name=\"mobile-web-app-capable\"").unwrap();
        let head_close_pos = result.find("</head>").unwrap();
        assert!(
            modern_pos < head_close_pos,
            "modern meta should live inside <head>:\n{result}"
        );
    }

    #[test]
    fn test_fix_jsonld_dates_invalid_rfc2822() {
        // String too short
        let input = r#"{"datePublished":"Mon"}"#;
        assert_eq!(fix_jsonld_dates(input), input);

        // Doesn't start with day abbreviation / comma
        let input2 = r#"{"datePublished":"2026, 11 Apr 2026"}"#;
        assert_eq!(fix_jsonld_dates(input2), input2);

        // Non-matching field
        let input3 = r#"{"dateCreated":"Thu, 11 Apr 2026 06:06:06 +0000"}"#;
        assert_eq!(fix_jsonld_dates(input3), input3);

        // Missing quote
        let input4 = r#"{"datePublished":"Thu, 11 Apr 2026"#;
        assert_eq!(fix_jsonld_dates(input4), input4);
    }

    #[test]
    fn test_fix_broken_img_tags_edge_cases() {
        // Missing quote for src
        let input = r#"<img <p src=image.jpg>"#;
        assert_eq!(fix_broken_img_tags(input), input);

        // No img tag before p
        let input2 = r#"<p src="image.jpg">"#;
        assert_eq!(fix_broken_img_tags(input2), input2);
    }

    #[test]
    fn test_fix_literal_class_syntax_edge_cases() {
        // Unclosed class syntax
        let input = r#"<img src="img.jpg">.class="my-class"#;
        assert_eq!(fix_literal_class_syntax(input), input);
    }

    #[test]
    fn test_inject_class_attr_edge_cases() {
        // No preceding tag
        let mut html = "some text without tags".to_string();
        inject_class_attr(&mut html, 10, "foo");
        assert_eq!(html, "some text without tags");

        // Preceding tag already has class
        let mut html2 = "<img class=\"existing\"> some text".to_string();
        let len = html2.len();
        inject_class_attr(&mut html2, len, "foo");
        assert_eq!(html2, "<img class=\"existing\"> some text");

        // A `>` exists before `pos` but has no matching `<` before it
        // (malformed/truncated markup) — the inner `rfind('<')` must
        // return `None` and the function must leave the string alone.
        let mut html3 = "> stray text".to_string();
        let len3 = html3.len();
        inject_class_attr(&mut html3, len3, "foo");
        assert_eq!(html3, "> stray text");
    }

    // -----------------------------------------------------------------
    // apply_html_fixes: routing gates for each fixer
    // -----------------------------------------------------------------

    #[test]
    fn test_apply_html_fixes_routes_broken_img_repair() {
        let html = r#"<img alt="x" <p src="/pic.png"> tail"#;
        let out = apply_html_fixes(html);
        assert!(
            out.contains(r#"<img alt="x" src="/pic.png" />"#),
            "broken img must be repaired via the apply pipeline: {out}"
        );
    }

    #[test]
    fn test_apply_html_fixes_routes_class_syntax_repair() {
        let html = r#"<div>.class="hero"</div>"#;
        let out = apply_html_fixes(html);
        assert!(
            !out.contains(".class="),
            "literal class syntax must be removed via the apply pipeline: {out}"
        );
    }

    #[test]
    fn test_apply_html_fixes_routes_empty_preload_removal() {
        let html = r#"<head><link rel="preload" href="" as="style"><link rel="stylesheet" href="/a.css"></head>"#;
        let out = apply_html_fixes(html);
        assert!(
            !out.contains("rel=\"preload\""),
            "empty-href preload must be dropped via the apply pipeline: {out}"
        );
        assert!(out.contains("/a.css"), "real links survive: {out}");
    }

    // -----------------------------------------------------------------
    // fix_jsonld_dates: RFC-2822-shaped but unparseable value
    // -----------------------------------------------------------------

    #[test]
    fn test_fix_jsonld_dates_keeps_unparseable_rfc_shaped_date() {
        let html = r#"{"datePublished":"Mon, not a real date"}"#;
        let out = fix_jsonld_dates(html);
        assert_eq!(out, html, "unparseable date passes through verbatim");
    }

    // -----------------------------------------------------------------
    // fix_broken_img_tags: unterminated src attribute bails out
    // -----------------------------------------------------------------

    #[test]
    fn test_fix_broken_img_tags_unterminated_src_bails_out() {
        let html = r#"<img alt="x" <p src="never-closes"#;
        let out = fix_broken_img_tags(html);
        assert_eq!(out, html, "unterminated src must not loop or rewrite");
    }

    // -----------------------------------------------------------------
    // inject_class_attr: preceding tag already has a class
    // -----------------------------------------------------------------

    #[test]
    fn test_fix_literal_class_syntax_keeps_existing_class_attr() {
        let html = r#"<div class="old">.class="new"</div>"#;
        let out = fix_literal_class_syntax(html);
        assert!(out.contains(r#"class="old""#), "existing class kept: {out}");
        assert!(
            !out.contains(r#"class="new""#),
            "no second class attribute injected: {out}"
        );
    }

    // -----------------------------------------------------------------
    // inject_modern_meta_into_head fallbacks
    // -----------------------------------------------------------------

    #[test]
    fn test_inject_meta_falls_back_when_head_close_is_escaped_only() {
        // `</head>` appears only as text with no real head element, so
        // the lol_html pass injects nothing and we fall through to the
        // prepend fallback.
        let html = "no real head here </head>";
        let out = inject_mobile_web_app_capable_meta(html);
        assert!(
            out.starts_with("<meta name=\"mobile-web-app-capable\""),
            "prepend fallback used: {out}"
        );
    }

    #[test]
    fn test_inject_meta_after_open_head_when_no_close_tag() {
        let html = "<head><meta charset=\"utf-8\">";
        let out = inject_mobile_web_app_capable_meta(html);
        assert!(
            out.starts_with(
                "<head><meta name=\"mobile-web-app-capable\" content=\"yes\">"
            ),
            "meta injected right after <head>: {out}"
        );
    }
}
