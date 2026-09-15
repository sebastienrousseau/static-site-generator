// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Template engine integration (`MiniJinja`).
//!
//! Wraps the [MiniJinja](https://docs.rs/minijinja) template engine to
//! provide Jinja2-style templating with inheritance, conditionals, loops,
//! partials, and custom filters for static site generation.

#[cfg(feature = "templates")]
use anyhow::{Context, Result};
#[cfg(feature = "templates")]
use std::{collections::HashMap, path::PathBuf};

/// Configuration for the template engine.
#[cfg(feature = "templates")]
#[derive(Debug, Clone)]
pub struct TemplateConfig {
    /// Directory containing templates.
    pub template_dir: PathBuf,
    /// Global variables injected into every template context.
    pub globals: HashMap<String, serde_json::Value>,
    /// Whether to enable HTML auto-escaping (default: true).
    pub autoescape: bool,
}

#[cfg(feature = "templates")]
impl Default for TemplateConfig {
    fn default() -> Self {
        Self {
            template_dir: PathBuf::from("templates/tera"),
            globals: HashMap::new(),
            autoescape: true,
        }
    }
}

/// Wraps `MiniJinja` and provides site-generation-specific rendering.
#[cfg(feature = "templates")]
#[derive(Debug)]
pub struct TemplateEngine {
    env: minijinja::Environment<'static>,
    config: TemplateConfig,
}

#[cfg(feature = "templates")]
impl TemplateEngine {
    /// Initializes the template engine from a template directory.
    ///
    /// Uses a path-based loader for lazy template resolution.
    /// Returns `Ok(None)` if the template directory does not exist
    /// (graceful fallback for projects without templates).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::template_engine::{TemplateConfig, TemplateEngine};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let cfg = TemplateConfig {
    ///     template_dir: dir.path().join("missing"),
    ///     ..TemplateConfig::default()
    /// };
    /// // Missing dir ⇒ Ok(None), never an error.
    /// assert!(TemplateEngine::init(cfg).unwrap().is_none());
    /// ```
    pub fn init(config: TemplateConfig) -> Result<Option<Self>> {
        if !config.template_dir.exists() {
            return Ok(None);
        }

        let mut env = minijinja::Environment::new();
        env.set_loader(minijinja::path_loader(&config.template_dir));

        if !config.autoescape {
            env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
        }

        // Register custom filters
        env.add_filter("reading_time", reading_time_filter);
        env.add_filter("slugify", slugify_filter);

        Ok(Some(Self { env, config }))
    }

    /// Renders a page through the template chain.
    ///
    /// # Arguments
    /// * `template_name` — template to render (e.g. `"page.html"`)
    /// * `page_content` — compiled HTML content from staticdatagen
    /// * `frontmatter` — parsed frontmatter as JSON key-value pairs
    /// * `site_globals` — site-level variables (name, `base_url`, etc.)
    ///
    /// # Per-page language (spec A5, plan §2 1.5)
    ///
    /// Inside the render context, `site.language` and `page.language`
    /// both carry the page's *resolved* language rather than the raw
    /// site-wide default: front-matter `language` wins, then
    /// front-matter `hreflang`, then the `site.language` global, then
    /// `"en"` — normalised to BCP-47 hyphen form. Because the default
    /// templates emit `<html lang="{{ site.language }}">`, a `/hi/…`
    /// page with front-matter `language: hi` renders
    /// `<html lang="hi">` even when the site default is `en-GB`, so
    /// `<html lang>` agrees with the JSON-LD `inLanguage`,
    /// `og:locale`, and hreflang self-reference sinks that resolve
    /// through `seo::lang::resolve_page_lang`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::template_engine::{TemplateConfig, TemplateEngine};
    /// use tempfile::tempdir;
    /// use std::collections::HashMap;
    /// use std::fs;
    ///
    /// let dir = tempdir().unwrap();
    /// let cfg = TemplateConfig {
    ///     template_dir: dir.path().to_path_buf(),
    ///     ..TemplateConfig::default()
    /// };
    /// // No matching template ⇒ returns the content unchanged.
    /// let engine = TemplateEngine::init(cfg).unwrap().unwrap();
    /// let html = engine.render_page("p.html", "<p>hi</p>", &HashMap::new(), &HashMap::new()).unwrap();
    /// assert!(html.contains("hi"));
    /// ```
    pub fn render_page(
        &self,
        template_name: &str,
        page_content: &str,
        frontmatter: &HashMap<String, serde_json::Value>,
        site_globals: &HashMap<String, serde_json::Value>,
    ) -> Result<String> {
        // Resolve the page language once so every `<html lang>`
        // emitter publishes the per-page value (spec A5, plan §2 1.5):
        // front-matter `language` → front-matter `hreflang` → site
        // default → "en", normalised to BCP-47 hyphen form. This is
        // the emitter-side subset of `seo::lang::resolve_page_lang`.
        let resolved_lang = crate::core_group::lang::resolve_render_lang(
            frontmatter,
            site_globals
                .get("language")
                .and_then(serde_json::Value::as_str),
        );

        // Build page context
        let mut page: serde_json::Map<String, serde_json::Value> = frontmatter
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let _ = page.insert(
            "content".to_string(),
            serde_json::Value::String(page_content.to_string()),
        );
        let _ = page.insert(
            "language".to_string(),
            serde_json::Value::String(resolved_lang.clone()),
        );
        // Writing direction for the resolved language. Without it a
        // template has no way to emit `dir` short of hardcoding a
        // language list, and an Arabic or Hebrew page renders
        // left-to-right.
        let direction = crate::core_group::lang::text_direction(&resolved_lang);
        let _ = page.insert(
            "direction".to_string(),
            serde_json::Value::String(direction.to_string()),
        );

        // Build the full render context. `site.language` carries the
        // per-page resolved language so templates that emit
        // `<html lang="{{ site.language }}">` (scaffold base.html and
        // user templates alike) publish the page's language without
        // needing template changes.
        let mut site: serde_json::Map<String, serde_json::Value> = site_globals
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let _ = site.insert(
            "language".to_string(),
            serde_json::Value::String(resolved_lang),
        );
        let _ = site.insert(
            "direction".to_string(),
            serde_json::Value::String(direction.to_string()),
        );

        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("page".to_string(), serde_json::Value::Object(page));
        let _ = ctx.insert("site".to_string(), serde_json::Value::Object(site));

        // Inject global config variables at top level
        for (k, v) in &self.config.globals {
            let _ = ctx.insert(k.clone(), v.clone());
        }

        // The bundled templates address `{{language}}` and
        // `{{direction}}` bare rather than through `site.`, so both have
        // to exist at top level. Inserted *after* the globals loop: a
        // stale `direction` global must not shadow the value resolved
        // for this page.
        let _ = ctx.insert(
            "direction".to_string(),
            serde_json::Value::String(direction.to_string()),
        );

        // Determine which template to use, fall back to page.html.
        // Single lookup per candidate — the successful `get_template`
        // result is reused directly, so the resolve and load steps can
        // never disagree.
        let (tmpl_name, tmpl) =
            if let Ok(t) = self.env.get_template(template_name) {
                (template_name, t)
            } else if let Ok(t) = self.env.get_template("page.html") {
                ("page.html", t)
            } else {
                // No matching template — return content as-is
                return Ok(page_content.to_string());
            };

        tmpl.render(serde_json::Value::Object(ctx))
            .with_context(|| format!("Failed to render template '{tmpl_name}'"))
    }

    /// Reports whether `name` resolves to a loadable template.
    ///
    /// Callers use this to distinguish "rendered through a template"
    /// from [`render_page`](Self::render_page)'s pass-through arm,
    /// which hands the input back unchanged when neither the requested
    /// template nor the `page.html` fallback exists.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::template_engine::{TemplateConfig, TemplateEngine};
    /// use tempfile::tempdir;
    /// use std::fs;
    ///
    /// let dir = tempdir().unwrap();
    /// fs::write(dir.path().join("page.html"), "{{ page.content }}").unwrap();
    /// let cfg = TemplateConfig {
    ///     template_dir: dir.path().to_path_buf(),
    ///     ..TemplateConfig::default()
    /// };
    /// let engine = TemplateEngine::init(cfg).unwrap().unwrap();
    /// assert!(engine.has_template("page.html"));
    /// assert!(!engine.has_template("missing.html"));
    /// ```
    #[must_use]
    pub fn has_template(&self, name: &str) -> bool {
        self.env.get_template(name).is_ok()
    }

    /// Builds site-level globals from an `SsgConfig`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::cmd::SsgConfig;
    /// use ssg::template_engine::TemplateEngine;
    ///
    /// let cfg = SsgConfig::default();
    /// let globals = TemplateEngine::site_globals_from_config(&cfg);
    /// assert!(globals.contains_key("name"));
    /// assert!(globals.contains_key("base_url"));
    /// ```
    #[must_use]
    pub fn site_globals_from_config(
        config: &crate::cmd::SsgConfig,
    ) -> HashMap<String, serde_json::Value> {
        let mut globals = HashMap::new();
        let _ = globals.insert(
            "name".to_string(),
            serde_json::Value::String(config.site_name.clone()),
        );
        let _ = globals.insert(
            "title".to_string(),
            serde_json::Value::String(config.site_title.clone()),
        );
        let _ = globals.insert(
            "description".to_string(),
            serde_json::Value::String(config.site_description.clone()),
        );
        let _ = globals.insert(
            "base_url".to_string(),
            serde_json::Value::String(config.base_url.clone()),
        );
        let _ = globals.insert(
            "language".to_string(),
            serde_json::Value::String(config.language.clone()),
        );
        globals
    }

    /// Loads data files from a `data/` directory into the context.
    ///
    /// Supports `.toml`, `.json`, and `.yml`/`.yaml` files.
    /// Files are accessible as `{{ data.filename }}` in templates.
    ///
    /// Example: `data/nav.toml` → `{{ data.nav.links }}`
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::template_engine::TemplateEngine;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// // Returns empty map when no sibling `data/` dir exists.
    /// let data = TemplateEngine::load_data_files(dir.path());
    /// assert!(data.is_empty());
    /// ```
    #[must_use]
    pub fn load_data_files(
        content_dir: &std::path::Path,
    ) -> HashMap<String, serde_json::Value> {
        let data_dir = content_dir.parent().unwrap_or(content_dir).join("data");
        let mut data = HashMap::new();

        if !data_dir.exists() {
            return data;
        }

        let Ok(entries) = std::fs::read_dir(&data_dir) else {
            return data;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let ext = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();

            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };

            let value: Option<serde_json::Value> = match ext.as_str() {
                "toml" => match toml::from_str::<serde_json::Value>(&content) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        log::warn!(
                            "Failed to parse data file {}: {e}",
                            path.display()
                        );
                        None
                    }
                },
                "json" => match serde_json::from_str(&content) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        log::warn!(
                            "Failed to parse data file {}: {e}",
                            path.display()
                        );
                        None
                    }
                },
                "yml" | "yaml" => {
                    match noyalib::from_str::<serde_json::Value>(&content) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            log::warn!(
                                "Failed to parse data file {}: {e}",
                                path.display()
                            );
                            None
                        }
                    }
                }
                _ => None,
            };

            if let Some(val) = value {
                let _ = data.insert(stem, val);
            }
        }

        data
    }
}

/// Custom filter: estimates reading time in minutes.
///
/// Usage: `{{ page.content | reading_time }}`
/// Returns a string like "3 min read".
#[cfg(feature = "templates")]
fn reading_time_filter(value: String) -> String {
    let word_count = value.split_whitespace().count();
    let minutes = (word_count / 200).max(1);
    format!("{minutes} min read")
}

/// Custom filter: converts a string to a URL-safe slug.
///
/// Usage: `{{ tag | slugify }}`
#[cfg(feature = "templates")]
fn slugify_filter(value: String) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(all(test, feature = "templates"))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn setup_templates(dir: &Path) {
        crate::test_support::init_logger();
        let tera_dir = dir.join("tera");
        fs::create_dir_all(&tera_dir).unwrap();

        fs::write(
            tera_dir.join("base.html"),
            r#"<!DOCTYPE html>
<html lang="{{ site.language | default("en") }}">
<head><title>{% block title %}{{ page.title | default("Untitled") }}{% endblock %}</title>
{% block head_extra %}{% endblock %}
</head>
<body>
<main>{% block content %}{% endblock %}</main>
<footer>{% block footer %}<p>&copy; {{ site.name | default("") }}</p>{% endblock %}</footer>
</body>
</html>"#,
        )
        .unwrap();

        fs::write(
            tera_dir.join("page.html"),
            r#"{% extends "base.html" %}
{% block content %}{{ page.content | safe }}{% endblock %}"#,
        )
        .unwrap();

        fs::write(
            tera_dir.join("post.html"),
            r#"{% extends "base.html" %}
{% block content %}
<article>
<h1>{{ page.title | default("") }}</h1>
<time>{{ page.date | default("") }}</time>
<p>{{ page.content | reading_time }}</p>
{{ page.content | safe }}
</article>
{% endblock %}"#,
        )
        .unwrap();
    }

    #[test]
    fn test_init_missing_dir() {
        let config = TemplateConfig {
            template_dir: PathBuf::from("/nonexistent/path"),
            ..Default::default()
        };
        let result = TemplateEngine::init(config).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_init_and_render_page() {
        let dir = tempdir().unwrap();
        setup_templates(dir.path());

        let config = TemplateConfig {
            template_dir: dir.path().join("tera"),
            ..Default::default()
        };
        let engine = TemplateEngine::init(config).unwrap().unwrap();

        let mut fm = HashMap::new();
        let _ = fm.insert(
            "title".to_string(),
            serde_json::Value::String("Hello".to_string()),
        );

        let mut site = HashMap::new();
        let _ = site.insert(
            "name".to_string(),
            serde_json::Value::String("My Site".to_string()),
        );
        let _ = site.insert(
            "language".to_string(),
            serde_json::Value::String("en-GB".to_string()),
        );

        let result = engine
            .render_page("page.html", "<p>Body</p>", &fm, &site)
            .unwrap();

        assert!(result.contains("Hello"));
        assert!(result.contains("<p>Body</p>"));
        assert!(result.contains("My Site"));
        assert!(result.contains("en-GB"));
    }

    // ── Per-page <html lang> (spec A5, plan §2 1.5) ────────────────

    fn engine_with_default_templates(dir: &Path) -> TemplateEngine {
        setup_templates(dir);
        let config = TemplateConfig {
            template_dir: dir.join("tera"),
            ..Default::default()
        };
        TemplateEngine::init(config).unwrap().unwrap()
    }

    fn site_with_language(lang: &str) -> HashMap<String, serde_json::Value> {
        let mut site = HashMap::new();
        let _ = site.insert(
            "language".to_string(),
            serde_json::Value::String(lang.to_string()),
        );
        site
    }

    #[test]
    fn frontmatter_language_wins_over_site_language_in_html_lang() {
        // The A5 acceptance shape: a page with front-matter
        // `language: hi` must render `<html lang="hi">` even when the
        // site-wide default is en-GB.
        let dir = tempdir().unwrap();
        let engine = engine_with_default_templates(dir.path());

        let mut fm = HashMap::new();
        let _ = fm.insert(
            "language".to_string(),
            serde_json::Value::String("hi".to_string()),
        );

        let result = engine
            .render_page(
                "page.html",
                "<p>B</p>",
                &fm,
                &site_with_language("en-GB"),
            )
            .unwrap();
        assert!(
            result.contains(r#"<html lang="hi">"#),
            "front-matter language must win: {result}"
        );
        assert!(!result.contains(r#"lang="en-GB""#));
    }

    #[test]
    fn frontmatter_hreflang_used_when_language_absent() {
        let dir = tempdir().unwrap();
        let engine = engine_with_default_templates(dir.path());

        let mut fm = HashMap::new();
        let _ = fm.insert(
            "hreflang".to_string(),
            serde_json::Value::String("fr_fr".to_string()),
        );

        let result = engine
            .render_page(
                "page.html",
                "<p>B</p>",
                &fm,
                &site_with_language("en"),
            )
            .unwrap();
        // Also normalised to BCP-47 hyphen form.
        assert!(
            result.contains(r#"<html lang="fr-FR">"#),
            "hreflang should be used and normalised: {result}"
        );
    }

    #[test]
    fn site_language_used_when_page_has_no_lang_signal() {
        let dir = tempdir().unwrap();
        let engine = engine_with_default_templates(dir.path());

        let result = engine
            .render_page(
                "page.html",
                "<p>B</p>",
                &HashMap::new(),
                &site_with_language("en-GB"),
            )
            .unwrap();
        assert!(result.contains(r#"<html lang="en-GB">"#));
    }

    #[test]
    fn en_fallback_when_no_language_anywhere() {
        let dir = tempdir().unwrap();
        let engine = engine_with_default_templates(dir.path());

        let result = engine
            .render_page(
                "page.html",
                "<p>B</p>",
                &HashMap::new(),
                &HashMap::new(),
            )
            .unwrap();
        assert!(result.contains(r#"<html lang="en">"#));
    }

    #[test]
    fn test_render_post_with_reading_time() {
        let dir = tempdir().unwrap();
        setup_templates(dir.path());

        let config = TemplateConfig {
            template_dir: dir.path().join("tera"),
            ..Default::default()
        };
        let engine = TemplateEngine::init(config).unwrap().unwrap();

        let content = "word ".repeat(600); // ~3 min read
        let mut fm = HashMap::new();
        let _ = fm.insert(
            "title".to_string(),
            serde_json::Value::String("Post".to_string()),
        );
        let _ = fm.insert(
            "date".to_string(),
            serde_json::Value::String("2026-01-01".to_string()),
        );

        let site = HashMap::new();
        let result = engine
            .render_page("post.html", &content, &fm, &site)
            .unwrap();

        assert!(result.contains("3 min read"));
        assert!(result.contains("<article>"));
    }

    #[test]
    fn test_fallback_to_page_html() {
        let dir = tempdir().unwrap();
        setup_templates(dir.path());

        let config = TemplateConfig {
            template_dir: dir.path().join("tera"),
            ..Default::default()
        };
        let engine = TemplateEngine::init(config).unwrap().unwrap();

        let fm = HashMap::new();
        let site = HashMap::new();
        let result = engine
            .render_page("nonexistent.html", "<p>fallback</p>", &fm, &site)
            .unwrap();

        assert!(result.contains("<p>fallback</p>"));
    }

    #[test]
    fn test_reading_time_filter_direct() {
        let text = "word ".repeat(400);
        let result = reading_time_filter(text);
        assert_eq!(result, "2 min read");
    }

    #[test]
    fn test_slugify_filter() {
        assert_eq!(slugify_filter("Hello World!".to_string()), "hello-world");
        assert_eq!(slugify_filter("Rust & Web".to_string()), "rust-web");
    }

    // -------------------------------------------------------------------
    // load_data_files — format + fallback coverage
    // -------------------------------------------------------------------

    #[test]
    fn load_data_files_missing_data_dir_returns_empty_map() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let result = TemplateEngine::load_data_files(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn load_data_files_parses_toml_and_json_and_yaml() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();

        fs::write(data.join("site.toml"), r#"key = "toml-value""#).unwrap();
        fs::write(data.join("nav.json"), r#"{"items": ["home", "about"]}"#)
            .unwrap();
        fs::write(data.join("conf.yml"), r#"{"yaml": "value"}"#).unwrap();
        fs::write(data.join("ignored.txt"), "not parsed").unwrap();

        let sub = data.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("inside.json"), "{}").unwrap();

        let result = TemplateEngine::load_data_files(&content);
        assert!(result.contains_key("site"));
        assert!(result.contains_key("nav"));
        assert!(result.contains_key("conf"));
        assert!(!result.contains_key("ignored"));
        assert!(!result.contains_key("sub"));
    }

    #[test]
    fn load_data_files_skips_files_with_invalid_content() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();

        fs::write(data.join("broken.toml"), "not valid toml [[[").unwrap();
        fs::write(data.join("broken.json"), "{not valid").unwrap();
        fs::write(data.join("good.toml"), r#"x = "y""#).unwrap();

        let result = TemplateEngine::load_data_files(&content);
        assert!(result.contains_key("good"));
        assert!(!result.contains_key("broken"));
    }

    #[test]
    fn load_data_files_skips_non_utf8_file() {
        // A file whose bytes are not valid UTF-8 makes
        // `read_to_string` fail, taking the `continue` arm.
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();

        fs::write(data.join("binary.toml"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();
        fs::write(data.join("ok.toml"), r#"k = "v""#).unwrap();

        let result = TemplateEngine::load_data_files(&content);
        assert!(result.contains_key("ok"));
        assert!(!result.contains_key("binary"));
    }

    #[test]
    fn load_data_files_skips_invalid_yaml() {
        // Exercises the YAML parse-error arm (including the
        // `log::warn!` format arguments).
        crate::test_support::init_logger();
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();

        fs::write(data.join("broken.yml"), "key: [unclosed").unwrap();
        fs::write(data.join("good.yaml"), "k: v").unwrap();

        let result = TemplateEngine::load_data_files(&content);
        assert!(result.contains_key("good"));
        assert!(!result.contains_key("broken"));
    }

    #[test]
    fn load_data_files_ignores_unsupported_extensions() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();

        fs::write(data.join("a.xml"), "<x/>").unwrap();
        fs::write(data.join("b.csv"), "a,b").unwrap();
        fs::write(data.join("c"), "no extension").unwrap();

        let result = TemplateEngine::load_data_files(&content);
        assert!(result.is_empty());
    }

    // -------------------------------------------------------------------
    // render_page — custom globals + no-fallback branch
    // -------------------------------------------------------------------

    #[test]
    fn render_page_injects_custom_globals_from_config() {
        let dir = tempdir().unwrap();
        setup_templates(dir.path());

        // Write a minimal template that references the custom global.
        fs::write(
            dir.path().join("tera").join("branded.html"),
            r"<p>{{ brand }}</p>",
        )
        .unwrap();

        let config = TemplateConfig {
            template_dir: dir.path().join("tera"),
            globals: {
                let mut g = HashMap::new();
                let _ = g.insert(
                    "brand".to_string(),
                    serde_json::Value::String("Acme".to_string()),
                );
                g
            },
            ..Default::default()
        };
        let engine = TemplateEngine::init(config).unwrap().unwrap();

        let result = engine
            .render_page("branded.html", "", &HashMap::new(), &HashMap::new())
            .unwrap();
        assert!(result.contains("Acme"));
    }

    #[test]
    fn render_page_no_matching_template_and_no_page_html_returns_content_as_is()
    {
        let dir = tempdir().unwrap();
        let tera_dir = dir.path().join("tera");
        fs::create_dir_all(&tera_dir).unwrap();
        // Only write a `base.html`, NOT a `page.html`.
        fs::write(
            tera_dir.join("base.html"),
            r"<!DOCTYPE html><html><body>{% block content %}{% endblock %}</body></html>",
        )
        .unwrap();

        let config = TemplateConfig {
            template_dir: tera_dir,
            ..Default::default()
        };
        let engine = TemplateEngine::init(config).unwrap().unwrap();

        let content = "<p>raw content</p>";
        let result = engine
            .render_page(
                "nonexistent.html",
                content,
                &HashMap::new(),
                &HashMap::new(),
            )
            .unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn init_with_autoescape_false() {
        let dir = tempdir().unwrap();
        setup_templates(dir.path());

        let config = TemplateConfig {
            template_dir: dir.path().join("tera"),
            autoescape: false,
            ..Default::default()
        };
        let engine = TemplateEngine::init(config).unwrap().unwrap();
        let result = engine
            .render_page(
                "page.html",
                "<p>x</p>",
                &HashMap::new(),
                &HashMap::new(),
            )
            .unwrap();
        assert!(result.contains("<p>x</p>"));
    }

    #[test]
    fn init_with_broken_template_errors_on_render() {
        let dir = tempdir().unwrap();
        let tera_dir = dir.path().join("tera");
        fs::create_dir_all(&tera_dir).unwrap();
        // Use an extends to a non-existent parent — always errors on render
        fs::write(tera_dir.join("broken.html"), "{% extends \"nonexistent_parent.html\" %}{% block x %}{% endblock %}").unwrap();

        let config = TemplateConfig {
            template_dir: tera_dir,
            ..Default::default()
        };
        // MiniJinja uses lazy loading — init succeeds
        let engine = TemplateEngine::init(config).unwrap().unwrap();
        // Error surfaces at render time
        let result = engine.render_page(
            "broken.html",
            "",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    #[cfg(unix)]
    fn load_data_files_unreadable_file_continues_silently() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();

        fs::create_dir_all(data.join("not-really.toml")).unwrap();
        fs::write(data.join("real.toml"), r#"k = "v""#).unwrap();

        let result = TemplateEngine::load_data_files(&content);
        assert!(result.contains_key("real"));
        assert!(!result.contains_key("not-really"));
    }

    #[test]
    fn load_data_files_data_dir_is_a_file_returns_empty() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let data = dir.path().join("data");
        fs::write(&data, "I am a file, not a directory").unwrap();

        let result = TemplateEngine::load_data_files(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn render_page_propagates_render_errors() {
        let dir = tempdir().unwrap();
        let tera_dir = dir.path().join("tera");
        fs::create_dir_all(&tera_dir).unwrap();
        // Undefined filter → render fails
        fs::write(
            tera_dir.join("broken.html"),
            r"{{ page.title | nonexistent_filter }}",
        )
        .unwrap();

        let config = TemplateConfig {
            template_dir: tera_dir,
            ..Default::default()
        };
        let engine = TemplateEngine::init(config).unwrap().unwrap();

        let mut fm = HashMap::new();
        let _ = fm.insert(
            "title".to_string(),
            serde_json::Value::String("T".to_string()),
        );

        let result =
            engine.render_page("broken.html", "", &fm, &HashMap::new());
        assert!(result.is_err());
    }
}
