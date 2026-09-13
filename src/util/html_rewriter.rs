// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Thin wrapper around Cloudflare's [`lol_html`] streaming HTML rewriter.
//!
//! Replaces the previous fragile `str::find` / `str::rfind` rewriting
//! that lived in the image, search, CSP, and html-fix plugins (issue
//! #525). Compared to the old approach `lol_html`:
//!
//! - **Skips HTML comments.** `<!-- <img …> -->` is byte-identical in
//!   the output rather than being half-rewritten.
//! - **Preserves character entities.** `alt="Café &amp; bar"` round-trips
//!   verbatim — no double-encoding, no entity decoding inside attribute
//!   values.
//! - **Streams.** Peak RSS stays flat regardless of input size; there is
//!   no full-document DOM materialised in memory.
//!
//! ## API
//!
//! The crate-private functions [`rewrite_html`] and
//! [`extract_text_with_filter`] cover every call site inside SSG.
//! Plugins pass closure-based [`ElementContentHandlers`] or write to a
//! `&mut String` aggregator and let the wrapper deal with feeding
//! chunks through `lol_html::HtmlRewriter`.

use crate::error::SsgError;
use lol_html::html_content::TextChunk;
use lol_html::{
    element, end_tag, rewrite_str, ElementContentHandlers, RewriteStrSettings,
    Selector,
};
use std::borrow::Cow;

/// Run `lol_html` over `html` with the supplied
/// `(selector, handlers)` pairs and return the rewritten document.
///
/// Maps any `lol_html` failure (selector parse error, encoder failure,
/// memory-limit overflow) onto [`SsgError::Io`] with a synthetic
/// `<lol_html>` path so callers don't have to deal with `lol_html`'s
/// error types directly.
///
/// # Examples
///
/// ```rust
/// use ssg::util::html_rewriter::rewrite_html;
///
/// // With no handlers the output is byte-identical to the input.
/// let out = rewrite_html("<p>hi</p>", Vec::new()).unwrap();
/// assert_eq!(out, "<p>hi</p>");
/// ```
///
/// # Errors
///
/// Returns [`SsgError::Io`] if `lol_html` rejects any selector or
/// fails while writing output chunks.
pub fn rewrite_html<'h>(
    html: &str,
    handlers: Vec<(Cow<'h, Selector>, ElementContentHandlers<'h>)>,
) -> Result<String, SsgError> {
    let mut settings = RewriteStrSettings::new();
    for h in handlers {
        settings = settings.append_element_content_handler(h);
    }
    rewrite_str(html, settings).map_err(|e| {
        SsgError::io(
            std::io::Error::other(format!("lol_html rewrite failed: {e}")),
            "<lol_html>",
        )
    })
}

/// Rewrites every element with its attributes in name order.
///
/// Attribute order carries no meaning in HTML, but it does change the
/// bytes, so a golden-file suite sees a reordering as a diff even though
/// nothing observable changed. Sorting first makes those comparisons
/// stable — #466 asks for exactly this.
///
/// Real parsing via `lol_html` rather than scanning for `<` and quotes:
/// attribute values legitimately contain both, and hand-rolled tag
/// surgery over arbitrary HTML is the failure mode that put escaped
/// `<span>` tags on a published page in v0.0.58.
///
/// # Examples
///
/// ```rust
/// use ssg::util::html_rewriter::sort_attributes;
///
/// let out = sort_attributes(r#"<img src="a.png" alt="x" id="i">"#).unwrap();
/// assert_eq!(out, r#"<img alt="x" id="i" src="a.png">"#);
/// ```
///
/// # Errors
///
/// Appends `payload` immediately before the document's `</body>` end tag.
///
/// The counterpart to [`crate::util::head_dom::inject_before_head_close`],
/// and added for the same reason: five call sites across `islands`,
/// `view_transitions` and `search` were splicing at `html.rfind("</body>")`.
/// A byte search cannot tell the document's end tag from the characters
/// `</body>` sitting inside a comment, a script string or a `<pre>` block,
/// and when it picks the wrong one the payload lands somewhere inert with
/// nothing reporting it (ssg#570).
///
/// Only the first `</body>` in document order is used, so a page carrying a
/// nested document — the generator wraps an already-complete document in a
/// layout — gets one copy, in its own body, rather than one per match.
///
/// Returns the input unchanged when the document has no `<body>` or when the
/// rewrite fails, so callers keep whatever fallback they had.
///
/// # Examples
///
/// ```
/// use ssg::util::html_rewriter::inject_before_body_close;
/// let html = "<html><body><p>hi</p></body></html>";
/// let out = inject_before_body_close(html, "<script src=\"x.js\"></script>");
/// assert!(out.contains("</script></body>"));
/// ```
#[must_use]
pub fn inject_before_body_close(html: &str, payload: &str) -> String {
    if payload.is_empty() {
        return html.to_string();
    }

    let payload_owned = payload.to_string();
    let injected = std::rc::Rc::new(std::cell::Cell::new(false));
    let injected_cb = std::rc::Rc::clone(&injected);
    // Nesting depth at the moment each `<body>` opens. The outermost is the
    // document's own, and it is the one that must carry the payload.
    //
    // This is where `<body>` differs from `<head>`, and the difference is
    // easy to get backwards: an outer `<head>` closes *before* a nested
    // document begins, so "first end tag" is the document's own. An outer
    // `<body>` closes *after* the nested one, so "first end tag" is the
    // nested document's — injecting there puts the payload inside the
    // embedded page instead of the real one.
    let depth = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let depth_cb = std::rc::Rc::clone(&depth);

    let handler = element!("body", move |el| {
        let pl = payload_owned.clone();
        let cb = std::rc::Rc::clone(&injected_cb);
        let d = std::rc::Rc::clone(&depth_cb);
        let opened_at = d.get();
        d.set(opened_at + 1);
        let _ = el.on_end_tag(end_tag!(move |end| {
            d.set(d.get().saturating_sub(1));
            if opened_at == 0 && !cb.get() {
                end.before(&pl, lol_html::html_content::ContentType::Html);
                cb.set(true);
            }
            Ok(())
        }));
        Ok(())
    });

    let out =
        rewrite_html(html, vec![handler]).unwrap_or_else(|_| html.to_string());

    if injected.get() {
        out
    } else {
        html.to_string()
    }
}

/// Injects `payload` before `</body>`, appending it when there is none.
///
/// Six call sites across `islands`, `view_transitions` and `search` each
/// wrote this by hand as `rfind("</body>")` with an append fallback. The
/// fallback matters — HTML allows the end tag to be omitted, and the parser
/// fires no handler then — so it is stated once here rather than six times.
///
/// # Examples
///
/// ```
/// use ssg::util::html_rewriter::inject_before_body_close_or_append;
/// let out = inject_before_body_close_or_append("<html><body><p>x", "<i>y</i>");
/// assert!(out.ends_with("<i>y</i>"), "no </body>, so appended: {out}");
/// ```
#[must_use]
pub fn inject_before_body_close_or_append(html: &str, payload: &str) -> String {
    let injected = inject_before_body_close(html, payload);
    if injected == html {
        format!("{html}{payload}")
    } else {
        injected
    }
}

/// Returns [`SsgError::Io`] if `lol_html` fails to parse or rewrite.
pub fn sort_attributes(html: &str) -> Result<String, SsgError> {
    use lol_html::html_content::ContentType;

    rewrite_html(
        html,
        vec![lol_html::element!("*", |el| {
            let mut attrs: Vec<(String, String)> = el
                .attributes()
                .iter()
                .map(|a| (a.name(), a.value()))
                .collect();
            if attrs.is_empty() {
                return Ok(());
            }
            // Re-serialise even a single attribute. Minified HTML writes
            // `lang=en` where unminified writes `lang="en"`; going back
            // through lol_html quotes both the same way, so the output is
            // canonical rather than merely sorted.
            attrs.sort_by(|a, b| a.0.cmp(&b.0));

            // Remove then re-add: lol_html has no reorder primitive, and
            // set_attribute on an existing name overwrites in place
            // rather than moving it.
            let names: Vec<String> =
                attrs.iter().map(|(n, _)| n.clone()).collect();
            for n in &names {
                el.remove_attribute(n);
            }
            for (n, v) in &attrs {
                el.set_attribute(n, v).map_err(|e| {
                    SsgError::io(
                        std::io::Error::other(format!(
                            "set_attribute({n}): {e}"
                        )),
                        "<lol_html>",
                    )
                })?;
            }
            let _ = ContentType::Html;
            Ok(())
        })],
    )
}

/// Extract concatenated text content from every element matching
/// `selector`, separating successive elements with a single space.
///
/// Returns the **decoded** text — `&amp;` becomes `&`, `&lt;` becomes
/// `<`, and so on — matching the on-screen rendering rather than the
/// raw HTML bytes. This is the right behaviour for search-index
/// extraction (issue #525 AC4).
///
/// # Examples
///
/// ```rust
/// use ssg::util::html_rewriter::extract_text_with_filter;
///
/// let html = "<h1>Hello</h1><h1>World</h1>";
/// let texts = extract_text_with_filter(html, "h1").unwrap();
/// assert_eq!(texts, vec!["Hello".to_string(), "World".to_string()]);
/// ```
///
/// # Errors
///
/// Returns [`SsgError::Io`] if `lol_html` rejects the selector or
/// fails while parsing the input.
pub fn extract_text_with_filter(
    html: &str,
    selector: &str,
) -> Result<Vec<String>, SsgError> {
    use std::cell::RefCell;
    use std::rc::Rc;

    let parsed: Selector = selector.parse().map_err(|e| {
        SsgError::io(
            std::io::Error::other(format!(
                "invalid selector {selector:?}: {e}"
            )),
            "<lol_html>",
        )
    })?;

    // `Rc<RefCell<...>>` because `lol_html` handlers are owned closures
    // that outlive any local borrow scope.
    let buf: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let scratch: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    let scratch_for_text = Rc::clone(&scratch);
    let scratch_for_end = Rc::clone(&scratch);
    let buf_for_end = Rc::clone(&buf);

    let text_handler = move |t: &mut TextChunk<'_>| {
        scratch_for_text.borrow_mut().push_str(t.as_str());
        Ok(())
    };

    let element_handler =
        move |el: &mut lol_html::html_content::Element<'_, '_>| {
            let buf_for_handler = Rc::clone(&buf_for_end);
            let scratch_for_handler = Rc::clone(&scratch_for_end);
            let _ = el.on_end_tag(Box::new(move |_end| {
                let mut s = scratch_for_handler.borrow_mut();
                let decoded = decode_html_entities(s.trim());
                let collapsed = collapse_whitespace(&decoded);
                if !collapsed.is_empty() {
                    buf_for_handler.borrow_mut().push(collapsed);
                }
                s.clear();
                Ok(())
            }));
            Ok(())
        };

    let handlers = vec![
        (
            Cow::Owned(parsed.clone()),
            ElementContentHandlers::default().text(text_handler),
        ),
        (
            Cow::Owned(parsed),
            ElementContentHandlers::default().element(element_handler),
        ),
    ];

    let _ = rewrite_html(html, handlers)?;
    let result = buf.borrow().clone();
    Ok(result)
}

/// Minimal HTML entity decoder for text-extraction call sites.
///
/// Handles the five XML/HTML named references (`&amp;`, `&lt;`, `&gt;`,
/// `&quot;`, `&apos;`), decimal (`&#39;`) and hex (`&#x27;`) numeric
/// character references, plus `&nbsp;`. Unknown references pass through
/// verbatim so the output for non-canonical input is the least
/// surprising.
///
/// The set is deliberately narrow — search-index titles / headings /
/// body text only ever contain these forms in practice, and a full
/// HTML5-spec named-reference table would balloon binary size for no
/// observable benefit.
///
/// # Examples
///
/// ```rust
/// use ssg::util::html_rewriter::decode_html_entities;
///
/// assert_eq!(decode_html_entities("a &amp; b"), "a & b");
/// assert_eq!(decode_html_entities("&#65;"), "A");
/// assert_eq!(decode_html_entities("&unknown;"), "&unknown;");
/// ```
#[must_use]
pub fn decode_html_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            // Try to match a named or numeric reference up to the next ';'
            if let Some(rel_semi) = s[i..].find(';') {
                let entity = &s[i..=i + rel_semi];
                if let Some(decoded) = decode_one_entity(entity) {
                    out.push_str(&decoded);
                    i += rel_semi + 1;
                    continue;
                }
            }
        }
        // Walk a single UTF-8 scalar; the byte index `i` always sits on
        // a char boundary because we never split inside a multi-byte
        // sequence (we only advance past `&...;` runs which are ASCII).
        // `next()` always yields `Some` because the while-loop bound
        // (`i < bytes.len()`) guarantees at least one more byte — this
        // is a true invariant, not a defensive/fallible branch, so it
        // is asserted rather than handled as a distinct code path.
        let Some(ch) = s[i..].chars().next() else {
            // Unreachable per the invariant above; `unreachable!()`
            // (not `.expect()`) keeps this out of `clippy::expect_used`,
            // which this crate denies via `-D warnings` in CI.
            unreachable!("i < bytes.len() guarantees a next char")
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn decode_one_entity(entity: &str) -> Option<String> {
    // `entity` is `&...;` inclusive.
    let inner = entity.strip_prefix('&')?.strip_suffix(';')?;
    let decoded = match inner {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{00A0}',
        n if n.starts_with("#x") || n.starts_with("#X") => {
            let cp = u32::from_str_radix(&n[2..], 16).ok()?;
            char::from_u32(cp)?
        }
        n if n.starts_with('#') => {
            let cp = n[1..].parse::<u32>().ok()?;
            char::from_u32(cp)?
        }
        _ => return None,
    };
    Some(decoded.to_string())
}

/// Collapses runs of ASCII whitespace into a single space and trims.
///
/// Matches the historical `strip_tags` behaviour in
/// `src/plugins/search.rs` so the search index stays byte-identical
/// across the port.
///
/// # Examples
///
/// ```rust
/// use ssg::util::html_rewriter::collapse_whitespace;
///
/// assert_eq!(collapse_whitespace("  hello  world  "), "hello world");
/// assert_eq!(collapse_whitespace("a\tb\nc"), "a b c");
/// ```
#[must_use]
pub fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {

    /// HTML allows `</body>` to be omitted. The parser fires no end-tag
    /// handler then, so the helper returns its input and the caller's
    /// fallback still runs — which is what makes converting the `rfind`
    /// call sites safe rather than a silent behaviour change.
    #[test]
    fn body_injection_returns_input_when_the_end_tag_is_omitted() {
        let html = "<html><body><p>x</p>";
        assert_eq!(inject_before_body_close(html, "<i>p</i>"), html);
    }

    /// `rfind("</body>")` cannot tell the document's end tag from those
    /// characters inside a script string, and the payload lands inside the
    /// script where it is a syntax error rather than markup.
    #[test]
    fn body_injection_ignores_a_body_close_inside_a_script() {
        let html = concat!(
            "<html><body><p>hi</p>",
            "<script>var s = \"</body>\";</script>",
            "</body></html>"
        );
        let out = inject_before_body_close(html, "<span id=\"p\"></span>");

        let payload = out.find("<span id=\"p\">").expect("payload present");
        let script_end = out.find("</script>").expect("script survives");
        assert!(
            payload > script_end,
            "payload was injected inside the script:\n{out}"
        );
        assert_eq!(out.matches("<span id=\"p\">").count(), 1);
    }

    /// A page can carry a nested document; only its own body takes the
    /// payload.
    #[test]
    fn body_injection_targets_the_outermost_body() {
        let html = concat!(
            "<html><body><main>",
            "<html><body>nested</body></html>",
            "</main></body></html>"
        );
        let out = inject_before_body_close(html, "<i>x</i>");
        assert_eq!(out.matches("<i>x</i>").count(), 1, "{out}");
        // It must land in the page's own body, after the embedded document
        // has closed — not inside it. The nested `</body>` comes first in
        // document order, so "first end tag" would be the wrong one.
        let payload = out.find("<i>x</i>").expect("payload");
        let nested_close = out.find("nested</body>").expect("nested body");
        assert!(
            payload > nested_close,
            "payload landed inside the embedded document:\n{out}"
        );
    }

    /// No `<body>` at all: the caller keeps whatever fallback it had.
    #[test]
    fn body_injection_returns_input_when_there_is_no_body() {
        let html = "<html><head><title>T</title></head></html>";
        assert_eq!(inject_before_body_close(html, "<i>x</i>"), html);
    }
    /// Attribute values may legitimately contain `<`, `>` and quotes.
    /// A scanner looking for those characters mangles them; a parser
    /// does not. This is the case that makes the difference.
    #[test]
    fn sorting_preserves_values_containing_markup_characters() {
        let html = r#"<a title="1 < 2 &amp; 3 > 0" href="/x" id="q">t</a>"#;
        let out = sort_attributes(html).expect("sort");
        // `<` is left raw inside an attribute value: that is valid HTML
        // and lol_html does not gratuitously re-escape it. What matters
        // is that the value survives intact and is not treated as markup.
        assert!(out.contains(r#"title="1 < 2 &amp; 3 > 0""#), "{out}");
        assert!(
            out.find("href").unwrap() < out.find("id").unwrap()
                && out.find("id").unwrap() < out.find("title").unwrap(),
            "attributes not in name order: {out}"
        );
    }

    /// Multi-byte text must survive untouched.
    #[test]
    fn sorting_preserves_multibyte_text() {
        let html = r#"<p class="b" id="a">Ünïcödé — 日本語 ✓</p>"#;
        let out = sort_attributes(html).expect("sort");
        assert!(out.contains("Ünïcödé — 日本語 ✓"), "{out}");
        // class < id already, so ordering is unchanged; the point of this
        // test is that the multi-byte body survives the rewrite.
        assert!(out.contains(r#"<p class="b" id="a">"#), "{out}");
    }

    /// A single attribute, or none, is left exactly as it was.
    #[test]
    fn sorting_is_a_no_op_below_two_attributes() {
        for html in ["<p>x</p>", r#"<p id="a">x</p>"#] {
            assert_eq!(sort_attributes(html).expect("sort"), html);
        }
    }

    /// Sorting twice equals sorting once.
    #[test]
    fn sorting_attributes_is_idempotent() {
        let html = r#"<div data-z="1" alt="a" class="c" id="i">x</div>"#;
        let once = sort_attributes(html).expect("sort");
        let twice = sort_attributes(&once).expect("sort");
        assert_eq!(once, twice);
    }

    use super::*;
    use lol_html::element;

    #[test]
    fn rewrite_html_noop_returns_input() {
        let html = "<p>hello</p>";
        let out = rewrite_html(html, Vec::new()).unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn rewrite_html_replace_text_via_handler() {
        let html = "<p>hello <span>world</span></p>";
        let out = rewrite_html(
            html,
            vec![element!("span", |el| {
                el.set_inner_content(
                    "rust",
                    lol_html::html_content::ContentType::Text,
                );
                Ok(())
            })],
        )
        .unwrap();
        assert!(out.contains("rust"), "out={out}");
    }

    #[test]
    fn extract_text_with_filter_decodes_entities() {
        let html = "<title>My &amp; Title</title>";
        let texts = extract_text_with_filter(html, "title").unwrap();
        assert_eq!(texts, vec!["My & Title".to_string()]);
    }

    #[test]
    fn extract_text_with_filter_collapses_whitespace() {
        let html = "<h1>  hello   world  </h1>";
        let texts = extract_text_with_filter(html, "h1").unwrap();
        assert_eq!(texts, vec!["hello world".to_string()]);
    }

    #[test]
    fn extract_text_with_filter_skips_empty_elements() {
        let html = "<h2></h2><h2>real</h2>";
        let texts = extract_text_with_filter(html, "h2").unwrap();
        assert_eq!(texts, vec!["real".to_string()]);
    }

    #[test]
    fn extract_text_with_filter_invalid_selector_errors() {
        let err = extract_text_with_filter("<p></p>", ":::").unwrap_err();
        assert!(
            err.to_string().contains("invalid selector"),
            "selector parse failure should surface as an Io error \
             mentioning the selector, got: {err}"
        );
    }

    #[test]
    fn rewrite_html_maps_handler_failure_to_io_error() {
        // A content handler that fails aborts the rewrite; the wrapper
        // must map the `lol_html` error onto `SsgError::Io`.
        let err = rewrite_html(
            "<p>x</p>",
            vec![element!("p", |_el| Err("boom".into()))],
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("lol_html rewrite failed"),
            "handler failure should be wrapped, got: {err}"
        );
    }

    #[test]
    fn extract_text_with_filter_propagates_parser_ambiguity_error() {
        // `<xmp>` inside `<select>` is a documented lol_html parsing
        // ambiguity — the streaming parser cannot decide whether the
        // following bytes are raw text, so the rewrite fails and the
        // error must propagate through `extract_text_with_filter`.
        let html = "<h1>t</h1><select><xmp>a</xmp></select>";
        let err = extract_text_with_filter(html, "h1").unwrap_err();
        assert!(
            err.to_string().contains("lol_html rewrite failed"),
            "ambiguity should surface as a wrapped rewrite error, got: {err}"
        );
    }

    #[test]
    fn collapse_whitespace_basic() {
        assert_eq!(collapse_whitespace("  a  b  "), "a b");
        assert_eq!(collapse_whitespace(""), "");
    }

    // ── decode_html_entities ────────────────────────────────────────

    #[test]
    fn decode_html_entities_named_set() {
        assert_eq!(decode_html_entities("&amp;"), "&");
        assert_eq!(decode_html_entities("&lt;&gt;"), "<>");
        assert_eq!(decode_html_entities("&quot;"), "\"");
        assert_eq!(decode_html_entities("&apos;"), "'");
        assert_eq!(decode_html_entities("&nbsp;"), "\u{00A0}");
    }

    #[test]
    fn decode_html_entities_decimal_numeric_reference() {
        assert_eq!(decode_html_entities("&#39;"), "'");
        assert_eq!(decode_html_entities("&#65;"), "A");
    }

    #[test]
    fn decode_html_entities_hex_numeric_reference() {
        assert_eq!(decode_html_entities("&#x27;"), "'");
        assert_eq!(decode_html_entities("&#X41;"), "A");
    }

    #[test]
    fn decode_html_entities_unknown_named_passes_through() {
        // Unrecognised named reference must be preserved verbatim so
        // non-canonical input is left alone.
        let s = "&foo;";
        assert_eq!(decode_html_entities(s), s);
    }

    #[test]
    fn decode_html_entities_invalid_numeric_passes_through() {
        // u32 overflow / non-digit hex / out-of-range codepoint all
        // fall through.
        assert_eq!(decode_html_entities("&#xZZZZ;"), "&#xZZZZ;");
        assert_eq!(decode_html_entities("&#99999999999;"), "&#99999999999;");
        // 0xD800 is a surrogate — char::from_u32 returns None.
        assert_eq!(decode_html_entities("&#xD800;"), "&#xD800;");
        // Same surrogate rejection on the *decimal* reference path.
        assert_eq!(decode_html_entities("&#55296;"), "&#55296;");
    }

    #[test]
    fn decode_one_entity_rejects_malformed_delimiters() {
        // Missing leading `&` and missing trailing `;` hit the two
        // `strip_prefix`/`strip_suffix` early-outs directly.
        assert_eq!(decode_one_entity("amp;"), None);
        assert_eq!(decode_one_entity("&amp"), None);
    }

    #[test]
    fn decode_html_entities_amp_without_semicolon() {
        // A lone `&` with no matching `;` is treated as a literal.
        assert_eq!(decode_html_entities("a & b"), "a & b");
    }

    #[test]
    fn decode_html_entities_handles_multibyte_chars() {
        // Make sure the UTF-8 walker doesn't choke on multi-byte input.
        assert_eq!(decode_html_entities("café &amp; bar"), "café & bar");
    }

    #[test]
    fn decode_html_entities_empty_string() {
        assert_eq!(decode_html_entities(""), "");
    }

    // ── extract_text_with_filter additional coverage ────────────────

    #[test]
    fn extract_text_with_filter_handles_nested_elements() {
        let html = "<div><p>outer <span>inner</span> tail</p></div>";
        let texts = extract_text_with_filter(html, "p").unwrap();
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("outer"));
        assert!(texts[0].contains("inner"));
    }

    #[test]
    fn extract_text_with_filter_multiple_matches_returns_separate_strings() {
        let html = "<h2>one</h2><h2>two</h2><h2>three</h2>";
        let texts = extract_text_with_filter(html, "h2").unwrap();
        assert_eq!(
            texts,
            vec!["one".to_string(), "two".to_string(), "three".to_string(),]
        );
    }

    #[test]
    fn extract_text_with_filter_no_match_returns_empty() {
        let html = "<div>not a heading</div>";
        let texts = extract_text_with_filter(html, "h1").unwrap();
        assert!(texts.is_empty());
    }

    // ── collapse_whitespace edge cases ──────────────────────────────

    #[test]
    fn collapse_whitespace_tabs_and_newlines() {
        assert_eq!(collapse_whitespace("a\tb\nc\r\nd"), "a b c d");
    }

    #[test]
    fn collapse_whitespace_only_whitespace_returns_empty() {
        assert_eq!(collapse_whitespace("   \t\n   "), "");
    }
}
