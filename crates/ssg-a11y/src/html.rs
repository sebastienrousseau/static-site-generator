// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hand-rolled HTML tag/attribute helpers.
//!
//! No parsing crate is used deliberately — these operate on raw byte/char
//! slices so the crate stays dependency-free and trivially embeddable in
//! any build pipeline.

/// Returns `true` if the `<img>` tag has any form of `alt` attribute.
pub(crate) fn has_valid_alt(tag: &str) -> bool {
    let has_alt_eq = tag.contains("alt=");
    let has_alt_bare = !has_alt_eq
        && (tag.contains(" alt ")
            || tag.contains(" alt>")
            || tag.ends_with(" alt"));
    has_alt_eq || has_alt_bare
}

/// Returns `true` if the `<img>` tag has an empty or missing-value alt.
pub(crate) fn has_empty_alt(tag: &str) -> bool {
    let has_alt_eq = tag.contains("alt=");
    let has_alt_bare = !has_alt_eq
        && (tag.contains(" alt ")
            || tag.contains(" alt>")
            || tag.ends_with(" alt"));
    // An unquoted value (`alt=photo`, as minifiers write it) is alt
    // text; only `alt=` with nothing after it is empty.
    tag.contains("alt=\"\"")
        || tag.contains("alt=''")
        || has_alt_bare
        || (has_alt_eq
            && extract_attr_value(tag, "alt").is_some_and(|v| v.is_empty()))
}

/// Returns `true` if the `<img>` tag is marked as decorative: an ARIA
/// role of `presentation` or `none`, or `aria-hidden="true"`, which
/// removes the image from the accessibility tree. Any quoting style.
pub(crate) fn is_decorative_img(tag: &str) -> bool {
    let role = extract_attr_value(tag, "role");
    matches!(role.as_deref(), Some("presentation" | "none"))
        || extract_attr_value(tag, "aria-hidden").as_deref() == Some("true")
}

/// Returns `true` if an `<img>` tag has no text alternative: no `alt`
/// attribute at all, or an empty one on an image that is not marked
/// decorative (see [WCAG 1.1.1]).
///
/// Expects the tag with ASCII-lowercased attribute names, from `<img` to
/// its closing `>` (see [`find_tag_end`]).
///
/// [WCAG 1.1.1]: https://www.w3.org/WAI/WCAG22/Understanding/non-text-content
#[must_use]
pub fn img_missing_alt(tag: &str) -> bool {
    !has_valid_alt(tag) || (has_empty_alt(tag) && !is_decorative_img(tag))
}

/// Returns the index one past the closing `>` of the tag at `tag_start`.
///
/// Skips `>` characters inside double- or single-quoted attribute values, so
/// an inline SVG `data:` URL in a `src` attribute cannot truncate the tag
/// prematurely.
// `const` because clippy::missing_const_for_fn asks for it under Rust
// 1.98, where the lint learned to see through this loop. Nothing about
// the body changed — it was already const-compatible.
pub const fn find_tag_end(html: &str, tag_start: usize) -> usize {
    let bytes = html.as_bytes();
    let mut i = tag_start;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'>' => return i + 1,
                _ => {}
            },
        }
        i += 1;
    }
    bytes.len()
}

/// Extracts an attribute value from an HTML tag string.
pub(crate) fn extract_attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let pattern = format!("{attr}=");
    let start = lower.find(&pattern)?;
    let after = &tag[start + pattern.len()..];
    let trimmed = after.trim_start();
    if let Some(inner) = trimmed.strip_prefix('"') {
        let end = inner.find('"')?;
        Some(inner[..end].to_string())
    } else if let Some(inner) = trimmed.strip_prefix('\'') {
        let end = inner.find('\'')?;
        Some(inner[..end].to_string())
    } else {
        let end = trimmed
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(trimmed.len());
        Some(trimmed[..end].to_string())
    }
}

/// Simple tag stripper for checking inner text.
pub(crate) fn strip_tags_simple(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_attr_value_double_quoted() {
        let result = extract_attr_value(r#"<a href="/foo">"#, "href");
        assert_eq!(result, Some("/foo".to_string()));
    }

    #[test]
    fn extract_attr_value_single_quoted() {
        // The single-quote branch.
        let result = extract_attr_value(r"<a href='/bar'>", "href");
        assert_eq!(result, Some("/bar".to_string()));
    }

    #[test]
    fn extract_attr_value_unquoted() {
        // The no-quote fallback branch, terminated by whitespace or `>`.
        let result = extract_attr_value(r"<a href=/baz>", "href");
        assert_eq!(result, Some("/baz".to_string()));
    }

    #[test]
    fn extract_attr_value_missing_attribute_returns_none() {
        let result = extract_attr_value(r"<a>", "href");
        assert!(result.is_none());
    }

    #[test]
    fn strip_tags_simple_removes_html_tags_and_preserves_text() {
        let result = strip_tags_simple("<p>hello <b>world</b>!</p>");
        assert_eq!(result, "hello world!");
    }

    #[test]
    fn strip_tags_simple_handles_empty_and_text_only() {
        assert_eq!(strip_tags_simple(""), "");
        assert_eq!(strip_tags_simple("plain text"), "plain text");
    }

    #[test]
    fn has_empty_alt_detects_bare_alt_attribute() {
        // `<img alt>` — valueless alt is "present" but empty.
        assert!(has_empty_alt("<img src=x alt>"));
        assert!(has_empty_alt("<img alt src=x>"));
        // Truncated tag ending exactly in ` alt`.
        assert!(has_empty_alt("<img src=x alt"));
    }

    #[test]
    fn has_empty_alt_detects_unquoted_missing_value() {
        // `alt=` followed by neither quote style — treated as empty.
        assert!(has_empty_alt("<img alt=>"));
        // A single-quoted non-empty value is NOT empty.
        assert!(!has_empty_alt("<img alt='photo'>"));
    }

    #[test]
    fn is_decorative_img_covers_all_role_spellings() {
        assert!(is_decorative_img("<img role=\"presentation\">"));
        assert!(is_decorative_img("<img role=\"none\">"));
        assert!(is_decorative_img("<img role='presentation'>"));
        assert!(is_decorative_img("<img role='none'>"));
        assert!(is_decorative_img("<img role=presentation>"));
        assert!(is_decorative_img("<img role=none>"));
        assert!(!is_decorative_img("<img role=\"img\">"));
    }

    #[test]
    fn is_decorative_img_accepts_aria_hidden_true() {
        // aria-hidden="true" removes the image from the accessibility
        // tree, so alt="" on it marks a decorative image.
        assert!(is_decorative_img("<img aria-hidden=\"true\">"));
        assert!(is_decorative_img("<img aria-hidden='true'>"));
        assert!(is_decorative_img("<img aria-hidden=true>"));
        assert!(!is_decorative_img("<img aria-hidden=\"false\">"));
        assert!(!is_decorative_img("<img src=\"a.png\">"));
    }

    #[test]
    fn has_empty_alt_accepts_unquoted_value() {
        // Minifiers drop the quotes around a one-word alt.
        assert!(!has_empty_alt("<img alt=photo src=x>"));
        assert!(!has_empty_alt("<img src=x alt=photo>"));
    }

    #[test]
    fn find_tag_end_without_closing_bracket_returns_len() {
        let html = "<img src=\"unterminated";
        assert_eq!(find_tag_end(html, 0), html.len());
    }

    #[test]
    fn test_extract_attr_value_quoting_styles() {
        assert_eq!(
            extract_attr_value("<img alt=\"hello\">", "alt"),
            Some("hello".to_string())
        );
        assert_eq!(
            extract_attr_value("<img alt='single'>", "alt"),
            Some("single".to_string())
        );
        assert_eq!(
            extract_attr_value("<img alt=unquoted>", "alt"),
            Some("unquoted".to_string())
        );
        assert_eq!(
            extract_attr_value("<img alt=unquoted-space class=x>", "alt"),
            Some("unquoted-space".to_string())
        );
        // Missing closing quotes
        assert_eq!(extract_attr_value("<img alt=\"unclosed>", "alt"), None);
        assert_eq!(extract_attr_value("<img alt='unclosed>", "alt"), None);
        // Attribute not found
        assert_eq!(extract_attr_value("<img alt=\"hello\">", "width"), None);
    }
}
