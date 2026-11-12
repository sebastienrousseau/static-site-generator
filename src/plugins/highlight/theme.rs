// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The `highlight.css` stylesheet: the `<pre>` box and the token
//! colours for the class-based spans mdx-gen emits (#822).

/// Token colours for the default theme, as `(selector, light, dark)`.
///
/// syntect's built-in themes were drawn for their own background and
/// fail WCAG 1.4.6 (7:1) on the `<pre>` box below: `InspiredGitHub`'s
/// green is 2.85:1 on `#f6f8fa`. Layering a dark theme over a light one
/// also left light rules the dark theme did not repeat (`.code`,
/// `#323232` on `#161b22`, 1.34:1). So the default palette is ours,
/// every colour is at least 7.3:1 on its box (the tests check), and the
/// dark block repeats every light selector. Later rules are more
/// specific and win over earlier ones on the same span.
const DEFAULT_TOKENS: &[(&str, &str, &str)] = &[
    (".comment", "#4b535d", "#a2acb7"),
    (".string", "#032563", "#addcff"),
    (".constant, .support.type", "#023b95", "#91cbff"),
    (".keyword, .storage, .invalid", "#a0111f", "#ff9492"),
    (".entity.name, .support.function", "#622cbc", "#dbb7ff"),
    (".entity.name.tag", "#024c1a", "#72f088"),
    (
        ".variable, .entity.other.attribute-name",
        "#702c00",
        "#ffb757",
    ),
];

/// Token colour rules for the default palette in one mode.
fn default_token_rules(dark: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for &(selectors, light, dark_colour) in DEFAULT_TOKENS {
        let scoped: Vec<String> = selectors
            .split(',')
            .map(|s| format!("pre.highlight {}", s.trim()))
            .collect();
        let colour = if dark { dark_colour } else { light };
        // Writing to a String cannot fail.
        let _ =
            write!(out, "{} {{\n color: {colour};\n}}\n", scoped.join(", "));
    }
    out
}

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

/// Token colour rules for `theme` when it is a syntect built-in the
/// site asked for by name, or the default palette otherwise.
fn token_css(theme: &str) -> String {
    if let Some(css) = scoped_theme_css(theme) {
        return css;
    }
    format!(
        "{}@media (prefers-color-scheme: dark) {{\n{}}}\n",
        default_token_rules(false),
        default_token_rules(true)
    )
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
  color: #1f2328;
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

    /// WCAG relative luminance of a `#rrggbb` colour.
    fn luminance(hex: &str) -> f64 {
        let channel = |i: usize| {
            let v = f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.0722f64.mul_add(
            channel(5),
            0.7152f64.mul_add(channel(3), 0.2126 * channel(1)),
        )
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// `(selector, body)` for every rule in `css` that is not a `<pre>`
    /// box rule or an at-rule, one entry per comma-separated selector.
    fn token_rules(css: &str) -> Vec<(String, String)> {
        css.split_inclusive('}')
            .filter_map(|rule| rule.split_once('{'))
            .flat_map(|(sels, body)| {
                sels.split(',')
                    .map(|s| (s.trim().to_owned(), body.to_owned()))
                    .collect::<Vec<_>>()
            })
            .filter(|(s, _)| {
                s.starts_with("pre.highlight ") && s != "pre.highlight code"
            })
            .collect()
    }

    /// Every `color: #rrggbb` declared by a token rule in `css`.
    fn token_colours(css: &str) -> Vec<String> {
        token_rules(css)
            .iter()
            .flat_map(|(_, body)| body.split(';'))
            .filter_map(|decl| {
                decl.trim().strip_prefix("color:").map(str::trim)
            })
            .filter(|v| v.starts_with('#') && v.len() == 7)
            .map(str::to_lowercase)
            .collect()
    }

    #[test]
    fn default_token_colours_meet_aaa_on_their_box_in_both_modes() {
        // The default theme took InspiredGitHub's colours (#63a35c on
        // the #f6f8fa box is 2.85:1) and left light rules the dark
        // theme did not override on the dark box (#323232 on #161b22
        // is 1.34:1). WCAG 1.4.6 asks 7:1 for code-sized text.
        let css = generate_highlight_css("github");
        let (light, dark) = css
            .rsplit_once("@media (prefers-color-scheme: dark)")
            .expect("dark token block");
        let dark = dark.trim_start().strip_prefix('{').expect("media block");
        let light = light.split_once("}\n}\n").map_or(light, |(_, rest)| rest);
        let (light, dark) = (token_colours(light), token_colours(dark));
        assert!(light.len() >= 6 && dark.len() >= 6, "{light:?} {dark:?}");
        for c in &light {
            assert!(
                contrast(c, "#f6f8fa") >= 7.0,
                "light {c}: {:.2}",
                contrast(c, "#f6f8fa")
            );
        }
        for c in &dark {
            assert!(
                contrast(c, "#161b22") >= 7.0,
                "dark {c}: {:.2}",
                contrast(c, "#161b22")
            );
        }
    }

    #[test]
    fn default_dark_rules_override_every_light_selector() {
        // A light rule with no dark counterpart keeps its light colour
        // on the dark box: that is how #323232 reached #161b22.
        let css = generate_highlight_css("github");
        let (light, dark) = css
            .rsplit_once("@media (prefers-color-scheme: dark)")
            .expect("dark token block");
        let dark = dark.trim_start().strip_prefix('{').expect("media block");
        let selectors = |block: &str| -> Vec<String> {
            token_rules(block)
                .into_iter()
                .filter(|(_, body)| body.contains("color:"))
                .map(|(s, _)| s)
                .collect()
        };
        let dark = selectors(dark);
        for s in selectors(
            light.split_once("}\n}\n").map_or(light, |(_, rest)| rest),
        ) {
            assert!(dark.contains(&s), "no dark rule for {s}");
        }
    }

    #[test]
    fn test_generate_css() {
        let css = generate_highlight_css("github");
        assert!(css.contains("pre.highlight"));
        assert!(css.contains("prefers-color-scheme: dark"));
    }
}
