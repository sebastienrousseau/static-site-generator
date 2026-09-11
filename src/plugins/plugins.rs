// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! # Built-in plugins
//!
//! Ready-to-use plugins for common static site generation tasks.
//!
//! - `MinifyPlugin` — Minifies HTML files in the site output directory.
//!   With the `minify` feature enabled, also minifies `.css` and `.js`
//!   assets and walks the site directory recursively.
//! - `ImageOptiPlugin` — Logs image files for optimization (stub for external tooling).
//! - `DeployPlugin` — Logs deployment target after build (stub for CI integration).

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use rayon::prelude::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Minifies HTML files and (with the `minify` feature) JS/CSS assets.
///
/// Runs during the `after_compile` hook.
///
/// * **Default build:** processes only top-level `.html` files in
///   `site_dir`, falling back to a whitespace-collapsing pass that
///   short-circuits on any document containing `<pre`.
/// * **`minify` feature:** walks `site_dir` recursively (via `walkdir`)
///   and uses
///   [`minify-html`](https://crates.io/crates/minify-html) for HTML,
///   [`oxc_minifier`](https://crates.io/crates/oxc_minifier) for JS, and
///   [`lightningcss`](https://crates.io/crates/lightningcss) for CSS.
///   `<pre>` content is preserved bit-identically by `minify-html`'s
///   native handling.
///
/// # Example
///
/// ```rust
/// use ssg::plugin::PluginManager;
/// use ssg::plugins::MinifyPlugin;
///
/// let mut pm = PluginManager::new();
/// pm.register(MinifyPlugin);
/// ```
#[derive(Debug, Copy, Clone)]
pub struct MinifyPlugin;

impl Plugin for MinifyPlugin {
    fn name(&self) -> &'static str {
        "minify"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        let cache = ctx.cache.as_ref();
        let (html_files, css_files, js_files) =
            collect_minifiable_files(&ctx.site_dir, cache)?;

        let count = AtomicUsize::new(0);

        html_files
            .par_iter()
            .try_for_each(|path| -> Result<(), SsgError> {
                fail_point!("plugins::minify-read", |_| {
                    Err(SsgError::Io {
                        path: path.clone(),
                        source: std::io::Error::other(
                            "injected: plugins::minify-read",
                        ),
                    })
                });
                let content = fs::read_to_string(path).with_path(path)?;
                let minified = minify_html(&content);
                fail_point!("plugins::minify-write", |_| {
                    Err(SsgError::Io {
                        path: path.clone(),
                        source: std::io::Error::other(
                            "injected: plugins::minify-write",
                        ),
                    })
                });
                fs::write(path, &minified).with_path(path)?;
                let _ = count.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })?;

        // CSS and JS go through ssg's own minifiers, the same ones the
        // asset pipeline uses. They used to run only under a `minify`
        // feature that pulled in minify-html, lightningcss and five oxc
        // crates; the default build populated these lists and then threw
        // them away.
        css_files
            .par_iter()
            .try_for_each(|path| -> Result<(), SsgError> {
                let content = fs::read_to_string(path).with_path(path)?;
                let minified = minify_css(&content);
                fs::write(path, &minified).with_path(path)?;
                let _ = count.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })?;

        js_files
            .par_iter()
            .try_for_each(|path| -> Result<(), SsgError> {
                let content = fs::read_to_string(path).with_path(path)?;
                let minified = minify_js(&content);
                fs::write(path, &minified).with_path(path)?;
                let _ = count.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })?;

        let total = count.load(Ordering::Relaxed);
        if total > 0 {
            println!("[minify] Processed {total} file(s)");
        }
        Ok(())
    }
}

/// `(html, css, js)` file lists returned by [`collect_minifiable_files`].
type MinifiableFiles = (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>);

/// Walks `site_dir` and returns `(html, css, js)` file lists, honouring
/// the plugin cache for incremental builds.
///
/// The walk is iterative rather than recursive so a deep tree cannot
/// overflow the stack, and symlinks are not followed - the same contract
/// `walkdir`'s `follow_links(false)` gave, without the dependency. Errors on
/// individual entries are skipped; only failing to read `site_dir` itself is
/// reported, which is what the previous implementation did.
fn collect_minifiable_files(
    site_dir: &std::path::Path,
    cache: Option<&crate::plugin::PluginCache>,
) -> Result<MinifiableFiles, SsgError> {
    let mut html = Vec::new();
    let mut css = Vec::new();
    let mut js = Vec::new();

    // Probe the root first so an unreadable site directory is an error
    // rather than an empty result; deeper directories are skipped quietly,
    // which is what filtering `walkdir`'s errors did.
    drop(fs::read_dir(site_dir).with_path(site_dir)?);

    let mut stack = vec![site_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            // Cache check applies uniformly to all minifiable assets.
            if cache.is_some_and(|c| !c.has_changed(&path)) {
                continue;
            }
            match ext {
                "html" => html.push(path),
                "css" => css.push(path),
                "js" => js.push(path),
                _ => {}
            }
        }
    }
    Ok((html, css, js))
}

/// HTML minification.
///
/// * With the `minify` feature: delegates to `minify-html` configured
///   with `keep_comments: false`, `do_not_minify_doctype: true`. CSS
///   inside `<style>` and JS inside `<script>` are passed through
///   without inline minification (the dedicated asset-file passes
///   handle that, and avoid double-minification of inline blocks that
///   may contain template-specific syntax).
/// * Without the feature: falls back to a whitespace-collapsing pass
///   that short-circuits when any `<pre` substring is present so
///   user-visible whitespace in code blocks is preserved.
///
/// # Examples
///
/// ```rust
/// use ssg::plugins::minify_html;
///
/// let out = minify_html("<html>   <body>hi</body>  </html>");
/// assert!(out.len() <= "<html>   <body>hi</body>  </html>".len());
/// ```
/// Elements whose text content must survive byte for byte.
///
/// `pre` and `textarea` render whitespace literally; `script` and `style`
/// hold source in another language, where a run of spaces can sit inside a
/// string literal. Collapsing any of them changes what the page does.
const RAW_TEXT_ELEMENTS: [&str; 4] = ["pre", "textarea", "script", "style"];

/// Scans one tag starting at `start` (which must index a `<`).
///
/// Returns the byte index just past the closing `>` and the lowercased
/// element name for an opening tag. Attribute values are scanned with quote
/// tracking, so a `>` inside one does not end the tag early.
fn scan_tag(html: &str, start: usize) -> (usize, Option<String>) {
    let bytes = html.as_bytes();
    let mut i = start + 1;
    let closing = bytes.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    let name_start = i;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-')
    {
        i += 1;
    }
    let name = if i > name_start && !closing {
        Some(html[name_start..i].to_ascii_lowercase())
    } else {
        None
    };
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'"' || c == b'\'' {
                    quote = Some(c);
                } else if c == b'>' {
                    return (i + 1, name);
                }
            }
        }
        i += 1;
    }
    (bytes.len(), name)
}

/// Byte offset of `</name` in `hay`, case-insensitively, without allocating.
fn find_closing_tag(hay: &[u8], name: &str) -> Option<usize> {
    let n = name.as_bytes();
    let mut i = 0;
    while i + 2 + n.len() <= hay.len() {
        if hay[i] == b'<'
            && hay[i + 1] == b'/'
            && hay[i + 2..i + 2 + n.len()].eq_ignore_ascii_case(n)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Collapses insignificant whitespace in HTML.
///
/// This is ssg's own implementation rather than a dependency. Minification
/// rewrites every page the generator emits, so a bug in it is a bug in every
/// site; keeping it in-tree means it is covered by this crate's own tests and
/// cannot change underneath a release.
///
/// What it does not touch:
///
/// * the content of [`RAW_TEXT_ELEMENTS`], byte for byte
/// * anything between `<` and `>`, so attribute values keep their spacing
/// * comments, including conditional ones
///
/// A previous version bailed out of the whole document if `<pre` appeared
/// anywhere, and collapsed whitespace everywhere else - including inside
/// `<script>`, where it silently rewrote string literals. This one tracks
/// which element it is inside, so a page can contain a `<pre>` block and
/// still be minified around it.
///
/// # Examples
///
/// ```rust
/// use ssg::plugins::minify_html;
///
/// assert_eq!(minify_html("<p>  Hello   World  </p>"), "<p> Hello World </p>");
///
/// // A script's contents are left exactly as written.
/// let js = r#"<script>var s = "a  b";</script>"#;
/// assert_eq!(minify_html(js), js);
/// ```
#[must_use]
pub fn minify_html(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    // Whitespace seen in a text run, not yet emitted. Holding it back means a
    // run collapses to one space and the space lands before the next thing,
    // whether that is text or a tag.
    let mut pending_space = false;

    while i < bytes.len() {
        if bytes[i] == b'<' {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            if html[i..].starts_with("<!--") {
                let end =
                    html[i..].find("-->").map_or(bytes.len(), |p| i + p + 3);
                out.push_str(&html[i..end]);
                i = end;
                continue;
            }
            let (tag_end, name) = scan_tag(html, i);
            let self_closing = html[i..tag_end].trim_end().ends_with("/>");
            out.push_str(&html[i..tag_end]);
            i = tag_end;

            if let Some(name) = name {
                if RAW_TEXT_ELEMENTS.contains(&name.as_str()) && !self_closing {
                    let rest = &bytes[i..];
                    let stop =
                        find_closing_tag(rest, &name).unwrap_or(rest.len());
                    out.push_str(&html[i..i + stop]);
                    i += stop;
                }
            }
            continue;
        }

        let ch = html[i..].chars().next().unwrap_or('\0');
        if ch.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
        i += ch.len_utf8();
    }
    if pending_space {
        out.push(' ');
    }
    out
}

/// ssg's own CSS and JavaScript minifiers, re-exported so the whole
/// minification surface lives behind one module.
///
/// These replace `lightningcss` and `oxc_minifier`, which sat behind an
/// optional `minify` feature. A minifier rewrites every byte the generator
/// emits; keeping it in-tree means it is covered by this crate's own tests
/// and cannot change underneath a release.
pub use crate::plugins_group::assets::{minify_css, minify_js};

/// Image optimization plugin stub.
///
/// Scans the site directory for image files and logs them.
/// Actual optimization requires external tools (e.g., `cwebp`, `avifenc`).
///
/// # Example
///
/// ```rust
/// use ssg::plugin::PluginManager;
/// use ssg::plugins::ImageOptiPlugin;
///
/// let mut pm = PluginManager::new();
/// pm.register(ImageOptiPlugin);
/// ```
#[derive(Debug, Copy, Clone)]
pub struct ImageOptiPlugin;

impl Plugin for ImageOptiPlugin {
    fn name(&self) -> &'static str {
        "image-opti"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }
        let mut images = Vec::new();
        for entry in fs::read_dir(&ctx.site_dir).with_path(&ctx.site_dir)? {
            let entry = entry.with_path(&ctx.site_dir)?;
            let path = entry.path();
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if matches!(
                    ext.as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "bmp"
                ) {
                    images.push(path);
                }
            }
        }
        if !images.is_empty() {
            println!(
                "[image-opti] Found {} images for optimization",
                images.len()
            );
        }
        Ok(())
    }
}

/// Deployment plugin stub.
///
/// Logs the deployment target after a successful build.
/// Extend with actual deployment logic for Vercel, Netlify, or Cloudflare.
///
/// # Example
///
/// ```rust
/// use ssg::plugin::PluginManager;
/// use ssg::plugins::DeployPlugin;
///
/// let mut pm = PluginManager::new();
/// pm.register(DeployPlugin::new("production"));
/// ```
/// Superseded by [`crate::deploy::DeployPlugin`], which is the implementation
/// the pipeline registers. This one was never wired into a build; it survived
/// as a second, divergent copy of the same idea.
#[deprecated(
    since = "0.0.58",
    note = "use `ssg::deploy::DeployPlugin`; this one is never registered by the pipeline"
)]
#[derive(Debug)]
pub struct DeployPlugin {
    target: String,
}

#[allow(deprecated)]
impl DeployPlugin {
    /// Creates a new deployment plugin for the given target environment.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugins::DeployPlugin;
    /// use ssg::plugin::Plugin;
    ///
    /// let p = DeployPlugin::new("production");
    /// assert_eq!(p.name(), "deploy");
    /// ```
    #[must_use]
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_string(),
        }
    }
}

#[allow(deprecated)]
impl Plugin for DeployPlugin {
    fn name(&self) -> &'static str {
        "deploy"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        println!(
            "[deploy] Site at {} ready for deployment to '{}'",
            ctx.site_dir.display(),
            self.target
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // These tests exercise the deprecated plugin deliberately: it is
    // still shipped for one release, and this is what keeps it working
    // until removal.
    #![allow(deprecated)]
    use super::*;
    use crate::plugin::{PluginCache, PluginContext};
    use crate::test_support::init_logger;
    use anyhow::Result;
    use std::path::Path;
    use tempfile::tempdir;

    fn test_ctx_with(site_dir: &Path) -> PluginContext {
        init_logger();
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            site_dir,
            Path::new("templates"),
        )
    }

    #[test]
    fn test_minify_plugin_name() {
        assert_eq!(MinifyPlugin.name(), "minify");
    }

    #[test]
    fn test_minify_plugin_empty_dir() -> Result<()> {
        let temp = tempdir().unwrap();
        let ctx = test_ctx_with(temp.path());
        MinifyPlugin.after_compile(&ctx).unwrap();
        Ok(())
    }

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn test_minify_plugin_processes_html() -> Result<()> {
        let temp = tempdir().unwrap();
        let html_path = temp.path().join("index.html");
        fs::write(&html_path, "<h1>  Hello   World  </h1>").unwrap();

        let ctx = test_ctx_with(temp.path());
        MinifyPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(&html_path).unwrap();
        assert!(!content.contains("  "));
        Ok(())
    }

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn test_minify_plugin_cache_skips_unchanged_html() -> Result<()> {
        // `collect_minifiable_files`'s
        // `cache.is_none_or(|c| c.has_changed(p))` closure is only
        // ever invoked when `ctx.cache` is `Some(..)` — every other
        // test in this file leaves it `None`, where `is_none_or`
        // short-circuits without calling the closure at all.
        let temp = tempdir().unwrap();
        let html_path = temp.path().join("index.html");
        fs::write(&html_path, "<h1>  Hello   World  </h1>").unwrap();

        let mut cache = PluginCache::new();
        cache.update(&html_path);

        let mut ctx = test_ctx_with(temp.path());
        ctx.cache = Some(cache);
        MinifyPlugin.after_compile(&ctx).unwrap();

        // Unchanged per the cache ⇒ filtered out ⇒ left untouched.
        let content = fs::read_to_string(&html_path).unwrap();
        assert_eq!(content, "<h1>  Hello   World  </h1>");
        Ok(())
    }

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn test_minify_plugin_cache_processes_changed_html() -> Result<()> {
        // The same closure's "changed" arm: a cache entry recorded
        // against different content means `has_changed` returns
        // `true`, so the file is still processed.
        let temp = tempdir().unwrap();
        let html_path = temp.path().join("index.html");
        fs::write(&html_path, "<h1>  Hello   World  </h1>").unwrap();

        let mut cache = PluginCache::new();
        fs::write(&html_path, "stale content").unwrap();
        cache.update(&html_path);
        fs::write(&html_path, "<h1>  Hello   World  </h1>").unwrap();

        let mut ctx = test_ctx_with(temp.path());
        ctx.cache = Some(cache);
        MinifyPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(&html_path).unwrap();
        assert!(!content.contains("  "), "changed file must be minified");
        Ok(())
    }

    #[test]
    fn minify_plugin_minifies_css_too() -> Result<()> {
        let temp = tempdir().unwrap();
        let css_path = temp.path().join("style.css");
        fs::write(&css_path, "body {   color: red;   }").unwrap();

        let ctx = test_ctx_with(temp.path());
        MinifyPlugin.after_compile(&ctx).unwrap();

        // This used to assert the opposite - that the file came back with
        // its three spaces intact - because CSS was only minified when the
        // `minify` feature pulled in lightningcss. The default build
        // collected the file and threw the list away.
        let content = fs::read_to_string(&css_path).unwrap();
        assert!(
            !content.contains("   "),
            "CSS was not minified: {content:?}"
        );
        assert!(content.contains("color"));
        assert!(content.contains("red"));
        Ok(())
    }

    #[test]
    fn test_minify_plugin_nonexistent_dir() -> Result<()> {
        let ctx = test_ctx_with(Path::new("/nonexistent"));
        MinifyPlugin.after_compile(&ctx).unwrap();
        Ok(())
    }

    #[test]
    fn minify_html_leaves_raw_text_elements_byte_for_byte() {
        // Every one of these was corrupted by the previous implementation,
        // which collapsed whitespace everywhere outside a `<pre>`-bearing
        // document. A run of spaces inside a string literal is data.
        for input in [
            r#"<script>var s = "a  b";</script>"#,
            "<textarea>line1\n  line2</textarea>",
            r#"<style>a{content:"x  y"}</style>"#,
            "<pre>  keep   spaces  </pre>",
        ] {
            assert_eq!(
                minify_html(input),
                input,
                "raw text was rewritten: {input:?}"
            );
        }
    }

    #[test]
    fn minify_html_minifies_around_a_pre_block() {
        // The old pass gave up on the whole document the moment `<pre`
        // appeared anywhere in it, so a single code block cost every other
        // byte on the page.
        let out = minify_html("<p>a   b</p><pre>x   y</pre><p>c   d</p>");
        assert_eq!(out, "<p>a b</p><pre>x   y</pre><p>c d</p>");
    }

    #[test]
    fn minify_html_keeps_attribute_values_intact() {
        let input = r#"<a title="two  spaces" href="/x">t   t</a>"#;
        assert_eq!(
            minify_html(input),
            r#"<a title="two  spaces" href="/x">t t</a>"#
        );
    }

    #[test]
    fn minify_html_does_not_end_a_tag_on_a_quoted_angle_bracket() {
        let input = r#"<a title="a > b">x   y</a>"#;
        assert_eq!(minify_html(input), r#"<a title="a > b">x y</a>"#);
    }

    #[test]
    fn minify_html_preserves_comments() {
        let input = "<!--[if IE]>  legacy  <![endif]--><p>a   b</p>";
        assert_eq!(
            minify_html(input),
            "<!--[if IE]>  legacy  <![endif]--><p>a b</p>"
        );
    }

    #[test]
    fn minify_html_is_idempotent() {
        let corpus = [
            "<p>  a   b  </p>",
            r#"<script>var s = "a  b";</script><p>  c  </p>"#,
            "<pre>  x  </pre><div>  y  </div>",
            "<!DOCTYPE html><html lang=\"en\"><body>  hi  </body></html>",
        ];
        for input in corpus {
            let once = minify_html(input);
            assert_eq!(minify_html(&once), once, "not idempotent: {input:?}");
        }
    }

    #[test]
    fn test_minify_html_collapses_whitespace() {
        let result = minify_html("<p>  Hello   World  </p>");
        assert_eq!(result, "<p> Hello World </p>");
    }

    #[test]
    fn test_minify_html_preserves_pre() {
        let input = "<pre>  keep   spaces  </pre>";
        let result = minify_html(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_image_opti_plugin_name() {
        assert_eq!(ImageOptiPlugin.name(), "image-opti");
    }

    #[test]
    fn test_image_opti_plugin_finds_images() -> Result<()> {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("photo.png"), "PNG").unwrap();
        fs::write(temp.path().join("logo.jpg"), "JPG").unwrap();
        fs::write(temp.path().join("style.css"), "CSS").unwrap();

        let ctx = test_ctx_with(temp.path());
        ImageOptiPlugin.after_compile(&ctx).unwrap();
        Ok(())
    }

    #[test]
    fn test_image_opti_plugin_nonexistent_dir() -> Result<()> {
        let ctx = test_ctx_with(Path::new("/nonexistent"));
        ImageOptiPlugin.after_compile(&ctx).unwrap();
        Ok(())
    }

    #[test]
    fn test_deploy_plugin_name() {
        let p = DeployPlugin::new("staging");
        assert_eq!(p.name(), "deploy");
    }

    #[test]
    fn test_deploy_plugin_prints_target() -> Result<()> {
        let temp = tempdir().unwrap();
        let ctx = test_ctx_with(temp.path());
        let p = DeployPlugin::new("production");
        p.after_compile(&ctx).unwrap();
        Ok(())
    }

    #[test]
    fn test_all_plugins_register() {
        use crate::plugin::PluginManager;
        let mut pm = PluginManager::new();
        pm.register(MinifyPlugin);
        pm.register(ImageOptiPlugin);
        pm.register(DeployPlugin::new("test"));
        assert_eq!(pm.len(), 3);
        assert_eq!(pm.names(), vec!["minify", "image-opti", "deploy"]);
    }

    #[test]
    fn minify_plugin_preserves_pre_blocks() {
        // Arrange
        let input = "<pre>  code   with   spaces  </pre><p>  other  </p>";

        // Act
        let result = minify_html(input);

        // Assert — the <pre> survives byte for byte, the rest is collapsed
        assert_eq!(result, "<pre>  code   with   spaces  </pre><p> other </p>");
    }

    #[test]
    fn minify_plugin_handles_nested_html() {
        // Arrange
        let input = "<div>  <section>  <article>  <p>  deep  </p>  </article>  </section>  </div>";

        // Act
        let result = minify_html(input);

        // Assert — runs of whitespace collapsed to single spaces
        assert!(!result.contains("  "));
        assert!(result.contains("<div>"));
        assert!(result.contains("</div>"));
        assert!(result.contains("deep"));
    }

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn minify_plugin_empty_html_file() -> Result<()> {
        // Arrange
        let temp = tempdir().unwrap();
        let html_path = temp.path().join("empty.html");
        fs::write(&html_path, "").unwrap();

        // Act
        let ctx = test_ctx_with(temp.path());
        MinifyPlugin.after_compile(&ctx).unwrap();

        // Assert — file exists, no crash
        let content = fs::read_to_string(&html_path).unwrap();
        assert!(content.is_empty());
        Ok(())
    }

    #[test]
    fn image_opti_plugin_finds_jpeg_variants() -> Result<()> {
        // Arrange
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("photo.jpg"), "JPG").unwrap();
        fs::write(temp.path().join("banner.jpeg"), "JPEG").unwrap();
        fs::write(temp.path().join("readme.txt"), "text").unwrap();
        // Extensionless entry drives the `path.extension()` None branch
        // in the verification loop below.
        fs::write(temp.path().join("LICENSE"), "MIT").unwrap();

        // Act
        let ctx = test_ctx_with(temp.path());
        ImageOptiPlugin.after_compile(&ctx).unwrap();

        // Assert — plugin runs without error (it only logs; we verify no crash)
        // Also verify both extensions are recognized by the match arm
        let mut found = Vec::new();
        for entry in fs::read_dir(temp.path()).unwrap() {
            let path = entry.unwrap().path();
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if matches!(ext.as_str(), "jpg" | "jpeg") {
                    found.push(path);
                }
            }
        }
        assert_eq!(found.len(), 2);
        Ok(())
    }

    #[test]
    fn image_opti_plugin_nested_directories() -> Result<()> {
        // Arrange — ImageOptiPlugin only reads top-level (read_dir, not recursive)
        let temp = tempdir().unwrap();
        let subdir = temp.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("deep.png"), "PNG").unwrap();
        fs::write(temp.path().join("top.png"), "PNG").unwrap();

        // Act
        let ctx = test_ctx_with(temp.path());
        ImageOptiPlugin.after_compile(&ctx).unwrap();

        // Assert — plugin completes without error; subdir images are not
        // discovered since read_dir is non-recursive
        Ok(())
    }

    #[test]
    fn deploy_plugin_custom_target() -> Result<()> {
        // Arrange
        let temp = tempdir().unwrap();
        let ctx = test_ctx_with(temp.path());
        let target_name = "staging-eu-west-1";
        let plugin = DeployPlugin::new(target_name);

        // Act — after_compile prints the target
        plugin.after_compile(&ctx).unwrap();

        // Assert — the stored target matches what was provided
        assert_eq!(plugin.target, target_name);
        Ok(())
    }

    #[test]
    fn minify_plugin_nonexistent_dir_returns_ok() -> Result<()> {
        // Arrange
        let ctx = test_ctx_with(Path::new("/this/path/does/not/exist/at/all"));

        // Act & Assert — returns Ok without error
        assert!(MinifyPlugin.after_compile(&ctx).is_ok());
        Ok(())
    }

    // -----------------------------------------------------------------
    // minify_html — additional edge cases (fallback only)
    // -----------------------------------------------------------------

    #[test]
    fn minify_html_empty_string() {
        let result = minify_html("");
        assert_eq!(result, "");
    }

    #[test]
    fn minify_html_whitespace_only() {
        let result = minify_html("   \n\t  \n  ");
        assert_eq!(result, " ");
    }

    #[test]
    fn minify_html_no_whitespace() {
        let input = "<p>hello</p>";
        let result = minify_html(input);
        assert_eq!(result, input);
    }

    #[test]
    fn minify_html_preserves_pre_with_class() {
        let input = "<pre class=\"lang-rust\">  fn main() {  }  </pre>";
        let result = minify_html(input);
        assert_eq!(result, input);
    }

    #[test]
    fn minify_html_tabs_and_newlines() {
        let input = "<div>\n\t<p>\n\t\tHello\n\t</p>\n</div>";
        let result = minify_html(input);
        assert_eq!(result, "<div> <p> Hello </p> </div>");
    }

    #[test]
    fn minify_html_mixed_whitespace_types() {
        let input = "<span>  \t\n  word  \t\n  </span>";
        let result = minify_html(input);
        assert_eq!(result, "<span> word </span>");
    }

    #[test]
    fn minify_html_single_char() {
        assert_eq!(minify_html("a"), "a");
        assert_eq!(minify_html(" "), " ");
    }

    #[test]
    fn minify_html_multiple_pre_tags() {
        let input = "<pre>a</pre><pre>b</pre>";
        let result = minify_html(input);
        assert_eq!(result, input);
    }

    // -----------------------------------------------------------------
    // MinifyPlugin — multiple HTML files
    // -----------------------------------------------------------------

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn minify_plugin_processes_multiple_html_files() -> Result<()> {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("a.html"), "<p>  hello  </p>").unwrap();
        fs::write(temp.path().join("b.html"), "<div>  world  </div>").unwrap();
        fs::write(temp.path().join("c.txt"), "  not html  ").unwrap();

        let ctx = test_ctx_with(temp.path());
        MinifyPlugin.after_compile(&ctx).unwrap();

        let a = fs::read_to_string(temp.path().join("a.html")).unwrap();
        let b = fs::read_to_string(temp.path().join("b.html")).unwrap();
        let c = fs::read_to_string(temp.path().join("c.txt")).unwrap();

        assert!(!a.contains("  "), "a.html should be minified");
        assert!(!b.contains("  "), "b.html should be minified");
        assert!(c.contains("  "), "c.txt should not be minified");
        Ok(())
    }

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn minify_plugin_whitespace_only_html_file() -> Result<()> {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("ws.html"), "   \n\t  \n  ").unwrap();

        let ctx = test_ctx_with(temp.path());
        MinifyPlugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(temp.path().join("ws.html")).unwrap();
        assert_eq!(content, " ");
        Ok(())
    }

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn minify_plugin_keeps_pre_and_minifies_the_rest() -> Result<()> {
        let temp = tempdir().unwrap();
        let original =
            "<html><pre>  keep  spaces  </pre><p>  other  </p></html>";
        fs::write(temp.path().join("pre.html"), original).unwrap();

        let ctx = test_ctx_with(temp.path());
        MinifyPlugin.after_compile(&ctx).unwrap();

        // The `<pre>` keeps every byte; the paragraph beside it does not.
        // This used to assert the whole document came back untouched,
        // because one `<pre>` anywhere disabled minification for the entire
        // page - a code block cost every other byte on it.
        let content = fs::read_to_string(temp.path().join("pre.html")).unwrap();
        assert_eq!(
            content,
            "<html><pre>  keep  spaces  </pre><p> other </p></html>"
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // ImageOptiPlugin — additional file types
    // -----------------------------------------------------------------

    #[test]
    fn image_opti_plugin_finds_gif_and_bmp() -> Result<()> {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("anim.gif"), "GIF").unwrap();
        fs::write(temp.path().join("icon.bmp"), "BMP").unwrap();
        fs::write(temp.path().join("doc.pdf"), "PDF").unwrap();
        // Extensionless entry drives the `path.extension()` None branch
        // in the verification loop below.
        fs::write(temp.path().join("Makefile"), "all:").unwrap();

        let ctx = test_ctx_with(temp.path());
        ImageOptiPlugin.after_compile(&ctx).unwrap();

        // Verify the plugin ran without error. The plugin only logs —
        // we verify it recognizes gif/bmp by not crashing and check
        // file counts manually.
        let mut count = 0;
        for entry in fs::read_dir(temp.path()).unwrap() {
            let path = entry.unwrap().path();
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if matches!(ext.as_str(), "gif" | "bmp") {
                    count += 1;
                }
            }
        }
        assert_eq!(count, 2);
        Ok(())
    }

    #[test]
    fn image_opti_plugin_empty_dir_no_crash() -> Result<()> {
        let temp = tempdir().unwrap();
        let ctx = test_ctx_with(temp.path());
        ImageOptiPlugin.after_compile(&ctx).unwrap();
        Ok(())
    }

    #[test]
    fn image_opti_plugin_no_images() -> Result<()> {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("readme.txt"), "text").unwrap();
        fs::write(temp.path().join("style.css"), "css").unwrap();

        let ctx = test_ctx_with(temp.path());
        ImageOptiPlugin.after_compile(&ctx).unwrap();
        Ok(())
    }

    #[test]
    fn image_opti_plugin_files_without_extension() -> Result<()> {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("Makefile"), "all:").unwrap();
        fs::write(temp.path().join("LICENSE"), "MIT").unwrap();

        let ctx = test_ctx_with(temp.path());
        ImageOptiPlugin.after_compile(&ctx).unwrap();
        Ok(())
    }

    // -----------------------------------------------------------------
    // DeployPlugin — additional targets
    // -----------------------------------------------------------------

    #[test]
    fn deploy_plugin_empty_target() -> Result<()> {
        let temp = tempdir().unwrap();
        let ctx = test_ctx_with(temp.path());
        let plugin = DeployPlugin::new("");
        plugin.after_compile(&ctx).unwrap();
        assert_eq!(plugin.target, "");
        Ok(())
    }

    #[test]
    fn deploy_plugin_various_targets() -> Result<()> {
        let temp = tempdir().unwrap();
        let ctx = test_ctx_with(temp.path());

        for target in ["staging", "production", "preview", "canary"] {
            let plugin = DeployPlugin::new(target);
            assert_eq!(plugin.name(), "deploy");
            assert_eq!(plugin.target, target);
            plugin.after_compile(&ctx).unwrap();
        }
        Ok(())
    }

    #[test]
    fn deploy_plugin_debug_format() {
        let plugin = DeployPlugin::new("prod");
        let debug = format!("{plugin:?}");
        assert!(debug.contains("prod"));
    }

    // -----------------------------------------------------------------
    // MinifyPlugin / ImageOptiPlugin — trait object coverage
    // -----------------------------------------------------------------

    #[test]
    fn minify_plugin_copy_clone() {
        let a = MinifyPlugin;
        let b = a;
        // Cloning a Copy type is the point: this asserts `Clone` is wired up,
        // not that cloning is the efficient way to get a second value.
        #[allow(clippy::clone_on_copy)]
        let c = a.clone();
        assert_eq!(a.name(), b.name());
        assert_eq!(a.name(), c.name());
    }

    #[test]
    fn minify_plugin_debug_format() {
        let debug = format!("{:?}", MinifyPlugin);
        assert!(debug.contains("MinifyPlugin"));
    }

    #[test]
    fn image_opti_plugin_copy_clone() {
        let a = ImageOptiPlugin;
        let b = a;
        // Cloning a Copy type is the point: this asserts `Clone` is wired up,
        // not that cloning is the efficient way to get a second value.
        #[allow(clippy::clone_on_copy)]
        let c = a.clone();
        assert_eq!(a.name(), b.name());
        assert_eq!(a.name(), c.name());
    }

    #[test]
    fn image_opti_plugin_debug_format() {
        let debug = format!("{:?}", ImageOptiPlugin);
        assert!(debug.contains("ImageOptiPlugin"));
    }

    #[test]
    fn test_minify_plugin_read_dir_error() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("not_a_dir");
        fs::write(&file_path, "").unwrap();
        let ctx = test_ctx_with(&file_path);
        let res = MinifyPlugin.after_compile(&ctx);
        assert!(res.is_err());
    }

    #[test]
    fn test_image_opti_plugin_read_dir_error() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("not_a_dir");
        fs::write(&file_path, "").unwrap();
        let ctx = test_ctx_with(&file_path);
        let res = ImageOptiPlugin.after_compile(&ctx);
        assert!(res.is_err());
    }

    // -----------------------------------------------------------------
    // `minify` feature — happy paths (only compiled with the feature)
    // -----------------------------------------------------------------

    #[test]
    fn minify_html_preserves_pre_content_bit_identical() {
        let body = "fn main() {\n    println!(\"hi\");\n}";
        let input =
            format!("<html><body><pre><code>{body}</code></pre></body></html>");
        let out = minify_html(&input);
        // The exact whitespace inside <pre><code>…</code></pre> must
        // survive minification untouched. We only check containment
        // because minify-html may rewrite attributes outside the pre.
        assert!(
            out.contains(body),
            "minified output must preserve <pre> body byte-for-byte:\n{out}"
        );
    }

    #[test]
    fn minify_css_compresses_input() {
        let input =
            "body  {\n  color:   red;\n  margin:  0px  0px  0px  0px;\n}";
        let out = minify_css(input);
        assert!(out.len() < input.len());
        assert!(out.contains("red"));
    }

    #[test]
    fn minify_js_compresses_input() {
        let input = "const greeting = 'hello world';\nconsole.log(greeting);";
        let out = minify_js(input);
        assert!(out.len() < input.len());
    }

    #[test]
    fn minify_plugin_recursive_walk_processes_nested_html() -> Result<()> {
        let temp = tempdir().unwrap();
        let deep = temp.path().join("blog").join("2026").join("post");
        fs::create_dir_all(&deep).unwrap();
        let nested = deep.join("index.html");
        fs::write(
            &nested,
            "<html>  <body>   <p>   nested   </p>   </body>   </html>",
        )
        .unwrap();
        let top = temp.path().join("index.html");
        fs::write(&top, "<html>  <body>   <p>   top   </p>   </body></html>")
            .unwrap();

        let ctx = test_ctx_with(temp.path());
        MinifyPlugin.after_compile(&ctx).unwrap();

        let nested_after = fs::read_to_string(&nested).unwrap();
        // Nested file must have been touched (size strictly smaller).
        assert!(
            nested_after.len()
                < "<html>  <body>   <p>   nested   </p>   </body>   </html>"
                    .len(),
            "nested file should have been minified: {nested_after}"
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // MinifyPlugin — per-file read/write error propagation
    // -----------------------------------------------------------------

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn minify_plugin_read_failure_on_invalid_utf8_html() {
        let temp = tempdir().unwrap();
        // Invalid UTF-8 makes read_to_string fail inside the html pass.
        fs::write(temp.path().join("broken.html"), [0xFF, 0xFE, 0xFD]).unwrap();

        let ctx = test_ctx_with(temp.path());
        let err = MinifyPlugin
            .after_compile(&ctx)
            .expect_err("invalid UTF-8 html must surface a read error");
        let msg = format!("{err:?}");
        assert!(msg.contains("broken.html"), "path context expected: {msg}");
    }

    #[test]
    #[cfg(unix)]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn minify_plugin_write_failure_on_readonly_html() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let file = temp.path().join("locked.html");
        fs::write(&file, "<p>  spaced  out  </p>").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();

        let ctx = test_ctx_with(temp.path());
        let result = MinifyPlugin.after_compile(&ctx);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let err =
            result.expect_err("write to a read-only html file must surface");
        let msg = format!("{err:?}");
        assert!(msg.contains("locked.html"), "path context expected: {msg}");
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::plugin::PluginContext;
    use serial_test::serial;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop (mirrors the
    /// convention in `tests/fault_injection.rs`).
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial]
    fn minify_read_failpoint_propagates() {
        let _guard = FailGuard("plugins::minify-read");
        fail::cfg("plugins::minify-read", "return")
            .expect("activate failpoint");

        let dir = tempdir().unwrap();
        fs::write(dir.path().join("index.html"), "<p>x</p>").unwrap();

        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let err = MinifyPlugin
            .after_compile(&ctx)
            .expect_err("injected read failure must propagate");
        assert!(format!("{err:?}").contains("injected: plugins::minify-read"));
    }

    #[test]
    #[serial]
    fn minify_write_failpoint_propagates() {
        let _guard = FailGuard("plugins::minify-write");
        fail::cfg("plugins::minify-write", "return")
            .expect("activate failpoint");

        let dir = tempdir().unwrap();
        fs::write(dir.path().join("index.html"), "<p>x</p>").unwrap();

        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let err = MinifyPlugin
            .after_compile(&ctx)
            .expect_err("injected write failure must propagate");
        assert!(format!("{err:?}").contains("injected: plugins::minify-write"));
    }
}
