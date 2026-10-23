// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Syntax highlighting plugin.
//!
//! Post-processes compiled HTML to add syntax highlighting to code
//! blocks. Uses class-based highlighting with a generated CSS file,
//! avoiding inline styles for better performance and cacheability.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use std::fs;

/// Plugin that adds syntax highlighting CSS classes to code blocks.
///
/// Runs in `after_compile`. Finds `<pre><code class="language-X">`
/// blocks, injects classes and links `highlight.css` from each page
/// that has one. Also generates `highlight.css`.
#[derive(Debug, Clone)]
pub struct HighlightPlugin {
    theme: String,
}

impl Default for HighlightPlugin {
    fn default() -> Self {
        Self {
            theme: "github".to_string(),
        }
    }
}

impl HighlightPlugin {
    /// Creates a new `HighlightPlugin` with the given theme.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::highlight::HighlightPlugin;
    /// use ssg::plugin::Plugin;
    ///
    /// let p = HighlightPlugin::new("monokai");
    /// assert_eq!(p.name(), "highlight");
    /// ```
    #[must_use]
    pub fn new(theme: impl Into<String>) -> Self {
        Self {
            theme: theme.into(),
        }
    }

    /// Creates a highlight plugin with the given theme name.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::highlight::HighlightPlugin;
    /// use ssg::plugin::Plugin;
    ///
    /// let p = HighlightPlugin::with_theme("solarized");
    /// assert_eq!(p.name(), "highlight");
    /// ```
    #[must_use]
    pub fn with_theme(theme: impl Into<String>) -> Self {
        Self {
            theme: theme.into(),
        }
    }
}

impl Plugin for HighlightPlugin {
    fn name(&self) -> &'static str {
        "highlight"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        // Pages are rewritten here rather than in `transform_html`: every
        // `after_compile` hook runs before any transform, so by then
        // `FingerprintPlugin` had renamed `highlight.css` and the injected
        // link named a file that was never written (#790). Linking it
        // first lets that pass rename the reference and add SRI with the
        // rest of the stylesheets.
        for path in ctx.get_html_files() {
            let html = fs::read_to_string(&path).with_path(&path)?;
            if let Some(out) = highlight_page(&html) {
                fs::write(&path, out).with_path(&path)?;
            }
        }

        // Generate highlight.css
        let css = generate_highlight_css(&self.theme);
        let path = ctx.site_dir.join("highlight.css");
        fs::write(&path, &css).with_path(&path)?;

        Ok(())
    }
}

/// Adds highlight markup to a page and links `highlight.css` from it.
///
/// Returns `None` when the page has no code block to highlight.
fn highlight_page(html: &str) -> Option<String> {
    let result = add_highlight_markup(html);
    if result == html {
        None
    } else if result.contains("highlight.css") {
        Some(result)
    } else {
        Some(inject_css_link(&result))
    }
}

/// Adds highlight markup to code blocks.
///
/// Transforms `<pre><code class="language-X">` into
/// `<pre class="highlight"><code class="language-X" data-lang="X">`.
fn add_highlight_markup(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut pos = 0;

    while pos < html.len() {
        if let Some(pre_start) = html[pos..].find("<pre>") {
            let abs_pre = pos + pre_start;
            let after_pre = abs_pre + 5; // len("<pre>")

            // Check if next element is <code class="language-
            let remaining = &html[after_pre..];
            if remaining.starts_with("<code class=\"language-") {
                // Extract language name
                let lang_start = "language-".len();
                let code_attr = &remaining["<code class=\"".len()..];
                let lang_end = code_attr.find('"').unwrap_or(0);
                let lang = &code_attr[lang_start..lang_end];

                // Write the enhanced pre tag
                result.push_str(&html[pos..abs_pre]);
                result.push_str(&format!(
                    "<pre class=\"highlight language-{lang}\">"
                ));
                result.push_str(&format!(
                    "<code class=\"language-{lang}\" data-lang=\"{lang}\">"
                ));

                // Skip past the original <pre><code class="language-X">
                let code_tag_end = remaining.find('>').unwrap_or(0);
                pos = after_pre + code_tag_end + 1;
                continue;
            }
        }

        // No match — copy rest and break
        result.push_str(&html[pos..]);
        break;
    }

    result
}

/// Injects a `<link>` to highlight.css before `</head>`.
///
/// A page with no `<head>` gets the link appended instead, as the CSP
/// pass does for its stylesheets, so highlighted code is never left
/// without its stylesheet.
fn inject_css_link(html: &str) -> String {
    let link = "<link rel=\"stylesheet\" href=\"/highlight.css\">\n";
    let injected = inject_before_head_close(html, link);
    if injected == html {
        format!("{html}{link}")
    } else {
        injected
    }
}

/// Generates a CSS theme for syntax highlighting.
fn generate_highlight_css(_theme: &str) -> String {
    r#"/* Syntax highlighting — GitHub-inspired theme */
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
"#
    .to_string()
}

#[cfg(test)]
fn collect_html_files(
    dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, SsgError> {
    crate::walk::walk_files(dir, "html")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_add_highlight_markup() {
        let html =
            r#"<pre><code class="language-rust">fn main() {}</code></pre>"#;
        let result = add_highlight_markup(html);
        assert!(result.contains("class=\"highlight language-rust\""));
        assert!(result.contains("data-lang=\"rust\""));
    }

    #[test]
    fn test_no_code_block_unchanged() {
        let html = "<pre>plain text</pre>";
        let result = add_highlight_markup(html);
        assert_eq!(result, html);
    }

    #[test]
    fn test_inject_css_link() {
        let html = "<html><head><title>X</title></head><body></body></html>";
        let result = inject_css_link(html);
        assert!(result.contains("highlight.css"));
    }

    #[test]
    fn test_generate_css() {
        let css = generate_highlight_css("github");
        assert!(css.contains("pre.highlight"));
        assert!(css.contains("prefers-color-scheme: dark"));
    }

    // -------------------------------------------------------------------
    // Plugin trait + constructor surface
    // -------------------------------------------------------------------

    #[test]
    fn name_returns_static_highlight_identifier() {
        assert_eq!(HighlightPlugin::default().name(), "highlight");
    }

    #[test]
    fn default_constructor_uses_github_theme() {
        let plugin = HighlightPlugin::default();
        assert_eq!(plugin.theme, "github");
    }

    #[test]
    fn new_constructor_stores_supplied_theme_name() {
        // Covers the `new` constructor (previously exercised only by
        // doctests, which don't count towards lib coverage).
        let plugin = HighlightPlugin::new("monokai");
        assert_eq!(plugin.theme, "monokai");
        let plugin2 = HighlightPlugin::new(String::from("nord"));
        assert_eq!(plugin2.theme, "nord");
    }

    #[test]
    fn highlight_page_with_existing_css_link_skips_reinjection() {
        // Covers the `result.contains("highlight.css")` true branch of
        // highlight_page: the markup is rewritten but no second <link>
        // is injected.
        let html = r#"<html><head><link rel="stylesheet" href="/highlight.css"></head><body><pre><code class="language-rs">x</code></pre></body></html>"#;
        let out = highlight_page(html).unwrap();
        assert!(out.contains("highlight language-rs"));
        assert_eq!(out.matches("/highlight.css").count(), 1);
    }

    #[test]
    fn highlight_page_without_code_blocks_returns_none() {
        let html = "<html><head></head><body><p>no code</p></body></html>";
        assert!(highlight_page(html).is_none());
    }

    #[test]
    fn with_theme_stores_supplied_theme_name() {
        // Covers the `with_theme` constructor at lines 38-42.
        let plugin = HighlightPlugin::with_theme("solarized");
        assert_eq!(plugin.theme, "solarized");
        let plugin2 = HighlightPlugin::with_theme(String::from("dracula"));
        assert_eq!(plugin2.theme, "dracula");
    }

    #[test]
    fn after_compile_missing_site_dir_returns_ok() {
        // Line 52: `!ctx.site_dir.exists()` early return.
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let ctx =
            PluginContext::new(dir.path(), dir.path(), &missing, dir.path());
        HighlightPlugin::default().after_compile(&ctx).unwrap();
        assert!(!missing.join("highlight.css").exists());
    }

    #[test]
    fn after_compile_html_without_code_blocks_is_unchanged() {
        // Covers the `result != html` false branch at line 66 —
        // file is not rewritten when add_highlight_markup returns
        // its input unchanged.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let html = "<html><head></head><body><p>no code</p></body></html>";
        fs::write(site.join("plain.html"), html).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        HighlightPlugin::default().after_compile(&ctx).unwrap();
        assert_eq!(fs::read_to_string(site.join("plain.html")).unwrap(), html);
    }

    #[test]
    fn after_compile_preserves_existing_highlight_css_link() {
        // Covers the `result.contains("highlight.css")` true branch
        // at line 68 — when the link is already present the file is
        // rewritten without re-injection.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let html = r#"<html><head><link rel="stylesheet" href="/highlight.css"></head><body><pre><code class="language-rs">x</code></pre></body></html>"#;
        fs::write(site.join("index.html"), html).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        HighlightPlugin::default().after_compile(&ctx).unwrap();
        let out = fs::read_to_string(site.join("index.html")).unwrap();
        // Exactly one stylesheet link — no double-injection.
        assert_eq!(out.matches("/highlight.css").count(), 1);
    }

    #[test]
    fn inject_css_link_without_head_appends_the_link() {
        // A page with no `<head>` still gets its stylesheet (#790).
        let html = "<body>no head</body>";
        let result = inject_css_link(html);
        assert_eq!(
            result,
            "<body>no head</body><link rel=\"stylesheet\" href=\"/highlight.css\">\n"
        );
    }

    #[test]
    fn collect_html_files_recurses_and_sorts() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.path().join("z.html"), "").unwrap();
        fs::write(dir.path().join("a.html"), "").unwrap();
        fs::write(sub.join("m.html"), "").unwrap();

        let files = collect_html_files(dir.path()).unwrap();
        assert_eq!(files.len(), 3);
        let first = files[0].file_name().unwrap().to_str().unwrap();
        assert_eq!(first, "a.html");
    }

    #[test]
    fn collect_html_files_returns_empty_for_missing_directory() {
        let dir = tempdir().unwrap();
        let result = collect_html_files(&dir.path().join("missing")).unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    fn test_plugin_generates_css() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        let html = r#"<html><head><title>X</title></head><body><pre><code class="language-js">let x = 1;</code></pre></body></html>"#;

        fs::write(site.join("index.html"), html).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        HighlightPlugin::default().after_compile(&ctx).unwrap();

        assert!(site.join("highlight.css").exists());
        let output = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(output.contains("highlight.css"));
        assert!(output.contains("highlight language-js"));
    }

    #[test]
    fn after_compile_write_failure_returns_io_error() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        // Create a directory where highlight.css is expected to be written, causing fs::write to fail.
        let css_dir = site.join("highlight.css");
        fs::create_dir(&css_dir).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let res = HighlightPlugin::default().after_compile(&ctx);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == &css_dir)
        );
    }
}
