// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Template rendering plugin.
//!
//! Post-processes compiled HTML through templates, enabling
//! template inheritance, conditionals, loops, and filters.

#[cfg(feature = "templates")]
use crate::{
    error::{PathErrorExt, SsgError},
    frontmatter,
    plugin::{Plugin, PluginContext},
    template_engine::{TemplateConfig, TemplateEngine},
    MAX_DIR_DEPTH,
};

#[cfg(feature = "templates")]
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// Plugin that post-processes compiled HTML through templates.
///
/// Runs in the `after_compile` phase. For each HTML file in `site_dir`:
/// 1. Reads the companion `.meta.json` sidecar (from frontmatter extraction)
/// 2. Determines the layout from frontmatter (`layout` field, default: `page`)
/// 3. Renders the HTML through the template chain
/// 4. Writes the rendered result back to the same file
///
/// Falls back gracefully if no templates directory exists.
#[cfg(feature = "templates")]
#[derive(Debug)]
pub struct TemplatePlugin {
    config: TemplateConfig,
}

#[cfg(feature = "templates")]
impl TemplatePlugin {
    /// Creates a new `TemplatePlugin` with the given configuration.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::Plugin;
    /// use ssg::template_engine::TemplateConfig;
    /// use ssg::template_plugin::TemplatePlugin;
    ///
    /// let p = TemplatePlugin::new(TemplateConfig::default());
    /// assert_eq!(p.name(), "templates");
    /// ```
    #[must_use]
    pub const fn new(config: TemplateConfig) -> Self {
        Self { config }
    }

    /// Creates a `TemplatePlugin` that looks for templates in the standard
    /// `templates/tera/` subdirectory of the template dir.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::Plugin;
    /// use ssg::template_plugin::TemplatePlugin;
    /// use std::path::Path;
    ///
    /// let p = TemplatePlugin::from_template_dir(Path::new("templates"));
    /// assert_eq!(p.name(), "templates");
    /// ```
    #[must_use]
    pub fn from_template_dir(template_dir: &Path) -> Self {
        Self {
            config: TemplateConfig {
                template_dir: template_dir.join("tera"),
                ..Default::default()
            },
        }
    }
}

#[cfg(feature = "templates")]
impl Plugin for TemplatePlugin {
    fn name(&self) -> &'static str {
        "templates"
    }

    fn before_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        // Emit .meta.json sidecars for all markdown content
        let sidecar_dir = ctx.build_dir.join(".meta");
        let count = frontmatter::emit_sidecars(&ctx.content_dir, &sidecar_dir)
            .map_err(|e| SsgError::io(e, &sidecar_dir))?;
        if count > 0 {
            log::info!("[templates] Emitted {count} frontmatter sidecar(s)");
        }
        Ok(())
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        let Some(engine) = TemplateEngine::init(self.config.clone())
            .map_err(|e| SsgError::io(e, &self.config.template_dir))?
        else {
            log::info!(
                "[templates] No templates at {}, skipping",
                self.config.template_dir.display()
            );
            return Ok(());
        };

        // Build site-level globals from config
        let mut site_globals = ctx
            .config
            .as_ref()
            .map(TemplateEngine::site_globals_from_config)
            .unwrap_or_default();

        // Load data files (data/*.toml, data/*.json) into context
        let data_files = TemplateEngine::load_data_files(&ctx.content_dir);
        if !data_files.is_empty() {
            let _ = site_globals.insert(
                "data".to_string(),
                serde_json::Value::Object(data_files.into_iter().collect()),
            );
        }

        let sidecar_dir = resolve_sidecar_dir(ctx);
        let html_files = collect_html_files(&ctx.site_dir)?;
        let enriched_fm_map =
            enrich_with_related_posts(&html_files, &ctx.site_dir, &sidecar_dir);

        let mut rendered = 0usize;
        let mut skipped = 0usize;
        for html_path in &html_files {
            let content = fs::read_to_string(html_path).with_path(html_path)?;

            // Read frontmatter sidecar (enriched with related posts)
            let fm =
                enriched_fm_map.get(html_path).cloned().unwrap_or_else(|| {
                    read_frontmatter_for_html(
                        html_path,
                        &ctx.site_dir,
                        &sidecar_dir,
                    )
                });

            // Determine template from `layout` field
            let layout =
                fm.get("layout").and_then(|v| v.as_str()).unwrap_or("page");
            let template_name = format!("{layout}.html");

            // Resolving to neither the requested layout nor the
            // `page.html` fallback means `render_page` would hand the
            // input straight back. Counting that as a render made a
            // no-op pipeline indistinguishable from a working one, so
            // report it instead of inflating the success tally.
            if !engine.has_template(&template_name)
                && !engine.has_template("page.html")
            {
                log::warn!(
                    "[templates] No template '{template_name}' (and no page.html fallback) for {}; leaving compiled output untouched",
                    html_path.display()
                );
                skipped += 1;
                continue;
            }

            match engine.render_page(
                &template_name,
                &content,
                &fm,
                &site_globals,
            ) {
                Ok(output) => {
                    fs::write(html_path, output).with_path(html_path)?;
                    rendered += 1;
                }
                Err(e) => {
                    log::warn!(
                        "[templates] Failed to render {}: {e}",
                        html_path.display()
                    );
                }
            }
        }

        if rendered > 0 {
            log::info!("[templates] Rendered {rendered} page(s)");
        }
        if skipped > 0 {
            log::warn!(
                "[templates] Skipped {skipped} page(s) with no matching template"
            );
        }
        Ok(())
    }
}

/// Resolves the directory holding the `.meta.json` frontmatter sidecars.
///
/// `before_compile` writes the sidecars under `<build_dir>/.meta`, but
/// `staticdatagen` *promotes* the build directory onto `site_dir` when
/// the two differ (the default `output.build-tmp` → `output` case), and
/// the sidecars travel with it. Reading `<build_dir>/.meta`
/// unconditionally therefore missed every sidecar on a default build:
/// `layout` silently fell back to `page` for every page, so a theme
/// whose layouts were `index`/`about`/`contact` rendered through none of
/// them — and, absent a `page.html`, through nothing at all.
///
/// Prefer the build directory (it still exists for `--serve` builds,
/// where `site_dir != output_dir` and no promotion happens) and fall
/// back to the site directory once the promotion has taken place.
#[cfg(feature = "templates")]
fn resolve_sidecar_dir(ctx: &PluginContext) -> PathBuf {
    let build_meta = ctx.build_dir.join(".meta");
    if build_meta.is_dir() {
        build_meta
    } else {
        ctx.site_dir.join(".meta")
    }
}

/// Reads frontmatter for an HTML file, trying sidecar then falling back to empty.
#[cfg(feature = "templates")]
fn read_frontmatter_for_html(
    html_path: &Path,
    site_dir: &Path,
    sidecar_dir: &Path,
) -> HashMap<String, serde_json::Value> {
    let rel = html_path.strip_prefix(site_dir).unwrap_or(html_path);
    let sidecar = sidecar_dir.join(rel).with_extension("meta.json");
    if sidecar.exists() {
        if let Ok(content) = fs::read_to_string(&sidecar) {
            if let Ok(meta) = serde_json::from_str(&content) {
                return meta;
            }
        }
    }
    HashMap::new()
}

/// Recursively collects `.html` files (delegates to `crate::walk`).
#[cfg(feature = "templates")]
fn collect_html_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files_bounded_depth(dir, "html", MAX_DIR_DEPTH)
}

/// Natively calculates top-3 related articles for each page based on overlapping tags/categories.
#[cfg(feature = "templates")]
fn enrich_with_related_posts(
    html_files: &[PathBuf],
    site_dir: &Path,
    sidecar_dir: &Path,
) -> HashMap<PathBuf, HashMap<String, serde_json::Value>> {
    let mut pages_meta = HashMap::new();

    for html_path in html_files {
        let fm = read_frontmatter_for_html(html_path, site_dir, sidecar_dir);
        let mut terms = std::collections::HashSet::new();

        if let Some(tags_val) = fm.get("tags") {
            if let Some(arr) = tags_val.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        let _ = terms.insert(s.to_string());
                    }
                }
            } else if let Some(s) = tags_val.as_str() {
                for term in ssg_core::split_terms(s) {
                    let _ = terms.insert(term);
                }
            }
        }
        if let Some(cats_val) = fm.get("categories") {
            if let Some(arr) = cats_val.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        let _ = terms.insert(s.to_string());
                    }
                }
            } else if let Some(s) = cats_val.as_str() {
                for term in ssg_core::split_terms(s) {
                    let _ = terms.insert(term);
                }
            }
        }

        let title = fm
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled")
            .to_string();
        let date = fm
            .get("date")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let rel = html_path.strip_prefix(site_dir).unwrap_or(html_path);
        let url = format!("/{}", rel.to_string_lossy().replace('\\', "/"));

        let _ =
            pages_meta.insert(html_path.clone(), (fm, title, terms, date, url));
    }

    let mut enriched = HashMap::new();
    for (html_path, (fm, _title, terms, _date, _url)) in &pages_meta {
        let mut candidates = Vec::new();

        for (
            other_path,
            (_other_fm, other_title, other_terms, other_date, other_url),
        ) in &pages_meta
        {
            if other_path == html_path {
                continue;
            }

            let overlap = terms.intersection(other_terms).count();
            if overlap > 0 {
                candidates.push((
                    overlap,
                    other_date.clone(),
                    other_title.clone(),
                    other_url.clone(),
                ));
            }
        }

        // Overlap desc, then date desc, then URL asc. The URL tiebreak is
        // what makes this a total order: without it, two posts sharing an
        // overlap count and a date kept whatever order the content walk
        // produced, and `.take(3)` below then selected a different three
        // depending on the filesystem. APFS and ext4 enumerate a
        // directory differently, so the same content produced different
        // "related posts" on macOS and Linux.
        //
        // `determinism.yml` could not catch this: it compares two builds
        // on one runner, and two builds on one filesystem agree. It
        // surfaced when the golden suite compared a macOS-seeded run
        // against Linux. URLs are unique per page, so this is total.
        candidates.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.3.cmp(&b.3))
        });

        let top_3: Vec<serde_json::Value> = candidates
            .into_iter()
            .take(3)
            .map(|(_overlap, other_date, other_title, other_url)| {
                let mut obj = serde_json::Map::new();
                let _ = obj.insert(
                    "title".to_string(),
                    serde_json::Value::String(other_title),
                );
                let _ = obj.insert(
                    "url".to_string(),
                    serde_json::Value::String(other_url),
                );
                let _ = obj.insert(
                    "date".to_string(),
                    serde_json::Value::String(other_date),
                );
                serde_json::Value::Object(obj)
            })
            .collect();

        let mut new_fm = fm.clone();
        let _ = new_fm.insert(
            "related_posts".to_string(),
            serde_json::Value::Array(top_3),
        );
        let _ = enriched.insert(html_path.clone(), new_fm);
    }

    enriched
}

#[cfg(all(test, feature = "templates"))]
mod tests {
    use super::*;
    use crate::cmd::SsgConfig;
    use crate::test_support::init_logger;
    use std::fs;
    use tempfile::{tempdir, TempDir};

    // -------------------------------------------------------------------
    // Test fixtures
    // -------------------------------------------------------------------

    fn layout() -> (TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
        init_logger();
        let dir = tempdir().expect("tempdir");
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        let templates = dir.path().join("templates/tera");
        for d in [&content, &build, &site, &templates] {
            fs::create_dir_all(d).expect("mkdir");
        }
        (dir, content, build, site, templates)
    }

    fn make_config(root: &Path) -> SsgConfig {
        SsgConfig {
            site_name: "Test".to_string(),
            site_title: "Test Site".to_string(),
            site_description: "Desc".to_string(),
            base_url: "http://localhost".to_string(),
            language: "en-GB".to_string(),
            content_dir: root.join("content"),
            output_dir: root.join("build"),
            template_dir: root.join("templates"),
            theme: None,
            serve_dir: None,
            i18n: None,
            cdn_prefix: None,
            og_image: None,
            image: crate::cmd::ImageConfig::default(),
            edge_headers: crate::cmd::EdgeHeadersConfig::default(),
            agents: None,
            transitions: false,
            security: crate::cmd::SecurityConfig::default(),
            no_taxonomy_pages: false,
        }
    }

    fn setup_project(dir: &Path) {
        let content = dir.join("content");
        let build = dir.join("build");
        let site = dir.join("site");
        let templates = dir.join("templates/tera");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&build).unwrap();
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&templates).unwrap();

        fs::write(
            templates.join("base.html"),
            r#"<!DOCTYPE html>
<html><head><title>{{ page.title | default("") }}</title></head>
<body>{% block content %}{% endblock %}</body></html>"#,
        )
        .unwrap();

        fs::write(
            templates.join("page.html"),
            r#"{% extends "base.html" %}
{% block content %}{{ page.content | safe }}{% endblock %}"#,
        )
        .unwrap();

        fs::write(
            content.join("index.md"),
            "---\ntitle: Home\nlayout: page\n---\n# Welcome\n",
        )
        .unwrap();

        fs::write(site.join("index.html"), "<h1>Welcome</h1>").unwrap();

        let meta_dir = build.join(".meta");
        fs::create_dir_all(&meta_dir).unwrap();
        fs::write(
            meta_dir.join("index.meta.json"),
            r#"{"title": "Home", "layout": "page"}"#,
        )
        .unwrap();
    }

    /// Regression: the `layout` field must still resolve after the
    /// build directory has been promoted onto `site_dir`.
    ///
    /// `before_compile` writes sidecars under `<build_dir>/.meta`, but
    /// `staticdatagen` renames `output.build-tmp` → `output` once the
    /// compile finishes, taking `.meta/` with it. Reading
    /// `<build_dir>/.meta` unconditionally therefore found nothing on
    /// every default build, so `layout` fell back to `page` for every
    /// page and a theme with `index`/`about`/`contact` layouts rendered
    /// through none of them.
    #[test]
    fn layout_resolves_when_build_dir_was_promoted_to_site_dir() {
        init_logger();
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        let templates = dir.path().join("templates/tera");
        for d in [&content, &build, &site, &templates] {
            fs::create_dir_all(d).unwrap();
        }

        // Two distinct templates so the assertion can tell which one ran.
        fs::write(templates.join("page.html"), "FALLBACK-PAGE").unwrap();
        fs::write(templates.join("custom.html"), "CUSTOM-LAYOUT").unwrap();
        fs::write(site.join("index.html"), "<h1>compiled</h1>").unwrap();

        // The promotion already happened: `.meta` lives beside the
        // compiled HTML in `site_dir`, and `build_dir/.meta` is absent.
        let meta_dir = site.join(".meta");
        fs::create_dir_all(&meta_dir).unwrap();
        fs::write(
            meta_dir.join("index.meta.json"),
            r#"{"title": "Home", "layout": "custom"}"#,
        )
        .unwrap();
        assert!(
            !build.join(".meta").exists(),
            "fixture must model the promoted layout"
        );

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: templates,
            ..Default::default()
        });
        let config = make_config(dir.path());
        let ctx = PluginContext::with_config(
            &content,
            &build,
            &site,
            &dir.path().join("templates"),
            config,
        );

        plugin.after_compile(&ctx).unwrap();

        let out = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(
            out.contains("CUSTOM-LAYOUT"),
            "expected the `custom` layout from the sidecar, got: {out}"
        );
        assert!(
            !out.contains("FALLBACK-PAGE"),
            "must not silently fall back to page.html: {out}"
        );
    }

    /// A layout with no template and no `page.html` fallback must be
    /// reported rather than counted as a successful render — the
    /// pass-through arm previously made a no-op pipeline look healthy.
    #[test]
    fn missing_template_leaves_output_untouched_and_is_not_counted() {
        init_logger();
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        let templates = dir.path().join("templates/tera");
        for d in [&content, &build, &site, &templates] {
            fs::create_dir_all(d).unwrap();
        }

        // A template dir that exists but holds no `page.html` and no
        // template matching the requested layout.
        fs::write(templates.join("other.html"), "OTHER").unwrap();
        fs::write(site.join("index.html"), "<h1>compiled</h1>").unwrap();
        let meta_dir = site.join(".meta");
        fs::create_dir_all(&meta_dir).unwrap();
        fs::write(meta_dir.join("index.meta.json"), r#"{"layout": "nope"}"#)
            .unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: templates,
            ..Default::default()
        });
        let config = make_config(dir.path());
        let ctx = PluginContext::with_config(
            &content,
            &build,
            &site,
            &dir.path().join("templates"),
            config,
        );

        plugin.after_compile(&ctx).unwrap();

        let out = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(
            out, "<h1>compiled</h1>",
            "compiled output must survive untouched"
        );
    }

    #[test]
    fn test_template_plugin_renders() {
        init_logger();
        let dir = tempdir().unwrap();
        setup_project(dir.path());

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("templates/tera"),
            ..Default::default()
        });

        let config = SsgConfig {
            site_name: "Test".to_string(),
            site_title: "Test Site".to_string(),
            site_description: "Desc".to_string(),
            base_url: "http://localhost".to_string(),
            language: "en-GB".to_string(),
            content_dir: dir.path().join("content"),
            output_dir: dir.path().join("build"),
            template_dir: dir.path().join("templates"),
            theme: None,
            serve_dir: None,
            i18n: None,
            cdn_prefix: None,
            og_image: None,
            image: crate::cmd::ImageConfig::default(),
            edge_headers: crate::cmd::EdgeHeadersConfig::default(),
            agents: None,
            transitions: false,
            security: crate::cmd::SecurityConfig::default(),
            no_taxonomy_pages: false,
        };

        let content_dir = config.content_dir.clone();
        let output_dir = config.output_dir.clone();
        let template_dir = config.template_dir.clone();
        let site = dir.path().join("site");
        let ctx = PluginContext::with_config(
            &content_dir,
            &output_dir,
            &site,
            &template_dir,
            config,
        );

        plugin.after_compile(&ctx).unwrap();

        let output =
            fs::read_to_string(dir.path().join("site/index.html")).unwrap();
        assert!(output.contains("<!DOCTYPE html>"));
        assert!(output.contains("Home"));
        assert!(output.contains("<h1>Welcome</h1>"));
    }

    #[test]
    fn test_template_plugin_skips_missing_templates() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("index.html"), "<p>hello</p>").unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("nonexistent"),
            ..Default::default()
        });

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());

        plugin.after_compile(&ctx).unwrap();

        let output = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(output, "<p>hello</p>");
    }

    #[test]
    fn name_returns_templates_identifier() {
        let plugin = TemplatePlugin::new(TemplateConfig::default());
        assert_eq!(plugin.name(), "templates");
    }

    #[test]
    fn template_plugin_debug_output_mentions_config() {
        // The derived `Debug` impl is otherwise never exercised (no
        // test formats the plugin with `{:?}`).
        let plugin = TemplatePlugin::new(TemplateConfig::default());
        let s = format!("{plugin:?}");
        assert!(s.contains("TemplatePlugin"), "{s}");
    }

    #[test]
    fn new_stores_supplied_config() {
        let cfg = TemplateConfig {
            template_dir: std::env::temp_dir().join("ssg_template_fake"),
            ..Default::default()
        };
        let plugin = TemplatePlugin::new(cfg.clone());
        assert_eq!(plugin.config.template_dir, cfg.template_dir);
    }

    #[test]
    fn from_template_dir_nests_under_tera_subdirectory() {
        let plugin =
            TemplatePlugin::from_template_dir(Path::new("/my/templates"));
        assert!(plugin.config.template_dir.ends_with("templates/tera"));
    }

    #[test]
    fn before_compile_emits_sidecars_from_content_markdown() {
        let (_tmp, content, build, _site, templates) = layout();
        fs::write(content.join("index.md"), "---\ntitle: Test\n---\nbody")
            .unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: templates,
            ..Default::default()
        });
        let ctx = PluginContext::new(&content, &build, &content, &content);

        plugin.before_compile(&ctx).unwrap();
        assert!(build.join(".meta").join("index.meta.json").exists());
    }

    #[test]
    fn before_compile_no_markdown_files_still_returns_ok() {
        let (_tmp, content, build, _site, templates) = layout();
        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: templates,
            ..Default::default()
        });
        let ctx = PluginContext::new(&content, &build, &content, &content);
        plugin.before_compile(&ctx).unwrap();
    }

    #[test]
    fn after_compile_without_config_uses_empty_site_globals() {
        let dir = tempdir().unwrap();
        setup_project(dir.path());

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("templates/tera"),
            ..Default::default()
        });
        let ctx = PluginContext::new(
            &dir.path().join("content"),
            &dir.path().join("build"),
            &dir.path().join("site"),
            &dir.path().join("templates"),
        );

        plugin.after_compile(&ctx).unwrap();
        let output =
            fs::read_to_string(dir.path().join("site").join("index.html"))
                .unwrap();
        assert!(output.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn after_compile_loads_data_files_into_context() {
        let dir = tempdir().unwrap();
        setup_project(dir.path());

        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("nav.toml"), r#"site = "demo""#).unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("templates/tera"),
            ..Default::default()
        });
        let config = make_config(dir.path());
        let ctx = PluginContext::with_config(
            &config.content_dir.clone(),
            &config.output_dir.clone(),
            &dir.path().join("site"),
            &config.template_dir.clone(),
            config,
        );

        plugin.after_compile(&ctx).unwrap();
        let output =
            fs::read_to_string(dir.path().join("site").join("index.html"))
                .unwrap();
        assert!(output.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn after_compile_unknown_layout_does_not_propagate_error() {
        let dir = tempdir().unwrap();
        setup_project(dir.path());

        let meta_dir = dir.path().join("build").join(".meta");
        fs::write(
            meta_dir.join("index.meta.json"),
            r#"{"title": "Home", "layout": "unknown_layout_999"}"#,
        )
        .unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("templates/tera"),
            ..Default::default()
        });
        let ctx = PluginContext::new(
            &dir.path().join("content"),
            &dir.path().join("build"),
            &dir.path().join("site"),
            &dir.path().join("templates"),
        );

        plugin
            .after_compile(&ctx)
            .expect("render failure must not propagate");
    }

    #[test]
    fn after_compile_default_layout_is_page_when_missing_field() {
        let dir = tempdir().unwrap();
        setup_project(dir.path());

        let meta_dir = dir.path().join("build").join(".meta");
        fs::write(meta_dir.join("index.meta.json"), r#"{"title": "Home"}"#)
            .unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("templates/tera"),
            ..Default::default()
        });
        let ctx = PluginContext::new(
            &dir.path().join("content"),
            &dir.path().join("build"),
            &dir.path().join("site"),
            &dir.path().join("templates"),
        );

        plugin.after_compile(&ctx).unwrap();
        let out =
            fs::read_to_string(dir.path().join("site").join("index.html"))
                .unwrap();
        assert!(out.contains("<!DOCTYPE html>"));
    }

    // -------------------------------------------------------------------
    // read_frontmatter_for_html — three branches
    // -------------------------------------------------------------------

    #[test]
    fn read_frontmatter_for_html_direct_sidecar_match() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let sidecars = dir.path().join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&sidecars).unwrap();

        let html = site.join("post.html");
        fs::write(&html, "").unwrap();
        fs::write(sidecars.join("post.meta.json"), r#"{"title": "Direct"}"#)
            .unwrap();

        let meta = read_frontmatter_for_html(&html, &site, &sidecars);
        assert_eq!(meta.get("title").and_then(|v| v.as_str()), Some("Direct"));
    }

    #[test]
    fn read_frontmatter_for_html_invalid_sidecar_returns_empty() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let sidecars = dir.path().join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&sidecars).unwrap();

        let html = site.join("post.html");
        fs::write(&html, "").unwrap();
        fs::write(sidecars.join("post.meta.json"), "{not valid").unwrap();

        let meta = read_frontmatter_for_html(&html, &site, &sidecars);
        assert!(meta.is_empty());
    }

    #[test]
    fn read_frontmatter_for_html_no_match_returns_empty_map() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let sidecars = dir.path().join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&sidecars).unwrap();

        let html = site.join("ghost.html");
        fs::write(&html, "").unwrap();

        let meta = read_frontmatter_for_html(&html, &site, &sidecars);
        assert!(meta.is_empty());
    }

    #[test]
    fn read_frontmatter_for_html_path_outside_site_dir_uses_full_path_as_rel() {
        // `html_path.strip_prefix(site_dir).unwrap_or(html_path)`: when
        // the page path isn't actually rooted under `site_dir` (e.g. a
        // symlinked or otherwise out-of-tree path), `strip_prefix` fails
        // and the fallback treats the whole path as `rel`. Every other
        // test in this file passes an in-tree path, so this branch was
        // otherwise never taken.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let elsewhere = dir.path().join("elsewhere");
        let sidecars = dir.path().join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::create_dir_all(&sidecars).unwrap();

        let html = elsewhere.join("post.html");
        fs::write(&html, "").unwrap();

        let meta = read_frontmatter_for_html(&html, &site, &sidecars);
        assert!(
            meta.is_empty(),
            "no sidecar exists for the full out-of-tree path"
        );
    }

    // -------------------------------------------------------------------
    // collect_html_files
    // -------------------------------------------------------------------

    #[test]
    fn collect_html_files_filters_non_html_extensions() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.html"), "").unwrap();
        fs::write(dir.path().join("b.css"), "").unwrap();
        fs::write(dir.path().join("c.js"), "").unwrap();

        let files = collect_html_files(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn collect_html_files_recurses_into_subdirectories() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("blog").join("2026");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("index.html"), "").unwrap();
        fs::write(nested.join("post.html"), "").unwrap();

        let files = collect_html_files(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn collect_html_files_returns_empty_for_missing_directory() {
        let dir = tempdir().unwrap();
        let result = collect_html_files(&dir.path().join("missing")).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_template_plugin_from_template_dir() {
        let p = TemplatePlugin::from_template_dir(Path::new("t"));
        assert_eq!(p.name(), "templates");
    }

    #[test]
    fn test_template_plugin_before_compile_error() {
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        fs::write(content_dir.join("index.md"), "---\ntitle: test\n---\n")
            .unwrap();

        let build_file = dir.path().join("build_file");
        fs::write(&build_file, "").unwrap();

        let ctx = PluginContext::new(
            &content_dir,
            &build_file,
            dir.path(),
            dir.path(),
        );
        let res =
            TemplatePlugin::new(TemplateConfig::default()).before_compile(&ctx);
        assert!(res.is_err());
    }

    #[test]
    #[cfg(unix)]
    fn test_template_plugin_after_compile_read_html_error() {
        let dir = tempdir().unwrap();
        let site_dir = dir.path().join("site");
        fs::create_dir_all(&site_dir).unwrap();
        let html_path = site_dir.join("test.html");
        fs::write(&html_path, "test").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&html_path, fs::Permissions::from_mode(0o000))
                .unwrap();
        }

        let templates = dir.path().join("tera");
        fs::create_dir_all(&templates).unwrap();
        fs::write(templates.join("base.html"), "").unwrap();

        let ctx =
            PluginContext::new(dir.path(), dir.path(), &site_dir, &templates);
        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: templates,
            ..Default::default()
        });
        let res = plugin.after_compile(&ctx);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(
                &html_path,
                fs::Permissions::from_mode(0o644),
            );
        }
        #[cfg(unix)]
        assert!(res.is_err());
    }

    #[test]
    fn test_enrich_with_related_posts_direct() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let sidecars = dir.path().join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&sidecars).unwrap();

        let html1 = site.join("p1.html");
        let html2 = site.join("p2.html");
        let html3 = site.join("p3.html");
        fs::write(&html1, "").unwrap();
        fs::write(&html2, "").unwrap();
        fs::write(&html3, "").unwrap();

        // p1: tags as array, categories as string
        fs::write(sidecars.join("p1.meta.json"), r#"{"title": "P1", "date": "2026-06-01", "tags": ["rust", "web"], "categories": "coding,tech"}"#).unwrap();
        // p2: tags as string (with overlapping "rust"), categories as array
        fs::write(sidecars.join("p2.meta.json"), r#"{"title": "P2", "date": "2026-06-02", "tags": "rust,systems", "categories": ["coding"]}"#).unwrap();
        // p3: no overlap
        fs::write(
            sidecars.join("p3.meta.json"),
            r#"{"title": "P3", "date": "2026-06-03", "tags": ["other"]}"#,
        )
        .unwrap();

        let files = vec![html1.clone(), html2.clone(), html3.clone()];
        let enriched = enrich_with_related_posts(&files, &site, &sidecars);

        let p1_fm = enriched.get(&html1).unwrap();
        let related_p1 =
            p1_fm.get("related_posts").unwrap().as_array().unwrap();
        assert_eq!(related_p1.len(), 1);
        assert_eq!(related_p1[0]["title"], "P2");
        assert_eq!(related_p1[0]["url"], "/p2.html");
        assert_eq!(related_p1[0]["date"], "2026-06-02");

        let p3_fm = enriched.get(&html3).unwrap();
        let related_p3 =
            p3_fm.get("related_posts").unwrap().as_array().unwrap();
        assert!(related_p3.is_empty());
    }

    #[test]
    fn enrich_with_related_posts_path_outside_site_dir_uses_full_path_as_rel() {
        // Same `strip_prefix(site_dir).unwrap_or(html_path)` fallback as
        // `read_frontmatter_for_html`, but in `enrich_with_related_posts`'s
        // own URL-building code — a second, independent occurrence of
        // the same pattern that every other enrich test bypasses by
        // always passing in-tree paths.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let elsewhere = dir.path().join("elsewhere");
        let sidecars = dir.path().join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::create_dir_all(&sidecars).unwrap();

        let html = elsewhere.join("post.html");
        fs::write(&html, "").unwrap();

        let files = vec![html.clone()];
        let enriched = enrich_with_related_posts(&files, &site, &sidecars);
        let fm = enriched.get(&html).unwrap();
        // Solo page, no overlap possible — just confirm it didn't panic
        // building the URL from the un-strippable full path.
        assert!(fm
            .get("related_posts")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn after_compile_render_failure_logs_and_leaves_file_untouched() {
        // A template that parses fine but fails at render time (missing
        // include) must hit the warn branch without propagating.
        init_logger();
        let dir = tempdir().unwrap();
        setup_project(dir.path());
        fs::write(
            dir.path().join("templates/tera/page.html"),
            r#"{% include "missing-partial.html" %}"#,
        )
        .unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("templates/tera"),
            ..Default::default()
        });
        let ctx = PluginContext::new(
            &dir.path().join("content"),
            &dir.path().join("build"),
            &dir.path().join("site"),
            &dir.path().join("templates"),
        );

        plugin
            .after_compile(&ctx)
            .expect("render failure must not propagate");
        let out =
            fs::read_to_string(dir.path().join("site").join("index.html"))
                .unwrap();
        assert_eq!(out, "<h1>Welcome</h1>", "failed render must not rewrite");
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_write_failure_propagates() {
        use std::os::unix::fs::PermissionsExt;

        // Read + render succeed; writing the rendered output back to a
        // read-only file must surface as Err.
        let dir = tempdir().unwrap();
        setup_project(dir.path());
        let html = dir.path().join("site").join("index.html");
        fs::set_permissions(&html, fs::Permissions::from_mode(0o444)).unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("templates/tera"),
            ..Default::default()
        });
        let ctx = PluginContext::new(
            &dir.path().join("content"),
            &dir.path().join("build"),
            &dir.path().join("site"),
            &dir.path().join("templates"),
        );

        let result = plugin.after_compile(&ctx);
        let _ = fs::set_permissions(&html, fs::Permissions::from_mode(0o644));
        assert!(result.is_err(), "read-only output must surface as Err");
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_walk_error_from_unreadable_site_subdir() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        setup_project(dir.path());
        let locked = dir.path().join("site").join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let plugin = TemplatePlugin::new(TemplateConfig {
            template_dir: dir.path().join("templates/tera"),
            ..Default::default()
        });
        let ctx = PluginContext::new(
            &dir.path().join("content"),
            &dir.path().join("build"),
            &dir.path().join("site"),
            &dir.path().join("templates"),
        );

        let result = plugin.after_compile(&ctx);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(result.is_err(), "unreadable site subdir must be an Err");
    }

    #[test]
    #[cfg(unix)]
    fn read_frontmatter_for_html_unreadable_sidecar_returns_empty() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let sidecars = dir.path().join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&sidecars).unwrap();

        let html = site.join("post.html");
        fs::write(&html, "").unwrap();
        let sidecar = sidecars.join("post.meta.json");
        fs::write(&sidecar, r#"{"title": "Hidden"}"#).unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o000))
            .unwrap();

        let meta = read_frontmatter_for_html(&html, &site, &sidecars);
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(meta.is_empty(), "unreadable sidecar must yield empty map");
    }

    #[test]
    fn test_enrich_related_posts_mixed_shapes_and_tie_break() {
        // Exercises the non-string array items, empty comma segments,
        // non-array/non-string tag+category values, and the sort
        // comparator (including the equal-overlap date tie-break).
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let sidecars = dir.path().join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&sidecars).unwrap();

        let a = site.join("a.html");
        let b = site.join("b.html");
        let c = site.join("c.html");
        let d = site.join("d.html");
        for f in [&a, &b, &c, &d] {
            fs::write(f, "").unwrap();
        }

        // a: non-string tag item ignored; categories is neither array
        //    nor string (number) and contributes nothing.
        fs::write(
            sidecars.join("a.meta.json"),
            r#"{"title": "A", "date": "2026-01-01", "tags": ["shared", 42], "categories": 7}"#,
        )
        .unwrap();
        // b: tags as string with empty segments (skipped).
        fs::write(
            sidecars.join("b.meta.json"),
            r#"{"title": "B", "date": "2026-01-02", "tags": "shared,,"}"#,
        )
        .unwrap();
        // c: categories as string with leading/trailing empty segments.
        fs::write(
            sidecars.join("c.meta.json"),
            r#"{"title": "C", "date": "2026-01-03", "tags": ["shared"], "categories": ",misc,"}"#,
        )
        .unwrap();
        // d: tags is neither array nor string; categories array with a
        //    non-string item.
        fs::write(
            sidecars.join("d.meta.json"),
            r#"{"title": "D", "date": "2026-01-04", "tags": 99, "categories": ["misc", 5]}"#,
        )
        .unwrap();

        let files = vec![a.clone(), b, c, d.clone()];
        let enriched = enrich_with_related_posts(&files, &site, &sidecars);

        // a shares "shared" with b and c equally; the tie-break sorts
        // the newer date (C) first.
        let a_rel = enriched
            .get(&a)
            .unwrap()
            .get("related_posts")
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(a_rel.len(), 2);
        assert_eq!(a_rel[0]["title"], "C");
        assert_eq!(a_rel[1]["title"], "B");

        // d only shares "misc" with c.
        let d_rel = enriched
            .get(&d)
            .unwrap()
            .get("related_posts")
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(d_rel.len(), 1);
        assert_eq!(d_rel[0]["title"], "C");
    }
}
