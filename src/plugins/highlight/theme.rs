// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The `highlight.css` stylesheet: the `<pre>` box and the token
//! colours for the class-based spans mdx-gen emits (#822).

/// syntect theme for token colours on the light `<pre>` box when the
/// configured theme is not one of syntect's built-ins (the default,
/// `"github"`, is not).
const LIGHT_TOKEN_THEME: &str = "InspiredGitHub";

/// syntect theme for token colours on the dark-mode `<pre>` box.
const DARK_TOKEN_THEME: &str = mdx_gen::highlight::DEFAULT_THEME;

/// Generates a CSS theme for syntax highlighting.
///
/// The `<pre>` box rules are followed by token colours for the
/// `<span class="...">` scopes mdx-gen emits (#822), scoped under
/// `pre.highlight` so generic names such as `.string` cannot restyle
/// the rest of the page.
pub(super) fn generate_highlight_css(theme: &str) -> String {
    let mut css = String::from(BOX_CSS);
    css.push_str(&token_css(theme));
    css
}

/// Token colour rules for `theme`, or for the light and dark defaults
/// when `theme` is not a syntect built-in.
fn token_css(theme: &str) -> String {
    if let Some(css) = scoped_theme_css(theme) {
        return css;
    }
    let light = scoped_theme_css(LIGHT_TOKEN_THEME).unwrap_or_default();
    let dark = scoped_theme_css(DARK_TOKEN_THEME).unwrap_or_default();
    format!("{light}@media (prefers-color-scheme: dark) {{\n{dark}}}\n")
}

/// syntect's stylesheet for a built-in theme, scoped to `pre.highlight`.
fn scoped_theme_css(theme: &str) -> Option<String> {
    mdx_gen::highlight::theme_css(theme)
        .map(|css| scope_rules(&css, "pre.highlight"))
}

/// Prefixes every selector of every plain rule in `css` with `scope`.
///
/// Comments are dropped and at-rule blocks are skipped: syntect's
/// output has neither nested blocks nor braces inside values.
fn scope_rules(css: &str, scope: &str) -> String {
    let mut out = String::new();
    for rule in strip_comments(css).split_inclusive('}') {
        let Some((selectors, body)) = rule.split_once('{') else {
            continue;
        };
        let selectors = selectors.trim();
        if selectors.is_empty() || selectors.starts_with('@') {
            continue;
        }
        let scoped: Vec<String> = selectors
            .split(',')
            .map(|s| format!("{scope} {}", s.trim()))
            .collect();
        out.push_str(&scoped.join(", "));
        out.push_str(" {");
        out.push_str(body);
        out.push('\n');
    }
    out
}

/// Removes `/* ... */` comments; an unterminated one runs to the end.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..]
            .split_once("*/")
            .map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    out
}

/// Styles for the `<pre>` box itself, in light and dark mode.
const BOX_CSS: &str = r#"/* Syntax highlighting — GitHub-inspired theme */
pre.highlight {
  background: #f6f8fa;
  border: 1px solid #d0d7de;
  border-radius: 6px;
  padding: 1em;
  overflow-x: auto;
  font-size: 0.875em;
  line-height: 1.45;
}
pre.highlight code {
  background: none;
  padding: 0;
  border: none;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
}
@media (prefers-color-scheme: dark) {
  pre.highlight {
    background: #161b22;
    border-color: #30363d;
    color: #e6edf3;
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_css_colours_the_token_classes_mdx_gen_emits() {
        // #822: the stylesheet styled only the `<pre>` box, so every
        // `<span class="comment ...">` rendered in one colour.
        let css = generate_highlight_css("github");
        assert!(css.contains("pre.highlight .comment"), "{css}");
        assert!(css.contains("pre.highlight .string"), "{css}");
        let dark = css
            .split("@media (prefers-color-scheme: dark)")
            .nth(2)
            .expect("token rules for dark mode");
        assert!(dark.contains("pre.highlight .comment"), "{dark}");
    }

    #[test]
    fn generated_css_uses_a_named_syntect_theme_alone() {
        let css = generate_highlight_css("Solarized (dark)");
        assert!(css.contains("pre.highlight .comment"), "{css}");
        assert_eq!(css.matches("prefers-color-scheme: dark").count(), 1);
    }

    #[test]
    fn scope_rules_prefixes_every_selector_and_drops_comments() {
        let css = "/* a, b */\n.a, .b c {\n color: red;\n}\n@x {}\n";
        assert_eq!(
            scope_rules(css, "pre.h"),
            "pre.h .a, pre.h .b c {\n color: red;\n}\n"
        );
    }

    #[test]
    fn test_generate_css() {
        let css = generate_highlight_css("github");
        assert!(css.contains("pre.highlight"));
        assert!(css.contains("prefers-color-scheme: dark"));
    }
}
