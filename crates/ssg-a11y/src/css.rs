// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Minimal inline-`<style>` CSS preprocessor used by the WCAG 2.2
//! target-size and focus-appearance checks.
//!
//! The previous, more naive approach parsed only the *first* `<style>`
//! block, did not strip `/* ... */` comments, and did not skip rules
//! nested inside `@media` / `@supports`. Common false positives:
//!
//! ```text
//! /* width: 10px */     — flagged as a 2.5.8 violation
//! @media print { button { width: 10px } }
//!                       — flagged as a 2.5.8 violation though
//!                         the rule only applies on print
//! ```
//!
//! [`extract_all_style_blocks`] + [`preprocess_css`] +
//! [`parse_top_level_rules`] fix all three. They lowercase as they go so
//! downstream checks continue to do case-insensitive matching.

use crate::html::find_tag_end;

/// Returns the inner CSS of every `<style>...</style>` block in the
/// document. Tolerant of `<style data-foo="bar">` and other
/// attribute-bearing forms (uses `find_tag_end` so quoted `>` inside
/// attribute values doesn't truncate the open tag).
pub(crate) fn extract_all_style_blocks(html: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;

    while let Some(rel_open) = lower[cursor..].find("<style") {
        let abs_open = cursor + rel_open;
        let tag_end = find_tag_end(&lower, abs_open);
        cursor = tag_end;

        let Some(rel_close) = lower[cursor..].find("</style>") else {
            break;
        };
        // Use the original-case slice (we lowercase later in
        // `preprocess_css` so case-sensitive selectors don't get
        // inadvertently normalised before extraction).
        blocks.push(html[cursor..cursor + rel_close].to_string());
        cursor += rel_close + "</style>".len();
    }

    blocks
}

/// CSS preprocessor: lowercases, strips `/* ... */` comments, and
/// removes the body of any `@media` / `@supports` / `@keyframes`
/// at-rule. The returned string contains only the top-level rules
/// that apply unconditionally to every viewport.
///
/// At-rule body removal is brace-balanced — it correctly skips over
/// nested blocks inside `@supports (display: grid) { @media (prefers
/// ...) { ... } }`. The at-rule's own preamble (the `@supports (...)`
/// part) is dropped along with its body.
pub(crate) fn preprocess_css(css: &str) -> String {
    let lower = css.to_ascii_lowercase();
    let no_comments = strip_css_comments(&lower);
    strip_at_rules(&no_comments)
}

/// Replaces every `/* ... */` block comment with a single space.
pub(crate) fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && &bytes[i..i + 2] == b"/*" {
            // Skip until the matching `*/`.
            i += 2;
            while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            // Replace the comment with a single space so adjacent
            // tokens don't accidentally merge (`/*x*/y` → ` y`).
            out.push(' ');
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Removes every `@`-rule (preamble + brace-balanced body, or a bare
/// `@import ...;`) from `css`, leaving only unconditional top-level rules.
pub(crate) fn strip_at_rules(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            // Skip the at-rule preamble until either `;` (rule
            // terminator, e.g. `@import`) or `{` (block start).
            let mut j = i;
            while j < bytes.len() && bytes[j] != b'{' && bytes[j] != b';' {
                j += 1;
            }
            if j >= bytes.len() {
                break;
            }
            if bytes[j] == b';' {
                // Bare at-rule with no block — skip including the `;`.
                i = j + 1;
                continue;
            }
            // Brace-balanced skip of the at-rule body.
            let mut depth = 0_i32;
            let mut k = j;
            while k < bytes.len() {
                match bytes[k] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            k += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            // `@layer name { … }` is a cascade-ordering wrapper, not a
            // condition: its rules always apply. Dropping it with the
            // conditional at-rules hid every rule a site moved into a
            // layer (a whole legacy stylesheet under `@layer legacy`)
            // from the target-size and focus-appearance checks. Unwrap
            // it instead, recursively, so nested `@media print` inside a
            // layer is still discarded.
            if is_layer_block(&css[i..j]) {
                let inner_end = k.saturating_sub(1).max(j + 1);
                out.push(' ');
                out.push_str(&strip_at_rules(&css[j + 1..inner_end]));
                out.push(' ');
            }
            i = k;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Whether an at-rule prelude (`@…` up to, not including, `{`) is a
/// cascade-layer block. Input is already lowercased by `preprocess_css`.
fn is_layer_block(prelude: &str) -> bool {
    let rest = prelude.trim_start_matches('@');
    rest.starts_with("layer")
        && rest[5..].chars().next().is_none_or(|c| c.is_whitespace())
}

/// Splits CSS into `(selector, body)` pairs at the top level. Comments
/// and at-rules must already be removed.
pub(crate) fn parse_top_level_rules(css: &str) -> Vec<(String, String)> {
    let mut rules = Vec::new();
    let bytes = css.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(open_rel) = css[i..].find('{') else {
            break;
        };
        let open = i + open_rel;
        let selector = css[i..open].trim().to_string();
        if selector.is_empty() {
            i = open + 1;
            continue;
        }
        // Brace-balanced body extraction (handles nested `{}` even
        // though plain CSS doesn't use them — defensive).
        let mut depth = 1_i32;
        let mut j = open + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let body = css[open + 1..j].to_string();
        rules.push((selector, body));
        i = j + 1;
    }
    rules
}

/// Returns `true` if `selector` (already lowercased + trimmed)
/// targets an interactive element class for WCAG 2.5.8.
pub(crate) fn selector_targets_interactive(selector: &str) -> bool {
    selector.contains("button")
        || selector.contains("input")
        || selector.contains("[role=\"button\"]")
        || selector.contains("[role='button']")
        || selector.contains("[role=button]")
        // Bare `a` or `a ...` selector. Excludes `area`, `aside`, etc.
        || selector == "a"
        || selector.starts_with("a ")
        || selector.starts_with("a:")
        || selector.starts_with("a.")
        || selector.starts_with("a#")
        || selector.starts_with("a[")
}

/// Returns the first `<prop>: NNpx` value (in pixels) inside `css`.
pub(crate) fn first_px_value(css: &str, prop: &str) -> Option<u32> {
    let pat = format!("{prop}:");
    let start = css.find(&pat)?;
    let after = &css[start + pat.len()..];
    let value = after.split(';').next()?.trim();
    let digits: String =
        value.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    if value[digits.len()..].trim_start().starts_with("px") {
        digits.parse().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_px_value_returns_none_for_non_numeric_value() {
        assert_eq!(first_px_value("width:auto;", "width"), None);
    }

    #[test]
    fn first_px_value_returns_none_for_non_px_unit() {
        assert_eq!(first_px_value("width:10em;", "width"), None);
    }

    #[test]
    fn extract_all_style_blocks_ignores_unterminated_block() {
        let html = "<html><head><style>button{width:8px}";
        assert_eq!(extract_all_style_blocks(html), Vec::<String>::new());
    }

    #[test]
    fn strip_at_rules_removes_bare_at_rule_with_semicolon() {
        let out = strip_at_rules("@import url(x.css);a{color:red}");
        assert!(!out.contains("@import"), "got: {out}");
        assert!(out.contains("a{color:red}"));
    }

    #[test]
    fn strip_at_rules_stops_at_unterminated_preamble() {
        // `@media (min-width: 600px` runs to EOF with neither `{` nor
        // `;` — the scanner must bail without panicking.
        let out = strip_at_rules("a{x:y}@media (min-width: 600px");
        assert!(out.contains("a{x:y}"));
        assert!(!out.contains("@media"));
    }

    #[test]
    fn parse_top_level_rules_handles_nested_braces_in_body() {
        // Defensive brace balancing: a nested `{}` inside a rule body
        // must not truncate the body early.
        let rules = parse_top_level_rules("s{a{b}c}");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].0, "s");
        assert_eq!(rules[0].1, "a{b}c");
    }

    #[test]
    fn parse_top_level_rules_skips_empty_selector() {
        let rules = parse_top_level_rules("{ width: 10px; }");
        assert_eq!(rules, Vec::<(String, String)>::new());
    }

    #[test]
    fn strip_at_rules_handles_nested_media() {
        let css = "a { color: red } @media print { a { color: blue } } b { color: green }";
        let stripped = strip_at_rules(css);
        assert!(stripped.contains("a { color: red }"));
        assert!(stripped.contains("b { color: green }"));
        assert!(!stripped.contains("@media"));
        // The print rule's `color: blue` declaration must be gone.
        assert!(!stripped.contains("blue"));
    }

    #[test]
    fn strip_at_rules_unwraps_layer_blocks() {
        let css = "@layer legacy { button { width: 10px } @media print { a { color: blue } } } b { c: d }";
        let stripped = strip_at_rules(css);
        assert!(stripped.contains("button { width: 10px }"), "{stripped}");
        assert!(stripped.contains("b { c: d }"));
        assert!(!stripped.contains("@layer") && !stripped.contains("@media"));
        assert!(
            !stripped.contains("blue"),
            "print rule inside a layer must still go"
        );
        let rules = parse_top_level_rules(&stripped);
        assert!(rules
            .iter()
            .any(|(s, b)| s == "button" && b.contains("10px")));
    }

    #[test]
    fn strip_at_rules_drops_layer_statements_and_layered_names_only() {
        // `@layer a, b;` is a bare statement; `@layers-foo` is not a layer.
        let css = "@layer a, b; @layerx { x { y: z } } q { r: s }";
        let stripped = strip_at_rules(css);
        assert!(!stripped.contains("x { y: z }"));
        assert!(stripped.contains("q { r: s }"));
    }

    #[test]
    fn strip_css_comments_removes_block_comments() {
        let css = "a { /* hidden */ color: red; }";
        let stripped = strip_css_comments(css);
        assert!(!stripped.contains("hidden"));
        assert!(stripped.contains("color: red"));
    }

    #[test]
    fn strip_css_comments_handles_unterminated_comment() {
        // Defensive: an unterminated /* should not loop forever.
        let css = "a { /* never closes";
        let _ = strip_css_comments(css);
    }

    #[test]
    fn extract_all_style_blocks_returns_every_block() {
        let html =
            "<html><head><style>x{}</style><style>y{}</style></head></html>";
        let blocks = extract_all_style_blocks(html);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].trim(), "x{}");
        assert_eq!(blocks[1].trim(), "y{}");
    }

    #[test]
    fn extract_all_style_blocks_handles_attributes_with_quoted_gt() {
        let html =
            r#"<html><head><style data-tag="x>y">a{}</style></head></html>"#;
        let blocks = extract_all_style_blocks(html);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].trim(), "a{}");
    }
}
