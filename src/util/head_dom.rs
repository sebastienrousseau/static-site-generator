// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Parser-driven helpers for HTML `<head>` manipulation.
//!
//! Replaces the previous `str::find` / `str::replace` patterns scattered
//! across the SEO, `og_image`, `llm`, `ai`, `i18n`, `highlight`, `sbom`,
//! `atom`, `json_feed`, and `jsonld` plugins (issues #538, #539, #540).
//!
//! Three operations are exposed:
//!
//! - [`inject_before_head_close`] — append a payload immediately before
//!   the real `</head>` end-tag, never inside a `<pre>` / `<code>` /
//!   comment / `<script>` template that happens to contain the literal
//!   `</head>` string.
//! - [`extract_head_meta`] — single `lol_html` walk that pulls the
//!   document `<title>`, the `<html lang>` attribute, and the existing
//!   `<link rel="canonical">` href in one pass.
//! - [`remove_canonical_links`] — strip every
//!   `<link rel~="canonical">` in `<head>` without disturbing
//!   non-canonical `<link>` elements or matching literals embedded in
//!   `<pre>` blocks.
//!
//! All three guard against the standard `str::find` failure modes:
//! comments containing the literal markup, `<pre>` / `<code>` samples
//! that quote the same tag, attribute order and quoting variants, and
//! multiple matches in body content.

use crate::util::html_rewriter::rewrite_html;
use lol_html::html_content::ContentType;
use lol_html::{element, end_tag, text};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Metadata extracted from the document `<head>` in a single parser
/// walk.
///
/// All fields default to the empty string when the corresponding markup
/// is missing — matching the historical `str::find`-based helpers in
/// `src/plugins/seo/helpers.rs` so call sites can swap in without
/// branch-equivalence regressions.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HeadMeta {
    /// Plain-text content of the first `<title>` element, with inner
    /// tags stripped and whitespace collapsed. Empty if no `<title>`.
    pub title: String,
    /// Value of `<html lang="…">`. Empty if absent.
    pub lang: String,
    /// `href` of the first `<link rel="canonical">`. Empty if absent.
    pub canonical: String,
}

/// Inserts `payload` immediately before the real `</head>` end-tag.
///
/// Returns the input unchanged when no `<head>` element exists or when
/// the underlying `lol_html` rewrite fails (allocation exhaustion is the
/// only documented failure mode). `payload` is treated as raw HTML so
/// callers retain control over tag construction; callers that need
/// escaping must do so themselves.
///
/// Idempotency is the **caller's** responsibility — this helper inserts
/// unconditionally and will append a second payload on a second call.
///
/// # Examples
///
/// ```
/// use ssg::util::head_dom::inject_before_head_close;
/// let html = "<html><head><title>T</title></head><body></body></html>";
/// let out = inject_before_head_close(html, "<meta name=\"x\">");
/// assert!(out.contains("<meta name=\"x\"></head>"));
/// ```
#[must_use]
pub fn inject_before_head_close(html: &str, payload: &str) -> String {
    if payload.is_empty() {
        return html.to_string();
    }

    let payload_owned = payload.to_string();
    let injected = Rc::new(Cell::new(false));
    let injected_cb = Rc::clone(&injected);

    let handler = element!("head", move |el| {
        let pl = payload_owned.clone();
        let cb = Rc::clone(&injected_cb);
        let _ = el.on_end_tag(end_tag!(move |end| {
            // Only the first `</head>` in document order, which is the
            // document's own. A page can carry a second, nested one: the
            // generator wraps an already-complete document in a layout, so
            // `<main>` holds a whole `<!DOCTYPE html>…<head>…` of its own.
            // Injecting into every match put a copy of the payload inside
            // `<body>` - a second canonical link, a second stylesheet, and
            // on one example the only copy of the JSON-LD.
            if !cb.get() {
                end.before(&pl, ContentType::Html);
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

/// Decodes the entities an HTML escaper produces, so `title` is genuinely the
/// plain text this module documents it to be.
///
/// `<title>` arrives already escaped — it came out of rendered HTML. Every
/// consumer treats the returned value as plain text and escapes it again on
/// the way out, so without decoding here a title containing `&` round-trips
/// to `&amp;amp;` and renders as a literal `&amp;`.
///
/// That is not hypothetical: it reached 1,494 of 6,856 pages on
/// sebastienrousseau.com, in `og:title` and `twitter:title` — every social
/// preview of a page whose title contains an ampersand. `og:description`
/// escaped correctly on the same pages, because `extract_description` routes
/// through `strip_tags`, which drops entities; only the title kept them. That
/// asymmetry is what made it look like an escaping bug rather than a decoding
/// one.
///
/// `&amp;` is decoded last. Doing it first would turn `&amp;lt;` into `&lt;`
/// and then into `<`, re-introducing the injection the escaping prevented.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Extracts `{title, lang, canonical}` from `html` in a single
/// `lol_html` pass.
///
/// - `title` is the text content of the first `<title>` element with
///   inner tags stripped (matching the historical
///   `extract_title` behaviour). Whitespace is collapsed and trimmed;
///   returns the empty string when the element is missing or its text
///   content is whitespace-only.
/// - `lang` is the `lang` attribute on the root `<html>` element.
///   `<html lang="…">` inside a `<pre>` block is ignored because
///   `lol_html` only matches the real element.
/// - `canonical` is the `href` attribute of the first
///   `<link rel~="canonical">` in `<head>`. Selector matches the
///   space-separated token set so `rel="canonical other-token"` is
///   detected, while quoting style is irrelevant (the parser normalises
///   it).
///
/// # Examples
///
/// ```
/// use ssg::util::head_dom::extract_head_meta;
/// let html = r#"<html lang="en"><head><title>Hi</title></head></html>"#;
/// let meta = extract_head_meta(html);
/// assert_eq!(meta.title, "Hi");
/// assert_eq!(meta.lang, "en");
/// ```
#[must_use]
pub fn extract_head_meta(html: &str) -> HeadMeta {
    let title_buf: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let title_done: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let lang: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let canonical: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    let title_buf_text = Rc::clone(&title_buf);
    let title_done_text = Rc::clone(&title_done);
    let title_text_handler = text!("title", move |t| {
        if !title_done_text.get() {
            title_buf_text.borrow_mut().push_str(t.as_str());
            if t.last_in_text_node() {
                title_done_text.set(true);
            }
        }
        Ok(())
    });

    let lang_cb = Rc::clone(&lang);
    let html_handler = element!("html", move |el| {
        if let Some(value) = el.get_attribute("lang") {
            *lang_cb.borrow_mut() = value;
        }
        Ok(())
    });

    let canonical_cb = Rc::clone(&canonical);
    let canonical_handler = element!("link[rel~=\"canonical\" i]", move |el| {
        if canonical_cb.borrow().is_empty() {
            if let Some(href) = el.get_attribute("href") {
                *canonical_cb.borrow_mut() = href;
            }
        }
        Ok(())
    });

    let _ = rewrite_html(
        html,
        vec![title_text_handler, html_handler, canonical_handler],
    );

    let raw_title = title_buf.borrow().clone();
    let title =
        decode_entities(&collapse_ws(strip_tags(raw_title.trim()).trim()));
    let lang_val = lang.borrow().clone();
    let canonical_val = canonical.borrow().clone();

    HeadMeta {
        title,
        lang: lang_val,
        canonical: canonical_val,
    }
}

/// Removes every `<link rel~="canonical">` element from the document.
///
/// Uses the `link[rel~="canonical" i]` selector so any attribute order
/// or quoting is handled by the parser, including `rel="canonical
/// other-token"` (the space-separated token set match). Literal
/// `<link rel="canonical">` text embedded inside a `<pre>` / `<code>`
/// block or an HTML comment is left untouched because `lol_html` only
/// dispatches on real elements.
///
/// # Examples
///
/// ```
/// use ssg::util::head_dom::remove_canonical_links;
/// let html = r#"<head><link rel="canonical" href="/old"></head>"#;
/// let out = remove_canonical_links(html);
/// assert!(!out.contains("canonical"));
/// ```
#[must_use]
pub fn remove_canonical_links(html: &str) -> String {
    let handler = element!("link[rel~=\"canonical\" i]", |el| {
        el.remove();
        Ok(())
    });
    rewrite_html(html, vec![handler]).unwrap_or_else(|_| html.to_string())
}

/// Replaces every existing `<link rel~="canonical">` with a single
/// `payload` injected just before `</head>`, in one parser pass.
///
/// Equivalent to `inject_before_head_close(&remove_canonical_links(html),
/// payload)` but executed in a single `lol_html` walk so that the
/// surrounding whitespace residue from the removal is consumed at the
/// same time as the new link is inserted — keeping the canonical
/// plugin's `transform_html` byte-stable across repeated invocations
/// (idempotency requirement).
///
/// # Examples
///
/// ```
/// use ssg::util::head_dom::replace_canonical_link;
/// let html = r#"<html><head><link rel="canonical" href="/old"></head></html>"#;
/// let out = replace_canonical_link(html, r#"<link rel="canonical" href="/new">"#);
/// assert!(out.contains("href=\"/new\""));
/// assert!(!out.contains("href=\"/old\""));
/// ```
#[must_use]
pub fn replace_canonical_link(html: &str, payload: &str) -> String {
    let payload_owned = payload.to_string();
    let injected = Rc::new(Cell::new(false));
    let injected_cb = Rc::clone(&injected);

    let canonical_handler = element!("link[rel~=\"canonical\" i]", |el| {
        el.remove();
        Ok(())
    });

    let head_handler = element!("head", move |el| {
        let pl = payload_owned.clone();
        let cb = Rc::clone(&injected_cb);
        let _ = el.on_end_tag(end_tag!(move |end| {
            end.before(&pl, ContentType::Html);
            cb.set(true);
            Ok(())
        }));
        Ok(())
    });

    let out = rewrite_html(html, vec![canonical_handler, head_handler])
        .unwrap_or_else(|_| html.to_string());

    if injected.get() {
        out
    } else {
        html.to_string()
    }
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn collapse_ws(s: &str) -> String {
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
mod entity_decode_tests {
    use super::*;

    /// The regression: a title with `&` must survive one escape, not two.
    ///
    /// `<title>` comes out of rendered HTML already escaped. Consumers treat
    /// the extracted value as plain text and re-escape it, so leaving the
    /// entity in place produced `&amp;amp;` — a literal `&amp;` on the page.
    /// 1,494 of 6,856 pages on sebastienrousseau.com shipped that in
    /// `og:title` and `twitter:title`.
    #[test]
    fn extracted_title_is_plain_text_not_escaped_markup() {
        let html = "<html><head><title>AI, Payments &amp; Post-Quantum</title></head></html>";
        assert_eq!(
            extract_head_meta(html).title,
            "AI, Payments & Post-Quantum"
        );
    }

    /// Guards the decode order. `&amp;` must be decoded LAST: doing it first
    /// turns `&amp;lt;` into `&lt;` and then into `<`, re-introducing exactly
    /// the injection the escaping existed to prevent.
    #[test]
    fn decoding_does_not_resurrect_escaped_markup() {
        let html =
            "<html><head><title>Tricky &amp;lt;script&amp;gt; name</title></head></html>";
        let title = extract_head_meta(html).title;
        assert_eq!(title, "Tricky &lt;script&gt; name");
        assert!(
            !title.contains("<script>"),
            "decoding must not turn an escaped tag back into markup: {title:?}"
        );
    }

    #[test]
    fn all_escaper_entities_round_trip() {
        let html = "<html><head><title>a &lt;b&gt; &quot;c&quot; &#39;d&#39; &amp; e</title></head></html>";
        assert_eq!(extract_head_meta(html).title, r#"a <b> "c" 'd' & e"#);
    }

    #[test]
    fn titles_without_entities_are_untouched() {
        let html = "<html><head><title>Plain Title</title></head></html>";
        assert_eq!(extract_head_meta(html).title, "Plain Title");
    }
}

#[cfg(test)]
mod tests {

    /// A page can contain a second, nested `<head>`: the generator wraps an
    /// already-complete document inside a layout, so `<main>` holds a whole
    /// `<!DOCTYPE html>…<head>…</head>…` of its own. Injecting into every
    /// `<head>` puts a copy of the payload inside `<body>`.
    ///
    /// Measured on the plugins example, where twelve plugins use this helper:
    /// two `/highlight.css` links, three canonical links, six alternate
    /// links, ten Open Graph meta — with one of each pair, and the only copy
    /// of the JSON-LD, landing in the body. Duplicate canonicals are worse
    /// than useless; a search engine may honour neither.
    #[test]
    fn injection_targets_the_document_head_not_every_head() {
        let html = concat!(
            "<html><head><title>Outer</title></head><body><main>",
            "<html><head><title>Nested</title></head><body>x</body></html>",
            "</main></body></html>"
        );
        let out = inject_before_head_close(html, "<link rel=\"x\">");

        assert_eq!(
            out.matches("<link rel=\"x\">").count(),
            1,
            "payload was injected more than once:\n{out}"
        );
        let body_at = out.find("<body").expect("a body");
        assert!(
            out.find("<link rel=\"x\">").expect("the payload") < body_at,
            "payload landed inside <body>:\n{out}"
        );
    }

    /// ssg#540: a `</head>` that appears inside a comment or a script in the
    /// head is not the head's end tag. A `find("</head>")` splice takes the
    /// first byte match and injects *inside* that comment, where the payload
    /// is inert — silently, because the output still looks like valid HTML.
    #[test]
    fn injection_ignores_a_head_close_inside_a_comment() {
        let html = concat!(
            "<html><head>",
            "<!-- </head> -->",
            "<title>T</title>",
            "</head><body></body></html>"
        );
        let out = inject_before_head_close(html, "<meta name=\"x\">");

        let naive = html.find("</head>").unwrap();
        let real = out.find("<meta name=\"x\">").unwrap();
        assert!(
            real > naive,
            "payload landed at the commented-out tag, not the real one: {out}"
        );
        assert!(
            out.contains("<meta name=\"x\"></head>"),
            "payload should sit immediately before the real close: {out}"
        );
    }

    /// The same hazard written as a script body rather than a comment.
    #[test]
    fn injection_ignores_a_head_close_inside_a_script() {
        let html = concat!(
            "<html><head>",
            "<script>var s = \"</head>\";</script>",
            "</head><body></body></html>"
        );
        let out = inject_before_head_close(html, "<meta name=\"y\">");
        assert!(
            out.contains("<meta name=\"y\"></head>"),
            "payload should sit before the real close: {out}"
        );
    }

    use super::*;

    // ── inject_before_head_close ────────────────────────────────────

    #[test]
    fn inject_at_real_head_close() {
        let html = "<html><head><title>T</title></head><body></body></html>";
        let out = inject_before_head_close(html, "<meta name=\"x\">");
        assert!(out.contains("<meta name=\"x\"></head>"));
        assert_eq!(out.matches("<meta name=\"x\">").count(), 1);
    }

    #[test]
    fn inject_skips_pre_block_literal() {
        let html = "<html><head><title>T</title></head>\
                    <body><pre>&lt;/head&gt;</pre></body></html>";
        let out = inject_before_head_close(html, "<meta name=\"x\">");
        assert_eq!(out.matches("<meta name=\"x\">").count(), 1);
        assert!(out.contains("<pre>&lt;/head&gt;</pre>"));
    }

    #[test]
    fn inject_skips_comment_literal() {
        let html =
            "<html><head><title>T</title></head><body><!-- </head> --></body></html>";
        let out = inject_before_head_close(html, "<meta name=\"x\">");
        assert_eq!(out.matches("<meta name=\"x\">").count(), 1);
        assert!(out.contains("<!-- </head> -->"));
    }

    #[test]
    fn inject_returns_input_when_no_head() {
        let html = "<html><body>no head</body></html>";
        let out = inject_before_head_close(html, "<meta>");
        assert_eq!(out, html);
    }

    #[test]
    fn inject_empty_payload_returns_input() {
        let html = "<html><head></head></html>";
        let out = inject_before_head_close(html, "");
        assert_eq!(out, html);
    }

    // ── extract_head_meta ───────────────────────────────────────────

    #[test]
    fn extract_title_from_real_title_not_comment() {
        let html = "<html><head><!-- <title>Old</title> --><title>Real</title></head></html>";
        let meta = extract_head_meta(html);
        assert_eq!(meta.title, "Real");
    }

    #[test]
    fn extract_lang_from_html_not_pre() {
        let html = "<html lang=\"en-GB\"><head></head>\
                    <body><pre>&lt;html lang=\"fr\"&gt;</pre></body></html>";
        let meta = extract_head_meta(html);
        assert_eq!(meta.lang, "en-GB");
    }

    #[test]
    fn extract_canonical_returns_href() {
        let html = r#"<html><head><link rel="canonical" href="https://x"></head></html>"#;
        let meta = extract_head_meta(html);
        assert_eq!(meta.canonical, "https://x");
    }

    #[test]
    fn extract_returns_defaults_when_absent() {
        let html = "<html><head></head><body></body></html>";
        let meta = extract_head_meta(html);
        assert!(meta.title.is_empty());
        assert!(meta.lang.is_empty());
        assert!(meta.canonical.is_empty());
    }

    #[test]
    fn extract_collapses_title_whitespace() {
        let html = "<html><head><title>  Hello   World  </title></head></html>";
        let meta = extract_head_meta(html);
        assert_eq!(meta.title, "Hello World");
    }

    // ── remove_canonical_links ──────────────────────────────────────

    #[test]
    fn remove_double_quoted_canonical() {
        let html = r#"<head><link rel="canonical" href="/old"><title>x</title></head>"#;
        let out = remove_canonical_links(html);
        assert!(!out.contains("rel=\"canonical\""));
        assert!(out.contains("<title>x</title>"));
    }

    #[test]
    fn remove_single_quoted_canonical() {
        let html = "<head><link rel='canonical' href='/old'></head>";
        let out = remove_canonical_links(html);
        assert!(!out.contains("canonical"));
    }

    #[test]
    fn remove_unquoted_canonical() {
        let html = "<head><link rel=canonical href=/old></head>";
        let out = remove_canonical_links(html);
        assert!(!out.contains("canonical"));
    }

    #[test]
    fn remove_keeps_non_canonical_link() {
        let html = r#"<head><link rel="stylesheet" href="/x.css"></head>"#;
        let out = remove_canonical_links(html);
        assert_eq!(out, html);
    }

    #[test]
    fn remove_multiple_canonicals() {
        let html = r#"<head><link rel="canonical" href="/a"><link rel="canonical" href="/b"></head>"#;
        let out = remove_canonical_links(html);
        assert!(!out.contains("canonical"));
    }

    #[test]
    fn remove_leaves_pre_literal_untouched() {
        let html = "<html><head></head>\
                    <body><pre>&lt;link rel=\"canonical\"&gt;</pre></body></html>";
        let out = remove_canonical_links(html);
        assert!(out.contains("<pre>&lt;link rel=\"canonical\"&gt;</pre>"));
    }

    // ── replace_canonical_link ───────────────────────────────────────

    #[test]
    fn replace_canonical_removes_old_and_injects_new() {
        let html = r#"<html><head><title>T</title><link rel="canonical" href="/old"></head><body></body></html>"#;
        let payload = r#"<link rel="canonical" href="/new">"#;
        let out = replace_canonical_link(html, payload);
        assert!(out.contains("href=\"/new\""));
        assert!(!out.contains("href=\"/old\""));
        // Only one canonical present after replacement.
        assert_eq!(out.matches("rel=\"canonical\"").count(), 1);
    }

    #[test]
    fn replace_canonical_injects_when_none_existed() {
        let html = "<html><head><title>T</title></head><body></body></html>";
        let payload = r#"<link rel="canonical" href="/new">"#;
        let out = replace_canonical_link(html, payload);
        assert!(out.contains("href=\"/new\""));
        // Payload sits just before </head>.
        assert!(out.contains("href=\"/new\"></head>"));
    }

    #[test]
    fn replace_canonical_returns_input_when_no_head() {
        // No <head> element => injector callback never fires => function
        // returns the original string unchanged.
        let html = "<html><body>nothing here</body></html>";
        let payload = r#"<link rel="canonical" href="/new">"#;
        let out = replace_canonical_link(html, payload);
        assert_eq!(out, html);
    }

    #[test]
    fn replace_canonical_is_idempotent() {
        let html = r#"<html><head><title>T</title><link rel="canonical" href="/a"></head></html>"#;
        let payload = r#"<link rel="canonical" href="/a">"#;
        let once = replace_canonical_link(html, payload);
        let twice = replace_canonical_link(&once, payload);
        // After two passes, still exactly one canonical and same href.
        assert_eq!(twice.matches("rel=\"canonical\"").count(), 1);
        assert!(twice.contains("href=\"/a\""));
    }

    // ── inject_before_head_close: exercise multiple <head> rewrite ──

    #[test]
    fn inject_handles_head_with_existing_children() {
        // Already-populated <head> with mixed children — inject must
        // preserve every prior element AND append the payload exactly
        // once just before </head>.
        let html = "<html><head><meta charset=\"utf-8\"><title>X</title>\
                    <link rel=\"stylesheet\" href=\"/a.css\"></head><body>b</body></html>";
        let out =
            inject_before_head_close(html, "<script src=\"/x.js\"></script>");
        assert!(out.contains("<meta charset=\"utf-8\">"));
        assert!(out.contains("<title>X</title>"));
        assert!(out.contains("<link rel=\"stylesheet\" href=\"/a.css\">"));
        assert!(out.contains("<script src=\"/x.js\"></script></head>"));
        assert_eq!(out.matches("<script src=\"/x.js\">").count(), 1);
    }

    #[test]
    fn remove_canonical_with_mixed_case_rel_value() {
        // Selector uses the case-insensitive flag — rel="CANONICAL" must
        // still match.
        let html = r#"<head><link rel="Canonical" href="/x"></head>"#;
        let out = remove_canonical_links(html);
        assert!(!out.contains("Canonical"));
    }

    // ── lol_html failure fallbacks ──────────────────────────────────
    //
    // `<xmp>` inside `<select>` is a documented lol_html parsing
    // ambiguity: the rewrite fails and every helper must fall back to
    // returning the input unchanged.

    const AMBIGUOUS: &str = "<html><head><title>T</title></head>\
                             <body><select><xmp>x</xmp></select></body></html>";

    #[test]
    fn inject_returns_input_when_rewrite_fails() {
        let out = inject_before_head_close(AMBIGUOUS, "<meta name=\"x\">");
        assert_eq!(out, AMBIGUOUS);
    }

    #[test]
    fn remove_canonical_returns_input_when_rewrite_fails() {
        let html = "<head><link rel=\"canonical\" href=\"/old\"></head>\
                    <select><xmp>x</xmp></select>";
        let out = remove_canonical_links(html);
        assert_eq!(out, html);
    }

    #[test]
    fn replace_canonical_returns_input_when_rewrite_fails() {
        let out = replace_canonical_link(
            AMBIGUOUS,
            "<link rel=\"canonical\" href=\"/new\">",
        );
        assert_eq!(out, AMBIGUOUS);
    }

    // ── extract_head_meta edge shapes ───────────────────────────────

    #[test]
    fn extract_title_uses_first_title_element_only() {
        // A second <title> must not overwrite or append to the first —
        // the `title_done` latch discards its text chunks.
        let html = "<head><title>First</title><title>Second</title></head>";
        let meta = extract_head_meta(html);
        assert_eq!(meta.title, "First");
    }

    #[test]
    fn extract_canonical_skips_link_without_href() {
        // First canonical carries no href — the handler leaves the slot
        // empty so the following canonical is captured instead.
        let html = "<head><link rel=\"canonical\">\
                    <link rel=\"canonical\" href=\"/real\"></head>";
        let meta = extract_head_meta(html);
        assert_eq!(meta.canonical, "/real");
    }

    #[test]
    fn extract_canonical_keeps_first_of_multiple_hrefs() {
        let html = "<head><link rel=\"canonical\" href=\"/first\">\
                    <link rel=\"canonical\" href=\"/second\"></head>";
        let meta = extract_head_meta(html);
        assert_eq!(meta.canonical, "/first");
    }
}
