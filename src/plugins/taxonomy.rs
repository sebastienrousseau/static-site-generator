// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Taxonomy generation plugin.
//!
//! Reads `tags` and `categories` from frontmatter sidecars and
//! generates index pages for each taxonomy term, rendered through
//! the same template engine (`MiniJinja`) that drives normal page
//! rendering. Built-in fallback templates extend `base.html` so the
//! pages share the site's layout, CSS, nav, and footer (#542).
//!
//! ## Per-term landing pages (issue #586, port 5 of 5)
//!
//! Besides the `/tags/index.html` hub, every term gets its own
//! landing page (`/tags/<slug>/index.html`, and likewise for
//! categories and topics) listing its member posts. Terms may be
//! declared either as frontmatter arrays (`tags: [a, b]`) or as the
//! comma-separated string form the bundled examples use
//! (`tags: "a, b, c"`). Slugs come from [`ssg_core::slugify`];
//! term ordering is case-insensitive alphabetical, so output is
//! deterministic across rebuilds.
//!
//! ## Lifecycle caveat — the `transform_html` bypass
//!
//! This plugin writes pages in `after_compile`, but the pipeline
//! snapshots the HTML file list *before* `after_compile` runs
//! (`pipeline.rs`: `cache_html_files()` precedes
//! `run_after_compile()`), so the fused `transform_html` pass never
//! sees taxonomy pages — canonical/JSON-LD/a11y transform plugins
//! skip them (the ROADMAP-documented plugin-lifecycle-phase trap;
//! see #586). Mitigation: the built-in templates (and the
//! non-`templates` fallback renderer) inline the essential head
//! elements themselves — `<!DOCTYPE html>`, `<html lang>`,
//! `<meta charset>`, `<title>`, a `<link rel="canonical">` derived
//! from `site.base_url` + the term's directory URL, plus the SEO
//! meta the audit gates probe (`description`, `og:title`,
//! `og:type`, `og:description`, `og:url`, `twitter:card`). Pages
//! generated here therefore do not depend on the transform chain
//! for correctness; richer per-page structured data (JSON-LD,
//! og:image) stays with the full taxonomy engine planned for
//! 0.0.48 (#587).
//!
//! Two further real-pipeline behaviours: sidecars are read from
//! `<build>/.meta/` with a fallback to the staged `<site>/.meta/`
//! copy, and author-authored pages (e.g. a hand-written
//! `/tags/index.html` compiled from `tags.md` — anything without
//! the `ssg-taxonomy` generator marker) are never overwritten.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::plugins_group::topic_clusters::{TopicCluster, TopicClusters};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

/// One page as a taxonomy lists it.
///
/// This was a `(title, url)` pair, which is all a bulleted list needs. A
/// card needs more — a date to sort and show, a description to read, an
/// image to look at — and none of it can be recovered at render time,
/// because the sidecar that held it has already been walked past. The
/// fields beyond `title` and `url` are all optional: a page that
/// declares none still lists, exactly as it did before.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, Default)]
pub struct PageRef {
    /// Display title.
    pub title: String,
    /// Resolved, prefix-aware URL.
    pub url: String,
    /// Front-matter `description`, for the card body and JSON-LD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Front-matter `date`, as authored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Front-matter `banner`, the card's image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
}

impl PageRef {
    /// A page with nothing but a title and a URL — what every taxonomy
    /// carried before cards existed.
    ///
    /// The collector fills every field from the sidecar; this is for
    /// callers building a [`TaxonomyTerm`] by hand, where the card fields
    /// are not available and are not wanted.
    #[must_use]
    pub fn new(title: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            ..Self::default()
        }
    }
}

/// A mapping from taxonomy term to the pages carrying it.
type TaxonomyMap = HashMap<String, Vec<PageRef>>;

/// One taxonomy term after slug merging: display spelling, URL slug and the
/// member pages listed on it.
type MergedTerm = (String, String, Vec<PageRef>);

/// A taxonomy term with its associated pages.
#[derive(Debug, Clone)]
pub struct TaxonomyTerm {
    /// The term name (e.g. "rust", "web").
    pub name: String,
    /// The URL slug (e.g. "rust", "web").
    pub slug: String,
    /// Pages carrying this term.
    pub pages: Vec<PageRef>,
}

// =====================================================================
// Built-in templates (embedded so the binary works without scaffold)
//
// The constants below are only loaded by the MiniJinja loader inside
// `cfg(feature = "templates")` (line ~174). The non-templates fallback
// impl renders without referencing them, so we gate each constant
// behind the feature so `cargo check --no-default-features` does not
// trip on `dead_code = "deny"` (see workspace [lints.rust]).
// =====================================================================

/// Built-in tag term-page template (#542).
#[cfg(feature = "templates")]
const BUILTIN_TAG_HTML: &str = include_str!("builtin_templates/tag.html");
/// Built-in category term-page template (#542).
#[cfg(feature = "templates")]
const BUILTIN_CATEGORY_HTML: &str =
    include_str!("builtin_templates/category.html");
/// Built-in archive/topic term-page template (#542).
#[cfg(feature = "templates")]
const BUILTIN_ARCHIVE_HTML: &str =
    include_str!("builtin_templates/archive.html");
/// Built-in taxonomy index-page template (lists all terms) (#542).
#[cfg(feature = "templates")]
const BUILTIN_TAXONOMY_INDEX_HTML: &str =
    include_str!("builtin_templates/taxonomy_index.html");
/// Built-in minimal `base.html` for sites that ship none of their own.
/// User-provided `base.html` is preferred via the path loader (#542).
#[cfg(feature = "templates")]
const BUILTIN_BASE_HTML: &str = include_str!("builtin_templates/base.html");

/// Plugin that generates taxonomy index pages for tags and categories.
///
/// Runs in `after_compile`. Reads `.meta.json` sidecars to find
/// `tags` and `categories` arrays, then generates:
/// - `/tags/index.html` — list of all tags with page counts
/// - `/tags/{slug}/index.html` — list of pages for each tag
/// - `/categories/index.html` and `/categories/{slug}/index.html`
/// - `/topics/index.html` and `/topics/{slug}/index.html`
///
/// All pages render through the site's `MiniJinja` template engine so
/// they share the site's `base.html`, CSS, nav, footer, and lang
/// attribute (issue #542).
#[derive(Debug, Clone, Copy)]
pub struct TaxonomyPlugin;

impl Plugin for TaxonomyPlugin {
    fn name(&self) -> &'static str {
        "taxonomy"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        // Opt-out for sites that own their own taxonomy (`--no-tag-pages`
        // / `SSG_NO_TAG_PAGES`). Checked before any work so the build does
        // not pay for output it is about to discard. Absent ⇒ generate, so
        // this cannot change an existing build.
        if ctx.config.as_ref().is_some_and(|c| c.no_taxonomy_pages) {
            return Ok(());
        }

        // Sidecar roots in priority order: `<build>/.meta/` (the
        // emit_sidecars convention) and `<site>/.meta/` — the staged
        // copy the real pipeline leaves in the output directory
        // (staticdatagen layout, what the audited demo site has once
        // the build staging dir is cleaned). (#586 port 5)
        let sidecar_dir = {
            let build_meta = ctx.build_dir.join(".meta");
            if build_meta.exists() {
                build_meta
            } else {
                ctx.site_dir.join(".meta")
            }
        };
        if !sidecar_dir.exists() {
            return Ok(());
        }

        let url_prefix = ctx.config.as_ref().map_or_else(String::new, |c| {
            crate::plugins_group::csp::base_url_path_prefix(&c.base_url)
        });
        let (tags, categories, topics) =
            collect_taxonomy_entries(&sidecar_dir, &ctx.site_dir, &url_prefix)?;

        // Lazily build the template engine once per run; reused across
        // tags, categories, and topics.
        let renderer = TaxonomyRenderer::new(ctx);

        // Curated pillar-page metadata for the `topics` taxonomy (#587).
        // Empty unless the site ships `_data/topics.toml`, and a site that
        // does not builds exactly as before.
        let clusters =
            crate::plugins_group::topic_clusters::load(&ctx.content_dir);
        if !clusters.is_empty() {
            let known: Vec<String> = topics.keys().cloned().collect();
            crate::plugins_group::topic_clusters::warn_unknown(
                &clusters, &known,
            );
        }

        // A multi-locale site gets one taxonomy tree per locale. Without
        // this every locale's pages share a single index, so an
        // English-language tag page lists French pages beside English ones
        // and a French reader has no tag index at all.
        let (locales, default_locale) = ctx.config.as_ref().map_or_else(
            || (Vec::new(), String::new()),
            |c| {
                c.i18n.as_ref().map_or_else(
                    || (Vec::new(), String::new()),
                    |i| (i.locales.clone(), i.default_locale.clone()),
                )
            },
        );
        let multi_locale = locales.len() > 1;

        for (name, title, map, kind) in [
            ("tags", "Tags", &tags, TaxonomyKind::Tag),
            (
                "categories",
                "Categories",
                &categories,
                TaxonomyKind::Category,
            ),
            ("topics", "Topics", &topics, TaxonomyKind::Archive),
        ] {
            if map.is_empty() {
                continue;
            }
            if multi_locale {
                let by_locale = split_map_by_locale(
                    map,
                    &locales,
                    &default_locale,
                    &url_prefix,
                );
                for (locale, scoped) in &by_locale {
                    // The default locale keeps the root path so existing
                    // links and sitemaps do not move.
                    let dir = if *locale == default_locale {
                        ctx.site_dir.join(name)
                    } else {
                        ctx.site_dir.join(locale).join(name)
                    };
                    // The default locale keeps the bare prefix; others
                    // get their own, so Home and term links stay inside
                    // the locale a reader is already in.
                    let is_default = *locale == default_locale;
                    let locale_prefix = if is_default {
                        url_prefix.clone()
                    } else {
                        format!("{url_prefix}/{locale}")
                    };
                    let segment = if is_default {
                        String::new()
                    } else {
                        format!("/{locale}")
                    };
                    let scoped_renderer = renderer.for_locale(
                        if is_default {
                            None
                        } else {
                            Some(locale.as_str())
                        },
                        &locale_prefix,
                        &segment,
                    );
                    generate_taxonomy_pages_at(
                        &dir,
                        name,
                        title,
                        scoped,
                        kind,
                        &scoped_renderer,
                        topic_clusters_for(name, &clusters),
                    )?;
                    log::info!(
                        "[taxonomy] Generated {} {name} page(s) for {locale}",
                        scoped.len()
                    );
                }
            } else {
                generate_taxonomy_pages(
                    &ctx.site_dir,
                    name,
                    title,
                    map,
                    kind,
                    &renderer,
                    topic_clusters_for(name, &clusters),
                )?;
                log::info!("[taxonomy] Generated {} {name} page(s)", map.len());
            }
        }

        Ok(())
    }
}

/// Which built-in template family to use for a taxonomy.
#[derive(Debug, Clone, Copy)]
enum TaxonomyKind {
    Tag,
    Category,
    Archive,
}

// These helpers only feed the MiniJinja-driven renderer; the
// non-templates fallback emits literal HTML and never asks for the
// template filename or term variable. Gated to suppress dead_code
// under `--no-default-features`.
#[cfg(feature = "templates")]
impl TaxonomyKind {
    /// User-overridable template filename (looked up in the user's
    /// `templates/tera/` directory first).
    const fn template_name(self) -> &'static str {
        match self {
            Self::Tag => "tag.html",
            Self::Category => "category.html",
            Self::Archive => "archive.html",
        }
    }

    /// Variable name the template uses to address the current term
    /// (`tag`, `category`, `term`).
    const fn term_var(self) -> &'static str {
        match self {
            Self::Tag => "tag",
            Self::Category => "category",
            Self::Archive => "term",
        }
    }
}

// =====================================================================
// Template engine integration
// =====================================================================

/// Encapsulates `MiniJinja` rendering for taxonomy pages.
///
/// Holds a `MiniJinja` environment whose loader prefers the user's
/// `templates/tera/` files and falls back to embedded built-in sources
/// so pages always extend a real `base.html` (issue #542).
struct TaxonomyRenderer<'a> {
    #[cfg(feature = "templates")]
    env: minijinja::Environment<'static>,
    ctx: &'a PluginContext,
    /// Locale of the tree currently being rendered, as
    /// `(language, url_prefix, path_segment)`. `None` on a single-locale
    /// site.
    ///
    /// `language` is `None` for the default locale, which keeps the
    /// site's configured tag — overriding it with the bare locale code
    /// turned `en-GB` into `en`. `path_segment` is `""` for the default
    /// locale and `/fr` for others, and prefixes `page_url` so the
    /// canonical points at the page that was actually written.
    ///
    /// Without any of this every tree inherited the site's default
    /// language, so the French tag index declared `lang="en-GB"` and
    /// linked Home to the English home page.
    locale: Option<(Option<String>, String, String)>,
}

#[cfg(feature = "templates")]
impl<'a> TaxonomyRenderer<'a> {
    fn new(ctx: &'a PluginContext) -> Self {
        let user_dir = resolve_user_template_dir(ctx);

        let mut env = minijinja::Environment::new();
        env.set_loader(
            move |name| -> Result<Option<String>, minijinja::Error> {
                // 1) Try the user's templates/tera/<name> if present.
                if let Some(dir) = user_dir.as_ref() {
                    let candidate = dir.join(name);
                    match fs::read_to_string(&candidate) {
                        Ok(s) => return Ok(Some(s)),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(minijinja::Error::new(
                                minijinja::ErrorKind::InvalidOperation,
                                format!(
                                    "failed to read user template {}: {e}",
                                    candidate.display()
                                ),
                            ))
                        }
                    }
                }
                // 2) Embedded fallbacks for the templates this plugin owns.
                Ok(match name {
                    "base.html" => Some(BUILTIN_BASE_HTML.to_string()),
                    "tag.html" => Some(BUILTIN_TAG_HTML.to_string()),
                    "category.html" => Some(BUILTIN_CATEGORY_HTML.to_string()),
                    "archive.html" => Some(BUILTIN_ARCHIVE_HTML.to_string()),
                    "taxonomy_index.html" => {
                        Some(BUILTIN_TAXONOMY_INDEX_HTML.to_string())
                    }
                    _ => None,
                })
            },
        );

        Self {
            env,
            ctx,
            locale: None,
        }
    }

    /// Returns a renderer scoped to one locale, so its pages declare that
    /// language and link within that locale's URL prefix.
    fn for_locale(
        &self,
        language: Option<&str>,
        url_prefix: &str,
        path_segment: &str,
    ) -> Self {
        Self {
            env: self.env.clone(),
            ctx: self.ctx,
            locale: Some((
                language.map(str::to_string),
                url_prefix.to_string(),
                path_segment.to_string(),
            )),
        }
    }

    /// Renders a term page (e.g. `/tags/rust/index.html`).
    fn render_term_page(
        &self,
        kind: TaxonomyKind,
        taxonomy_name: &str,
        taxonomy_title: &str,
        term: &str,
        slug: &str,
        pages: &[PageRef],
        cluster: Option<&TopicCluster>,
    ) -> Result<String, SsgError> {
        let tmpl =
            self.env.get_template(kind.template_name()).map_err(|e| {
                SsgError::Io {
                    path: PathBuf::from(kind.template_name()),
                    source: std::io::Error::other(e.to_string()),
                }
            })?;

        let mut ctx_map = self.base_context();
        let _ = ctx_map.insert(
            kind.term_var().to_string(),
            serde_json::Value::String(term.to_string()),
        );
        // Always expose `term` as well so generic templates can use it.
        let _ = ctx_map.insert(
            "term".to_string(),
            serde_json::Value::String(term.to_string()),
        );
        let _ = ctx_map.insert(
            "slug".to_string(),
            serde_json::Value::String(slug.to_string()),
        );
        // Structured data, for topics only. Tags and categories are
        // keyword indexes; a topic page is a curated collection, which is
        // what `CollectionPage` and `ItemList` exist to describe.
        if taxonomy_name == "topics" {
            let base_url =
                self.ctx.config.as_ref().map_or("", |c| c.base_url.as_str());
            let page_url = format!(
                "{}/{taxonomy_name}/{slug}/",
                self.locale_path_segment()
            );
            let jsonld = topic_jsonld(
                base_url,
                taxonomy_title,
                term,
                &page_url,
                cluster.and_then(|c| c.lede.as_deref()),
                pages,
            );
            let _ = ctx_map.insert(
                "jsonld".to_string(),
                serde_json::Value::String(jsonld.to_string()),
            );
        }

        // Curated pillar-page copy (#587). Both are absent unless
        // `_data/topics.toml` describes this term, and a template that
        // does not mention them renders exactly as it did before.
        if let Some(lede) = cluster.and_then(|c| c.lede.as_deref()) {
            let _ = ctx_map.insert(
                "lede".to_string(),
                serde_json::Value::String(lede.to_string()),
            );
        }
        if let Some(banner) = cluster.and_then(|c| c.banner.as_deref()) {
            let _ = ctx_map.insert(
                "banner".to_string(),
                serde_json::Value::String(banner.to_string()),
            );
        }
        let _ = ctx_map.insert(
            "taxonomy_name".to_string(),
            serde_json::Value::String(taxonomy_name.to_string()),
        );
        let _ = ctx_map.insert(
            "taxonomy_title".to_string(),
            serde_json::Value::String(taxonomy_title.to_string()),
        );
        let _ = ctx_map.insert(
            "page_url".to_string(),
            serde_json::Value::String(format!(
                "{}/{taxonomy_name}/{slug}/",
                self.locale_path_segment()
            )),
        );
        let _ = ctx_map.insert(
            "posts".to_string(),
            serde_json::Value::Array(pages_to_json(pages)),
        );
        // Essential head metadata — these pages bypass the transform
        // chain, so the SEO plugins never decorate them (#586 port 5).
        let _ = ctx_map.insert(
            "page_title".to_string(),
            serde_json::Value::String(format!("{taxonomy_title}: {term}")),
        );
        let _ = ctx_map.insert(
            "page_description".to_string(),
            serde_json::Value::String(format!(
                "{} page(s) under {taxonomy_title}: {term}.",
                pages.len()
            )),
        );

        tmpl.render(serde_json::Value::Object(ctx_map))
            .map(|mut s| {
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s
            })
            .map_err(|e| SsgError::Io {
                path: PathBuf::from(kind.template_name()),
                source: std::io::Error::other(e.to_string()),
            })
    }

    /// Renders the taxonomy index page (lists all terms).
    fn render_index_page(
        &self,
        taxonomy_name: &str,
        taxonomy_title: &str,
        sorted_terms: &[(&String, &Vec<PageRef>)],
        clusters: Option<&TopicClusters>,
    ) -> Result<String, SsgError> {
        let tmpl =
            self.env.get_template("taxonomy_index.html").map_err(|e| {
                SsgError::Io {
                    path: PathBuf::from("taxonomy_index.html"),
                    source: std::io::Error::other(e.to_string()),
                }
            })?;

        let mut ctx_map = self.base_context();
        let _ = ctx_map.insert(
            "taxonomy_name".to_string(),
            serde_json::Value::String(taxonomy_name.to_string()),
        );
        let _ = ctx_map.insert(
            "taxonomy_title".to_string(),
            serde_json::Value::String(taxonomy_title.to_string()),
        );
        let _ = ctx_map.insert(
            "page_url".to_string(),
            serde_json::Value::String(format!(
                "{}/{taxonomy_name}/",
                self.locale_path_segment()
            )),
        );
        // Essential head metadata (see render_term_page).
        let _ = ctx_map.insert(
            "page_title".to_string(),
            serde_json::Value::String(taxonomy_title.to_string()),
        );
        let _ = ctx_map.insert(
            "page_description".to_string(),
            serde_json::Value::String(format!(
                "All {} term(s): browse pages by {taxonomy_title}.",
                sorted_terms.len()
            )),
        );

        let term_entries: Vec<serde_json::Value> = sorted_terms
            .iter()
            .map(|(term, pages)| {
                let mut obj = serde_json::Map::new();
                let _ = obj.insert(
                    "name".to_string(),
                    serde_json::Value::String((*term).clone()),
                );
                let _ = obj.insert(
                    "slug".to_string(),
                    serde_json::Value::String(slugify(term)),
                );
                let _ = obj.insert(
                    "count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(
                        pages.len(),
                    )),
                );
                // Curated copy for the hub card, when the topic has any
                // (#587). A hub of bare slugs tells a reader nothing about
                // which topic is worth opening.
                let cluster =
                    clusters.and_then(|c| c.get(slugify(term).as_str()));
                for (key, value) in [
                    ("title", cluster.and_then(|c| c.title.as_deref())),
                    ("lede", cluster.and_then(|c| c.lede.as_deref())),
                    ("banner", cluster.and_then(|c| c.banner.as_deref())),
                ] {
                    if let Some(value) = value {
                        let _ = obj.insert(
                            key.to_string(),
                            serde_json::Value::String(value.to_string()),
                        );
                    }
                }
                serde_json::Value::Object(obj)
            })
            .collect();
        let _ = ctx_map.insert(
            "terms".to_string(),
            serde_json::Value::Array(term_entries),
        );

        tmpl.render(serde_json::Value::Object(ctx_map))
            .map(|mut s| {
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s
            })
            .map_err(|e| SsgError::Io {
                path: PathBuf::from("taxonomy_index.html"),
                source: std::io::Error::other(e.to_string()),
            })
    }

    /// The locale's URL segment (`""` for the default locale, `/fr`
    /// otherwise), used to prefix `page_url`.
    fn locale_path_segment(&self) -> String {
        self.locale
            .as_ref()
            .map_or_else(String::new, |(_, _, seg)| seg.clone())
    }

    /// Builds the `{ site: { name, title, language, ... } }` context
    /// shared by every taxonomy page.
    fn base_context(&self) -> serde_json::Map<String, serde_json::Value> {
        // Same sub-path prefix the term-page URLs use, so the index links
        // to pages that actually exist on a project-site deployment.
        let url_prefix =
            self.ctx.config.as_ref().map_or_else(String::new, |c| {
                crate::plugins_group::csp::base_url_path_prefix(&c.base_url)
            });
        let mut site = serde_json::Map::new();
        if let Some(cfg) = self.ctx.config.as_ref() {
            let _ = site.insert(
                "name".to_string(),
                serde_json::Value::String(cfg.site_name.clone()),
            );
            let _ = site.insert(
                "title".to_string(),
                serde_json::Value::String(cfg.site_title.clone()),
            );
            let _ = site.insert(
                "description".to_string(),
                serde_json::Value::String(cfg.site_description.clone()),
            );
            let _ = site.insert(
                "base_url".to_string(),
                serde_json::Value::String(cfg.base_url.clone()),
            );
            let _ = site.insert(
                "language".to_string(),
                serde_json::Value::String(cfg.language.clone()),
            );
            // Site-wide og:image fallback for pages with no image of
            // their own (#587 precursor — see SsgConfig::og_image doc).
            if let Some(og_image) = cfg.og_image.as_ref() {
                let _ = site.insert(
                    "og_image".to_string(),
                    serde_json::Value::String(og_image.clone()),
                );
            }
        } else {
            // Sensible defaults when the plugin runs without a config
            // (tests, ad-hoc invocations).
            let _ = site.insert(
                "language".to_string(),
                serde_json::Value::String("en".to_string()),
            );
        }

        let site_prefix = url_prefix.clone();
        let mut url_prefix = url_prefix;
        if let Some((language, locale_prefix, _)) = self.locale.as_ref() {
            if let Some(language) = language {
                let _ = site.insert(
                    "language".to_string(),
                    serde_json::Value::String(language.clone()),
                );
            }
            url_prefix.clone_from(locale_prefix);
        }

        let mut ctx_map = serde_json::Map::new();
        let _ =
            ctx_map.insert("site".to_string(), serde_json::Value::Object(site));
        // `url_prefix` is locale-scoped, for links that should keep a
        // reader inside their locale. Assets are not per-locale, so a
        // template linking a stylesheet needs the unscoped prefix too —
        // otherwise a French page asks for /atlas/fr/styles.css, which
        // does not exist.
        let _ =
            ctx_map.insert("url_prefix".to_string(), url_prefix.clone().into());
        let _ = ctx_map
            .insert("site_prefix".to_string(), site_prefix.clone().into());
        ctx_map
    }
}

/// Fallback shim so the module still compiles when the `templates`
/// feature is disabled. The MiniJinja crate is gated on that feature
/// in `Cargo.toml`; the shim falls back to a minimal escaped HTML
/// renderer that still respects `site.language` and per-page metadata.
#[cfg(not(feature = "templates"))]
impl<'a> TaxonomyRenderer<'a> {
    fn new(ctx: &'a PluginContext) -> Self {
        Self { ctx, locale: None }
    }

    /// Locale-scoped renderer; see the `templates` implementation.
    fn for_locale(
        &self,
        language: Option<&str>,
        url_prefix: &str,
        path_segment: &str,
    ) -> Self {
        Self {
            ctx: self.ctx,
            locale: Some((
                language.map(str::to_string),
                url_prefix.to_string(),
                path_segment.to_string(),
            )),
        }
    }

    /// ` — <site title>` when one is configured, else empty.
    ///
    /// Mirrors `{% if site.title %} — {{ site.title }}{% endif %}` in the
    /// bundled templates. Without this the no-templates build silently
    /// dropped a configured title, so the same config produced differently
    /// branded pages depending on which features the binary was built with.
    fn site_title_suffix(&self) -> String {
        self.ctx.config.as_ref().map_or_else(String::new, |cfg| {
            if cfg.site_title.is_empty() {
                String::new()
            } else {
                format!(" \u{2014} {}", cfg.site_title)
            }
        })
    }

    fn render_term_page(
        &self,
        _kind: TaxonomyKind,
        taxonomy_name: &str,
        taxonomy_title: &str,
        term: &str,
        slug: &str,
        pages: &[PageRef],
        _cluster: Option<&TopicCluster>,
    ) -> Result<String, SsgError> {
        let lang = self.lang();
        let canonical = self.canonical(&format!("/{taxonomy_name}/{slug}/"));
        let og_image = self.og_image_tag();
        let description =
            format!("{} page(s) under {taxonomy_title}: {term}.", pages.len());
        let suffix = self.site_title_suffix();
        let mut out = format!(
            "<!DOCTYPE html>\n<html lang=\"{lang}\">\n<head>\
             <meta charset=\"utf-8\">{canonical}\
             <meta name=\"generator\" content=\"ssg-taxonomy\">\
             <meta name=\"description\" content=\"{description}\">\
             <meta property=\"og:title\" content=\"{taxonomy_title}: {term}\">\
             <meta property=\"og:type\" content=\"website\">\
             {og_image}\
             <meta name=\"twitter:card\" content=\"summary\">\
             <title>{taxonomy_title}: {term}{suffix}</title></head>\n\
             <body>\n<main>\n<h1>{taxonomy_title}: {term}</h1>\n<ul>\n"
        );
        for (title, url) in pages {
            out.push_str(&format!("<li><a href=\"{url}\">{title}</a></li>\n"));
        }
        out.push_str("</ul>\n</main>\n</body>\n</html>\n");
        Ok(out)
    }

    fn render_index_page(
        &self,
        taxonomy_name: &str,
        taxonomy_title: &str,
        sorted_terms: &[(&String, &Vec<PageRef>)],
        clusters: Option<&TopicClusters>,
    ) -> Result<String, SsgError> {
        let lang = self.lang();
        let canonical = self.canonical(&format!("/{taxonomy_name}/"));
        let og_image = self.og_image_tag();
        let description = format!(
            "All {} term(s): browse pages by {taxonomy_title}.",
            sorted_terms.len()
        );
        let suffix = self.site_title_suffix();
        let mut out = format!(
            "<!DOCTYPE html>\n<html lang=\"{lang}\">\n<head>\
             <meta charset=\"utf-8\">{canonical}\
             <meta name=\"generator\" content=\"ssg-taxonomy\">\
             <meta name=\"description\" content=\"{description}\">\
             <meta property=\"og:title\" content=\"{taxonomy_title}\">\
             <meta property=\"og:type\" content=\"website\">\
             {og_image}\
             <meta name=\"twitter:card\" content=\"summary\">\
             <title>{taxonomy_title}{suffix}</title></head>\n\
             <body>\n<main>\n<h1>{taxonomy_title}</h1>\n<ul>\n"
        );
        for (term, pages) in sorted_terms {
            let slug = slugify(term);
            out.push_str(&format!(
                "<li><a href=\"/{taxonomy_name}/{slug}/\">{term}</a> ({})</li>\n",
                pages.len()
            ));
        }
        out.push_str("</ul>\n</main>\n</body>\n</html>\n");
        Ok(out)
    }

    /// The locale's URL segment (`""` for the default locale, `/fr`
    /// otherwise), used to prefix `page_url`. Mirrors the `templates`
    /// implementation so both paths address the same page.
    fn locale_path_segment(&self) -> String {
        self.locale
            .as_ref()
            .map_or_else(String::new, |(_, _, seg)| seg.clone())
    }

    /// The tree's language.
    ///
    /// A locale-scoped tree carries its own language and it wins over
    /// the site default — that override is the whole point of
    /// splitting the trees. `None` means the default locale, which
    /// deliberately keeps the site's configured tag: replacing it with
    /// the bare locale code turned `en-GB` into `en`.
    fn lang(&self) -> String {
        if let Some((Some(language), _, _)) = self.locale.as_ref() {
            return language.clone();
        }
        self.ctx
            .config
            .as_ref()
            .map(|c| c.language.clone())
            .unwrap_or_else(|| "en".to_string())
    }

    /// Inline canonical link — taxonomy pages bypass the transform
    /// chain, so the `CanonicalPlugin` never sees them (#586 port 5).
    ///
    /// `page_url` is locale-prefixed here rather than by the caller, so
    /// the canonical points at the file that was actually written
    /// (`/fr/tags/rust/`) instead of the default locale's copy.
    fn canonical(&self, page_url: &str) -> String {
        let segment = self.locale_path_segment();
        self.ctx
            .config
            .as_ref()
            .map(|c| c.base_url.trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .map(|b| {
                format!(
                    "<link rel=\"canonical\" href=\"{b}{segment}{page_url}\">"
                )
            })
            .unwrap_or_default()
    }

    /// Inline `og:image` fallback — see `SsgConfig::og_image` doc.
    /// Absent config or unset field ⇒ empty string, so the meta tag
    /// is omitted entirely rather than emitted with a blank `content`.
    fn og_image_tag(&self) -> String {
        self.ctx
            .config
            .as_ref()
            .and_then(|c| c.og_image.as_ref())
            .map(|image| {
                format!("<meta property=\"og:image\" content=\"{image}\">")
            })
            .unwrap_or_default()
    }
}

/// Resolves the user's template directory, preferring
/// `<template_dir>/tera/` (the canonical layout) but falling back to
/// `<template_dir>/` if `tera/` is absent.
#[cfg(feature = "templates")]
fn resolve_user_template_dir(ctx: &PluginContext) -> Option<PathBuf> {
    // Only `<template_dir>/tera`, never `<template_dir>` itself.
    //
    // Taxonomy pages render through MiniJinja, but page layouts render
    // through StaticWeaver — two engines, and historically one directory.
    // Falling back to the layouts directory therefore fed a StaticWeaver
    // `base.html` to MiniJinja, which failed to parse it and aborted the
    // *whole build* with `syntax error: unexpected character (in
    // base.html:26)` while naming `tag.html`, a file the author never
    // wrote. Taxonomy was unusable for any theme using the default engine.
    //
    // `tera/` is the documented home for MiniJinja templates. A theme that
    // wants to restyle its taxonomy pages puts them there; a theme that
    // does not gets the built-in fallbacks and a site that builds.
    let tera = ctx.template_dir.join("tera");
    tera.is_dir().then_some(tera)
}

/// Converts a list of (title, url) pairs into JSON page objects with
/// `title` and `url` keys, suitable for template iteration.
#[cfg(feature = "templates")]
/// Structured data for a topic page: what it is, what it lists, and where
/// it sits.
///
/// Three types, because they answer three different questions and search
/// engines read them separately. `CollectionPage` says this page is a
/// collection rather than an article; `ItemList` says what is in it and in
/// what order, which is the whole point of a curated topic; and
/// `BreadcrumbList` says where it sits, so a result can be shown as
/// Home → Topics → Payments instead of a bare URL.
///
/// URLs are absolute where a `base_url` is configured, because consumers of
/// structured data cannot resolve a site-relative path. Without one the
/// relative form is emitted rather than a fabricated origin.
fn topic_jsonld(
    base_url: &str,
    taxonomy_title: &str,
    term: &str,
    page_url: &str,
    lede: Option<&str>,
    pages: &[PageRef],
) -> serde_json::Value {
    let base = base_url.trim_end_matches('/');
    let abs = |path: &str| -> String {
        if base.is_empty() || path.starts_with("http") {
            path.to_string()
        } else {
            format!("{base}{path}")
        }
    };

    let items: Vec<serde_json::Value> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| {
            serde_json::json!({
                "@type": "ListItem",
                "position": i + 1,
                "url": abs(&p.url),
                "name": p.title,
            })
        })
        .collect();

    let mut collection = serde_json::json!({
        "@type": "CollectionPage",
        "name": term,
        "url": abs(page_url),
        "mainEntity": {
            "@type": "ItemList",
            "numberOfItems": pages.len(),
            "itemListElement": items,
        },
    });
    if let Some(lede) = lede {
        if let Some(obj) = collection.as_object_mut() {
            let _ = obj.insert(
                "description".to_string(),
                serde_json::Value::String(lede.to_string()),
            );
        }
    }

    // The topics hub is the parent of every topic page; `page_url` ends in
    // the term slug, so trimming one segment reaches it.
    let hub = page_url.trim_end_matches('/');
    let hub = hub.rsplit_once('/').map_or("/", |(head, _)| head);
    let breadcrumbs = serde_json::json!({
        "@type": "BreadcrumbList",
        "itemListElement": [
            {"@type": "ListItem", "position": 1, "name": "Home", "item": abs("/")},
            {"@type": "ListItem", "position": 2, "name": taxonomy_title, "item": abs(&format!("{hub}/"))},
            {"@type": "ListItem", "position": 3, "name": term, "item": abs(page_url)},
        ],
    });

    serde_json::json!({
        "@context": "https://schema.org",
        "@graph": [collection, breadcrumbs],
    })
}

fn pages_to_json(pages: &[PageRef]) -> Vec<serde_json::Value> {
    pages
        .iter()
        .map(|p| serde_json::to_value(p).unwrap_or(serde_json::Value::Null))
        .collect()
}

/// Extracts string terms from a JSON value (array of strings or comma-separated string) into the given map.
fn extract_terms_from_value(
    value: &serde_json::Value,
    map: &mut TaxonomyMap,
    page: &PageRef,
    allow_string: bool,
) {
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(s) = item.as_str() {
                for term in ssg_core::split_terms(s) {
                    map.entry(term).or_default().push(page.clone());
                }
            }
        }
    } else if allow_string {
        if let Some(s) = value.as_str() {
            for term in ssg_core::split_terms(s) {
                map.entry(term).or_default().push(page.clone());
            }
        }
    }
}

/// Collects taxonomy entries (tags, categories, topics) from sidecar JSON files.
///
/// `site_dir` is consulted to prefer pretty (directory-shaped) member
/// URLs — when `<site>/<stem>/index.html` exists the member link is
/// `/<stem>/`, otherwise the flat `/<stem>.html` form is used, so
/// term pages never link to paths that 404 (#586 port 5).
/// Splits a taxonomy map into one map per locale.
///
/// A page's locale is the first path segment of its staged stem when that
/// segment is a configured locale (`fr/a-propos` -> `fr`); everything else
/// belongs to the default locale. The stem is not available here, so the
/// URL is used instead — it carries the same segment after `url_prefix`.
///
/// ## Why this exists
///
/// Taxonomy was locale-blind. Every locale's pages landed in one index at
/// `tags/`, so an English-language tag page listed French pages beside
/// English ones, and a French reader had no tag index at all. Grouping by
/// locale means each index lists only pages a reader of that locale can
/// actually read.
fn split_map_by_locale(
    map: &TaxonomyMap,
    locales: &[String],
    default_locale: &str,
    url_prefix: &str,
) -> BTreeMap<String, TaxonomyMap> {
    let mut out: BTreeMap<String, TaxonomyMap> = BTreeMap::new();
    for (term, entries) in map {
        for entry in entries {
            let url = entry.url.as_str();
            let rest = url.strip_prefix(url_prefix).unwrap_or(url);
            let seg =
                rest.trim_start_matches('/').split('/').next().unwrap_or("");
            let locale = locales
                .iter()
                .find(|l| l.as_str() == seg && l.as_str() != default_locale)
                .map_or(default_locale, String::as_str);
            out.entry(locale.to_string())
                .or_default()
                .entry(term.clone())
                .or_default()
                .push(entry.clone());
        }
    }
    out
}

fn collect_taxonomy_entries(
    sidecar_dir: &Path,
    site_dir: &Path,
    url_prefix: &str,
) -> Result<(TaxonomyMap, TaxonomyMap, TaxonomyMap), SsgError> {
    let sidecars = collect_json_files(sidecar_dir)?;
    let mut tags: TaxonomyMap = HashMap::new();
    let mut categories: TaxonomyMap = HashMap::new();
    let mut topics: TaxonomyMap = HashMap::new();

    for sidecar_path in &sidecars {
        let content =
            fs::read_to_string(sidecar_path).with_path(sidecar_path)?;
        let meta: HashMap<String, serde_json::Value> =
            match serde_json::from_str(&content) {
                Ok(m) => m,
                Err(_) => continue,
            };

        let title = meta
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled")
            .to_string();

        let rel_stem = sidecar_path
            .strip_prefix(sidecar_dir)
            .unwrap_or(sidecar_path)
            .with_extension("")
            .with_extension("");
        let stem = rel_stem.to_string_lossy().replace('\\', "/");
        // Prefixed from `base_url`, like the extracted `_csp/` assets and
        // the islands loader: a site published under a sub-path resolves a
        // bare `/articles/` against the domain root, so every link on every
        // taxonomy page would 404.
        let url = if site_dir.join(&rel_stem).join("index.html").exists() {
            format!("{url_prefix}/{stem}/")
        } else {
            format!("{url_prefix}/{stem}.html")
        };

        // Everything a card needs, read once. A field the page does not
        // declare stays `None` and the template omits it; nothing here is
        // required, and a page with only a title still lists.
        let field = |key: &str| -> Option<String> {
            meta.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        };
        let page = PageRef {
            title: title.clone(),
            url: url.clone(),
            description: field("description"),
            date: field("date"),
            banner: field("banner"),
        };

        // Both the array (`tags: [a, b]`) and comma-separated string
        // (`tags: "a, b"`) frontmatter shapes are accepted — the
        // bundled examples use the string form (#586 port 5).
        if let Some(tag_arr) = meta.get("tags") {
            extract_terms_from_value(tag_arr, &mut tags, &page, true);
        }
        if let Some(cat_arr) = meta.get("categories") {
            extract_terms_from_value(cat_arr, &mut categories, &page, true);
        }
        if let Some(topic_arr) = meta.get("topic_clusters") {
            extract_terms_from_value(topic_arr, &mut topics, &page, true);
        }
    }

    Ok((tags, categories, topics))
}

/// Marker every taxonomy-generated page carries (the
/// `<meta name="generator" content="ssg-taxonomy">` tag). Pages
/// *without* it are author-authored content (e.g. a hand-written
/// `/tags/index.html` compiled from `tags.md`) and are never
/// overwritten (#586 port 5).
const TAXONOMY_MARKER: &str = "ssg-taxonomy";

/// Writes a taxonomy page unless an author-authored page already
/// occupies the path. Our own previous output (identified by
/// [`TAXONOMY_MARKER`]) is refreshed as usual, keeping rebuilds
/// idempotent.
fn write_taxonomy_page(out_file: &Path, html: &str) -> Result<(), SsgError> {
    if let Ok(existing) = fs::read_to_string(out_file) {
        if !existing.contains(TAXONOMY_MARKER) {
            log::debug!(
                "[taxonomy] Keeping author-authored page at {}",
                out_file.display()
            );
            return Ok(());
        }
    }
    fs::write(out_file, html).with_path(out_file)
}

/// Groups terms that share a URL slug into a single term.
///
/// Distinct spellings routinely slugify to one path — `SWIFT` and `Swift`
/// both give `swift`, as do `CBPR` and `CBPR+`. Rendering them separately
/// wrote two different pages to the same `index.html`, so the second write
/// silently discarded the first, and *which* one survived depended on
/// `HashMap` iteration order. Rust randomises that per process, which is why
/// building the same content twice produced different sites and failed the
/// byte-identical-rebuild gate.
///
/// Merging fixes both halves: the build is reproducible, and no member is
/// dropped just because an author capitalised a tag differently. One URL now
/// lists every page that carries any spelling of the term.
///
/// Ordering is a total order — lowercase first, then the term itself as the
/// tiebreak — so the result never depends on iteration order. Groups appear
/// in that order (preserving the previous by-lowercase listing), and the
/// first term in a group supplies the display spelling.
///
/// Returns `(display term, slug, member pages)` per group.
fn merge_terms_by_slug(terms: &TaxonomyMap) -> Vec<MergedTerm> {
    let mut ordered: Vec<_> = terms.iter().collect();
    ordered.sort_by(|(a, _), (b, _)| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });

    let mut out: Vec<MergedTerm> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    // Membership only — dedupe across merged spellings without changing the
    // order members are listed in.
    let mut members: Vec<HashSet<PageRef>> = Vec::new();

    for (term, pages) in ordered {
        let slug = slugify(term);
        if let Some(&i) = index.get(&slug) {
            for page in pages {
                if members[i].insert(page.clone()) {
                    out[i].2.push(page.clone());
                }
            }
        } else {
            let _ = index.insert(slug.clone(), out.len());
            members.push(pages.iter().cloned().collect());
            out.push((term.clone(), slug, pages.clone()));
        }
    }

    // Member pages arrive in whatever order the content walk produced,
    // which is the filesystem's, not ours. Terms were already given a
    // total order above for exactly this reason; members were not, and
    // the omission was invisible on one machine: two builds on the same
    // filesystem agree, so `determinism.yml` -- which compares two runs
    // on one runner -- could never see it.
    //
    // It surfaced when the golden suite compared a macOS-seeded run
    // against Linux: APFS and ext4 enumerate a directory differently, so
    // the tag index listed the same posts in a different order. Sorting
    // by (url, title) is a total order over a set already deduplicated by
    // that pair, so the result is stable everywhere.
    for entry in &mut out {
        entry.2.sort_by(|a, b| {
            a.url.cmp(&b.url).then_with(|| a.title.cmp(&b.title))
        });
    }

    out
}

/// Generates index and term pages for a taxonomy via the template engine.
fn generate_taxonomy_pages(
    site_dir: &Path,
    taxonomy_name: &str,
    taxonomy_title: &str,
    terms: &TaxonomyMap,
    kind: TaxonomyKind,
    renderer: &TaxonomyRenderer<'_>,
    clusters: Option<&TopicClusters>,
) -> Result<(), SsgError> {
    generate_taxonomy_pages_at(
        &site_dir.join(taxonomy_name),
        taxonomy_name,
        taxonomy_title,
        terms,
        kind,
        renderer,
        clusters,
    )
}

/// As [`generate_taxonomy_pages`], but writes into an explicit directory
/// so a multi-locale site can place each locale's tree under its own
/// prefix (`fr/tags/`) instead of sharing one at the site root.
/// Curated metadata applies to the `topics` taxonomy and nothing else.
///
/// Tags and categories are derived vocabulary — there is no editorial
/// copy to attach to them, and a `[tags]` section in the data file would
/// be a mistake rather than a feature.
fn topic_clusters_for<'a>(
    taxonomy_name: &str,
    clusters: &'a TopicClusters,
) -> Option<&'a TopicClusters> {
    (taxonomy_name == "topics" && !clusters.is_empty()).then_some(clusters)
}

fn generate_taxonomy_pages_at(
    tax_dir: &Path,
    taxonomy_name: &str,
    taxonomy_title: &str,
    terms: &TaxonomyMap,
    kind: TaxonomyKind,
    renderer: &TaxonomyRenderer<'_>,
    clusters: Option<&TopicClusters>,
) -> Result<(), SsgError> {
    let tax_dir = tax_dir.to_path_buf();
    fs::create_dir_all(&tax_dir).with_path(&tax_dir)?;

    let merged = merge_terms_by_slug(terms);

    // Per-term pages.
    for (term, slug, pages) in &merged {
        let term_dir = tax_dir.join(slug);
        fs::create_dir_all(&term_dir).with_path(&term_dir)?;

        // Curated metadata for this term, when `_data/topics.toml`
        // describes it (#587). Absent for every other taxonomy, and for
        // any topic nobody has written a pillar page for.
        let cluster = clusters.and_then(|c| c.get(slug.as_str()));

        // A curated `order` leads; everything else keeps the order the
        // taxonomy produced. Only cloned when there is an order to apply.
        let reordered;
        let pages = match cluster {
            Some(c) if !c.order.is_empty() => {
                let mut owned = pages.clone();
                crate::plugins_group::topic_clusters::apply_order(
                    &c.order,
                    &mut owned,
                    |p| p.url.as_str(),
                );
                reordered = owned;
                &reordered
            }
            _ => pages,
        };

        // A curated title replaces the term as displayed, not as slugged,
        // so URLs do not move when someone edits the copy.
        let display_term = cluster
            .and_then(|c| c.title.as_deref())
            .unwrap_or(term.as_str());

        let term_html = renderer.render_term_page(
            kind,
            taxonomy_name,
            taxonomy_title,
            display_term,
            slug,
            pages,
            cluster,
        )?;
        let out_file = term_dir.join("index.html");
        write_taxonomy_page(&out_file, &term_html)?;
    }

    // Taxonomy index page.
    let sorted_terms: Vec<(&String, &Vec<PageRef>)> = merged
        .iter()
        .map(|(term, _, pages)| (term, pages))
        .collect();
    let index_html = renderer.render_index_page(
        taxonomy_name,
        taxonomy_title,
        &sorted_terms,
        clusters,
    )?;
    let out_index = tax_dir.join("index.html");
    write_taxonomy_page(&out_index, &index_html)?;

    Ok(())
}

/// Term → URL slug. Delegates to [`ssg_core::slugify`] (#586 port 5)
/// so taxonomy URLs share the canonical slug rules with the rest of
/// the toolchain.
fn slugify(s: &str) -> String {
    ssg_core::slugify(s)
}

#[cfg(test)]
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn collect_json_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files(dir, "json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::init_logger;
    use std::path::PathBuf;
    use tempfile::{tempdir, TempDir};

    // -------------------------------------------------------------------
    // Test fixtures
    // -------------------------------------------------------------------

    /// Builds a fresh temp dir layout: `<root>/site`, `<root>/build/.meta`
    /// and a `PluginContext`.
    fn make_layout() -> (TempDir, PathBuf, PathBuf, PluginContext) {
        init_logger();
        let dir = tempdir().expect("create tempdir");
        let site = dir.path().join("site");
        let build = dir.path().join("build");
        let meta = build.join(".meta");
        fs::create_dir_all(&site).expect("mkdir site");
        fs::create_dir_all(&meta).expect("mkdir meta");
        let ctx = PluginContext::new(dir.path(), &build, &site, dir.path());
        (dir, site, meta, ctx)
    }

    // -------------------------------------------------------------------
    // slugify — table-driven coverage of the character classes
    // -------------------------------------------------------------------

    #[test]
    fn slugify_table_driven_inputs_produce_expected_slugs() {
        let cases: &[(&str, &str)] = &[
            // basic — alphanumeric + space
            ("Rust Programming", "rust-programming"),
            // punctuation collapsing
            ("C++", "c"),
            ("hello world!", "hello-world"),
            // multiple consecutive non-alphanumerics collapse to one dash
            ("a !! b", "a-b"),
            ("a___b", "a-b"),
            // leading and trailing punctuation are stripped
            ("---rust---", "rust"),
            ("!!!hello!!!", "hello"),
            // unicode letters survive (alphanumeric)
            ("café", "café"),
            // pure punctuation collapses to empty
            ("!!!", ""),
            // already-slug stays the same
            ("rust-web", "rust-web"),
            // mixed digits and letters
            ("Rust 2024", "rust-2024"),
            // empty input
            ("", ""),
        ];
        for &(input, expected) in cases {
            assert_eq!(
                slugify(input),
                expected,
                "slugify({input:?}) should be {expected:?}"
            );
        }
    }

    #[test]
    fn slugify_lowercases_uppercase_input() {
        assert_eq!(slugify("RUST"), "rust");
        assert_eq!(slugify("CamelCase"), "camelcase");
    }

    // -------------------------------------------------------------------
    // capitalize — table-driven (covers None/Some(_) match arms)
    // -------------------------------------------------------------------

    #[test]
    fn capitalize_table_driven_inputs_produce_expected_output() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("a", "A"),
            ("tags", "Tags"),
            ("categories", "Categories"),
            ("Tags", "Tags"),
            ("1", "1"),
        ];
        for &(input, expected) in cases {
            assert_eq!(
                capitalize(input),
                expected,
                "capitalize({input:?}) should be {expected:?}"
            );
        }
    }

    // -------------------------------------------------------------------
    // TaxonomyPlugin — derive surface
    // -------------------------------------------------------------------

    #[test]
    fn taxonomy_plugin_is_copy_after_move() {
        let plugin = TaxonomyPlugin;
        let _copy = plugin;
        assert_eq!(plugin.name(), "taxonomy");
    }

    #[test]
    fn name_returns_static_taxonomy_identifier() {
        assert_eq!(TaxonomyPlugin.name(), "taxonomy");
    }

    // -------------------------------------------------------------------
    // after_compile — early-return paths
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_missing_meta_dir_returns_ok_without_writing() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let build = dir.path().join("build");
        fs::create_dir_all(&site).expect("mkdir site");
        fs::create_dir_all(&build).expect("mkdir build");
        let ctx = PluginContext::new(dir.path(), &build, &site, dir.path());

        TaxonomyPlugin
            .after_compile(&ctx)
            .expect("missing meta is fine");
        assert!(!site.join("tags").exists());
        assert!(!site.join("categories").exists());
    }

    #[test]
    fn after_compile_empty_meta_dir_returns_ok_without_writing() {
        let (_tmp, site, _meta, ctx) = make_layout();
        TaxonomyPlugin
            .after_compile(&ctx)
            .expect("empty meta is fine");
        assert!(!site.join("tags").exists());
        assert!(!site.join("categories").exists());
    }

    #[test]
    fn after_compile_pages_without_taxonomies_emit_no_output() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(meta.join("about.meta.json"), r#"{"title": "About"}"#)
            .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(!site.join("tags").exists());
        assert!(!site.join("categories").exists());
    }

    // -------------------------------------------------------------------
    // after_compile — opt-out (`--no-tag-pages` / SSG_NO_TAG_PAGES)
    // -------------------------------------------------------------------

    /// Builds a context whose config carries `no_taxonomy_pages`.
    fn ctx_with_opt_out(base: &PluginContext, opt_out: bool) -> PluginContext {
        let mut cfg = crate::cmd::SsgConfig::builder()
            .site_name("Example".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("config");
        cfg.no_taxonomy_pages = opt_out;
        PluginContext::with_config(
            &base.content_dir,
            &base.build_dir,
            &base.site_dir,
            &base.template_dir,
            cfg,
        )
    }

    #[test]
    fn no_taxonomy_pages_suppresses_generation() {
        let (_tmp, site, meta, base) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust", "web"], "categories": ["coding"]}"#,
        )
        .unwrap();

        TaxonomyPlugin
            .after_compile(&ctx_with_opt_out(&base, true))
            .unwrap();

        assert!(
            !site.join("tags").exists(),
            "tags tree written despite opt-out"
        );
        assert!(!site.join("categories").exists());
    }

    #[test]
    fn taxonomy_pages_are_generated_by_default() {
        // The opt-out must be exactly that: absent config, or config with the
        // flag unset, keeps the pre-existing behaviour. A site that upgrades
        // without asking for anything sees no change.
        let (_tmp, site, meta, base) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();

        TaxonomyPlugin
            .after_compile(&ctx_with_opt_out(&base, false))
            .unwrap();

        assert!(
            site.join("tags/rust/index.html").exists(),
            "default must still generate taxonomy pages"
        );
    }

    // -------------------------------------------------------------------
    // after_compile — sidecar parsing fallbacks
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_skips_invalid_json_sidecars() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(meta.join("broken.meta.json"), "{not valid").unwrap();
        fs::write(
            meta.join("good.meta.json"),
            r#"{"title": "Good", "tags": ["rust"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("tags/rust/index.html").exists());
    }

    #[test]
    fn after_compile_missing_title_falls_back_to_untitled() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(meta.join("notitle.meta.json"), r#"{"tags": ["rust"]}"#)
            .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(html.contains("Untitled"));
    }

    // -------------------------------------------------------------------
    // Placeholder defaults must never reach a rendered page
    // -------------------------------------------------------------------

    /// A config that leaves `site_title` unset must not brand the tag pages.
    ///
    /// This is the bug: `builtin_templates/tag.html` renders
    /// `Tag: {{ tag }}{% if site.title %} — {{ site.title }}{% endif %}`, and
    /// `DEFAULT_SITE_TITLE` used to be `"My SSG Site"`. Any site built without
    /// an explicit `site_title` therefore titled every generated tag page
    /// `Tag: <term> — My SSG Site`. On sebastienrousseau.com that shipped
    /// 7,189 such pages, live and indexable.
    ///
    /// Nothing caught it because the defaults were only ever asserted equal to
    /// their own constant — `assert_eq!(props["site_title"]["default"], "My
    /// SSG Site")` — which pins the placeholder as correct rather than asking
    /// whether it escapes into output.
    #[test]
    fn tag_page_never_renders_a_placeholder_site_title() {
        let (_tmp, site, meta, base) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();

        // Exactly the bug condition: a valid config that never sets
        // site_title, so whatever DEFAULT_SITE_TITLE holds is rendered.
        let cfg = crate::cmd::SsgConfig::builder()
            .site_name("Example".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            &base.content_dir,
            &base.build_dir,
            &base.site_dir,
            &base.template_dir,
            cfg,
        );

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();

        for placeholder in ["My SSG Site", "MySsgSite", "A site built with SSG"]
        {
            assert!(
                !html.contains(placeholder),
                "generated tag page rendered the placeholder {placeholder:?}; \
                 an unconfigured site must brand nothing.\nRendered:\n{html}"
            );
        }
    }

    /// The positive control: an explicitly configured title *is* rendered.
    ///
    /// Without this, emptying the default would "pass" even if the suffix had
    /// been deleted from the template outright, which would silently drop a
    /// feature configured sites rely on.
    #[test]
    fn tag_page_renders_a_configured_site_title() {
        let (_tmp, site, meta, base) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();

        let cfg = crate::cmd::SsgConfig::builder()
            .site_name("Example".to_string())
            .site_title("Sebastien Rousseau".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            &base.content_dir,
            &base.build_dir,
            &base.site_dir,
            &base.template_dir,
            cfg,
        );

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(
            html.contains("Sebastien Rousseau"),
            "a configured site_title must still reach the tag page"
        );
    }

    /// The index page carries the same branding as the term pages.
    ///
    /// Both renderers build the `<title>` independently, so covering only the
    /// term page let the index drift. This runs in both feature configs, which
    /// is the point: the `templates` build reads the suffix from the bundled
    /// template and the fallback builds it by hand, and the two must agree.
    #[test]
    fn taxonomy_index_renders_a_configured_site_title() {
        let (_tmp, site, meta, base) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();

        let cfg = crate::cmd::SsgConfig::builder()
            .site_name("Example".to_string())
            .site_title("Sebastien Rousseau".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            &base.content_dir,
            &base.build_dir,
            &base.site_dir,
            &base.template_dir,
            cfg,
        );

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let html = fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(
            html.contains("Sebastien Rousseau"),
            "a configured site_title must reach the taxonomy index too"
        );
    }

    /// The negative control for the suffix: no configured title means no
    /// dangling separator left behind in the `<title>`.
    #[test]
    fn taxonomy_pages_omit_the_separator_without_a_site_title() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        for rel in ["tags/index.html", "tags/rust/index.html"] {
            let html = fs::read_to_string(site.join(rel)).unwrap();
            let title = html
                .split("<title>")
                .nth(1)
                .and_then(|t| t.split("</title>").next())
                .unwrap_or_default();
            assert!(
                !title.contains('\u{2014}'),
                "{rel}: an unset site_title must not leave a separator, \
                 got <title>{title}</title>"
            );
        }
    }

    /// Terms separated by a non-ASCII comma must split into separate tags.
    ///
    /// Splitting on ASCII `,` alone collapsed an Arabic or Japanese tag list
    /// into one enormous term, which slugified into a single path component
    /// long enough to hit ENAMETOOLONG on ext4 while succeeding on APFS —
    /// a Linux-only build failure no contributor could reproduce locally.
    #[test]
    fn tag_lists_split_on_the_comma_of_their_own_script() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("intl.meta.json"),
            r#"{"title": "Intl", "tags": "\u0627\u0644\u0645\u0635\u0631\u0641\u064a\u0629\u060c \u0627\u0644\u0645\u062f\u0641\u0648\u0639\u0627\u062a"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let tags = site.join("tags");
        let count = fs::read_dir(&tags).map_or(0, Iterator::count);
        assert!(
            count >= 2,
            "an Arabic-comma tag list must split into separate terms, got \
             {count} director(y|ies) under {}",
            tags.display()
        );
        for entry in fs::read_dir(&tags).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                name.len() <= 200,
                "slug {name:?} is {} bytes; path components are capped at 255 \
                 bytes on ext4 and slugify caps at 200",
                name.len()
            );
        }
    }

    #[test]
    fn after_compile_ignores_non_string_tag_values() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("mixed.meta.json"),
            r#"{"title": "Mixed", "tags": ["rust", 42, null, "web", {"x":1}]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("tags/rust/index.html").exists());
        assert!(site.join("tags/web/index.html").exists());
    }

    #[test]
    fn after_compile_accepts_comma_separated_categories_string() {
        // #586 port 5: the string form `categories: "a, b"` is a
        // first-class frontmatter shape (the bundled examples use it).
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("strcats.meta.json"),
            r#"{"title": "StrCats", "categories": "guides, how-to"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("categories/guides/index.html").exists());
        assert!(site.join("categories/how-to/index.html").exists());
    }

    #[test]
    fn after_compile_ignores_non_string_category_values() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("mixed-cats.meta.json"),
            r#"{"title": "Mixed", "categories": ["blog", 42, null, {"x":1}]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("categories/blog/index.html").exists());
    }

    #[test]
    fn after_compile_accepts_comma_separated_tags_string() {
        // #586 port 5: `tags: "rust, web"` generates a landing page
        // per term, exactly like the array form.
        let (_tmp, site, _meta_dir, ctx) = make_layout();
        let meta_dir = ctx.build_dir.join(".meta");
        fs::write(
            meta_dir.join("strtags.meta.json"),
            r#"{"title": "StrTags", "tags": "rust, web"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("tags/rust/index.html").exists());
        assert!(site.join("tags/web/index.html").exists());
        let html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(html.contains("StrTags"));
    }

    #[test]
    fn after_compile_ignores_non_string_non_array_tags_field() {
        // Numbers / objects still don't produce terms.
        let (_tmp, site, _meta_dir, ctx) = make_layout();
        let meta_dir = ctx.build_dir.join(".meta");
        fs::write(
            meta_dir.join("badtype.meta.json"),
            r#"{"title": "BadType", "tags": 42}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(!site.join("tags").exists());
    }

    #[test]
    fn slug_colliding_terms_merge_into_one_page() {
        // `SWIFT` and `Swift` slugify to the same `swift`, so both used to
        // render to tags/swift/index.html and the second write silently
        // discarded the first. Whichever won depended on HashMap iteration
        // order, which is randomised per process — so the same input built
        // twice produced different sites. The real corpus has 595 such
        // collisions across 34 locales (SWIFT/Swift, CBPR/CBPR+,
        // Governance/governance).
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "Upper", "tags": ["SWIFT"]}"#,
        )
        .unwrap();
        fs::write(
            meta.join("b.meta.json"),
            r#"{"title": "Mixed", "tags": ["Swift"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let page =
            fs::read_to_string(site.join("tags/swift/index.html")).unwrap();
        assert!(
            page.contains("Upper") && page.contains("Mixed"),
            "both spellings share one URL, so one page must list both \
             members; neither may be silently dropped. Got:\n{page}"
        );
    }

    #[test]
    fn slug_colliding_terms_appear_once_in_the_index() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "Upper", "tags": ["SWIFT"]}"#,
        )
        .unwrap();
        fs::write(
            meta.join("b.meta.json"),
            r#"{"title": "Mixed", "tags": ["Swift"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let index = fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert_eq!(
            index.matches("/tags/swift/").count(),
            1,
            "one slug must be listed once, not once per spelling:\n{index}"
        );
    }

    #[test]
    fn taxonomy_output_is_byte_identical_across_runs() {
        // The regression the byte-identical-rebuild CI gate caught. Ties in
        // the term ordering were broken by HashMap iteration order, so this
        // compares two independent runs over a corpus dense with ties.
        //
        // Both runs build their own HashMaps, and Rust seeds each instance
        // differently, so an ordering that depends on iteration order will
        // diverge here with overwhelming probability rather than by luck.
        fn build() -> Vec<(String, String)> {
            let (tmp, site, meta, ctx) = make_layout();
            for (i, tags) in [
                r#"["SWIFT", "Governance"]"#,
                r#"["Swift", "governance"]"#,
                r#"["CBPR", "Open Source"]"#,
                r#"["CBPR+", "open source"]"#,
                r#"["ISO 20022", "Payments"]"#,
                r#"["ISO-20022", "payments"]"#,
            ]
            .iter()
            .enumerate()
            {
                fs::write(
                    meta.join(format!("p{i}.meta.json")),
                    format!(r#"{{"title": "P{i}", "tags": {tags}}}"#),
                )
                .unwrap();
            }

            TaxonomyPlugin.after_compile(&ctx).unwrap();

            let mut out = Vec::new();
            let tags_dir = site.join("tags");
            let mut files = crate::walk::walk_files(&tags_dir, "html").unwrap();
            files.sort();
            for f in files {
                let rel = f
                    .strip_prefix(&tags_dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, fs::read_to_string(&f).unwrap()));
            }
            drop(tmp);
            out
        }

        let first = build();
        let second = build();
        assert!(!first.is_empty(), "fixture produced no taxonomy pages");
        assert_eq!(
            first, second,
            "two runs over identical input must produce identical pages"
        );
    }

    #[test]
    fn after_compile_preserves_author_authored_hub_page() {
        // #586 port 5: a hand-written /tags/index.html (compiled from
        // the site's own tags.md, no ssg-taxonomy marker) must never
        // be clobbered by the generated hub.
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();
        let tags_dir = site.join("tags");
        fs::create_dir_all(&tags_dir).unwrap();
        let authored = "<!DOCTYPE html><html><head><title>My topics</title>\
                        </head><body>hand-written</body></html>";
        fs::write(tags_dir.join("index.html"), authored).unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let hub = fs::read_to_string(tags_dir.join("index.html")).unwrap();
        assert_eq!(hub, authored, "author page must be preserved");
        // Term pages are still generated alongside it.
        assert!(site.join("tags/rust/index.html").exists());
    }

    #[test]
    fn after_compile_refreshes_its_own_previous_output() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();
        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let first = fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(
            first.contains(TAXONOMY_MARKER),
            "generated pages carry the marker:\n{first}"
        );

        // Add a second tagged page; the hub must pick it up.
        fs::write(
            meta.join("q.meta.json"),
            r#"{"title": "Q", "tags": ["rust", "web"]}"#,
        )
        .unwrap();
        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let second = fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(second.contains("web"), "refreshed hub lists new term");
    }

    #[test]
    fn after_compile_falls_back_to_site_meta_sidecars() {
        // Real-pipeline layout: sidecars staged at <site>/.meta/ and
        // pretty (directory-shaped) pages on disk.
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let build = dir.path().join("build");
        fs::create_dir_all(site.join(".meta")).unwrap();
        fs::create_dir_all(site.join("hello")).unwrap();
        fs::create_dir_all(&build).unwrap();
        fs::write(
            site.join(".meta/hello.meta.json"),
            r#"{"title": "Hello", "tags": "rust"}"#,
        )
        .unwrap();
        fs::write(site.join("hello/index.html"), "<html></html>").unwrap();
        let ctx = PluginContext::new(dir.path(), &build, &site, dir.path());

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let term =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        // Member link uses the pretty URL because hello/index.html exists.
        assert!(
            term.contains(r#"href="/hello/""#),
            "pretty member URL:\n{term}"
        );
    }

    #[test]
    fn generated_pages_carry_essential_meta() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();
        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(html.contains("name=\"description\""), "{html}");
        assert!(html.contains("property=\"og:title\""), "{html}");
        assert!(html.contains("property=\"og:type\""), "{html}");
        assert!(html.contains("name=\"twitter:card\""), "{html}");
        assert!(html.contains(TAXONOMY_MARKER), "{html}");
    }

    #[test]
    fn term_pages_inline_canonical_and_lang_with_config() {
        // #586 port 5: pages generated in after_compile bypass the
        // fused transform chain (canonical/JSON-LD/a11y plugins never
        // see them), so the essential head elements must be inlined
        // by the taxonomy templates themselves.
        let (_tmp, site, meta, base_ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": "rust"}"#,
        )
        .unwrap();
        let cfg = crate::cmd::SsgConfig::builder()
            .site_name("Example".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            &base_ctx.content_dir,
            &base_ctx.build_dir,
            &base_ctx.site_dir,
            &base_ctx.template_dir,
            cfg,
        );

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(html.contains("<!DOCTYPE html>"), "doctype:\n{html}");
        assert!(html.contains("<html lang="), "lang attr:\n{html}");
        #[cfg(feature = "templates")]
        assert!(
            html.contains(
                r#"<link rel="canonical" href="https://example.com/tags/rust/">"#
            ),
            "canonical:\n{html}"
        );
    }

    #[test]
    fn term_pages_include_og_image_when_configured() {
        let (_tmp, site, meta, base_ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": "rust"}"#,
        )
        .unwrap();
        let cfg = crate::cmd::SsgConfig::builder()
            .site_name("Example".to_string())
            .og_image(Some("/social/default.png".to_string()))
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            &base_ctx.content_dir,
            &base_ctx.build_dir,
            &base_ctx.site_dir,
            &base_ctx.template_dir,
            cfg,
        );

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let term_html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(
            term_html.contains(
                r#"<meta property="og:image" content="/social/default.png">"#
            ),
            "term page missing og:image:\n{term_html}"
        );
        let index_html =
            fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(
            index_html.contains(
                r#"<meta property="og:image" content="/social/default.png">"#
            ),
            "index page missing og:image:\n{index_html}"
        );
    }

    /// #587: curated pillar-page metadata reaches the rendered page.
    ///
    /// ssg already derives `/topics/{slug}/` from front matter. What it
    /// cannot derive is the title a human would choose, the paragraph
    /// saying what the topic is, or which page should lead — so this
    /// asserts all three arrive, and that the pages the curation does not
    /// name keep the order they had.
    #[test]
    fn curated_topic_metadata_reaches_the_pillar_page() {
        let (tmp, site, meta, ctx) = make_layout();
        for (name, title) in [("a", "Alpha"), ("b", "Bravo"), ("c", "Charlie")]
        {
            fs::write(
                meta.join(format!("{name}.meta.json")),
                format!(
                    r#"{{"title": "{title}", "topic_clusters": "payments", "permalink": "/posts/{name}/"}}"#
                ),
            )
            .unwrap();
        }
        let data = tmp.path().join("_data");
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("topics.toml"),
            concat!(
                "[payments]\n",
                "title = \"Payments, end to end\"\n",
                "lede = \"What moves money and what it costs.\"\n",
                "order = [\"c\"]\n",
            ),
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let page = fs::read_to_string(site.join("topics/payments/index.html"))
            .expect("the pillar page was written");

        assert!(
            page.contains("Payments, end to end"),
            "curated title is missing:\n{page}"
        );
        assert!(
            page.contains("What moves money and what it costs."),
            "curated lede is missing:\n{page}"
        );

        // Charlie is curated to lead; Alpha and Bravo keep their order.
        let c = page.find("Charlie").expect("Charlie listed");
        let a = page.find("Alpha").expect("Alpha listed");
        let b = page.find("Bravo").expect("Bravo listed");
        assert!(c < a && a < b, "curated order not applied:\n{page}");
    }

    /// #587: a page that carries card data is rendered as a card.
    ///
    /// The taxonomy used to carry `(title, url)` and nothing else, so a
    /// card was impossible however the template was written — the data had
    /// already been walked past by render time.
    #[test]
    fn pages_with_card_data_render_as_cards() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("rich.meta.json"),
            r#"{"title": "Rich", "topic_clusters": "payments",
                "description": "What moves money.",
                "date": "2026-04-01",
                "banner": "/img/rich.webp"}"#,
        )
        .unwrap();
        fs::write(
            meta.join("bare.meta.json"),
            r#"{"title": "Bare", "topic_clusters": "payments"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let page = fs::read_to_string(site.join("topics/payments/index.html"))
            .expect("page written");

        assert!(page.contains("What moves money."), "description: {page}");
        assert!(page.contains("/img/rich.webp"), "banner: {page}");
        assert!(
            page.contains(r#"<time datetime="2026-04-01">"#),
            "date: {page}"
        );
        // A page with none of it is still listed, as a plain link.
        assert!(page.contains(">Bare</a>"), "bare page still listed: {page}");
    }

    /// #587: topic pages carry CollectionPage, ItemList and BreadcrumbList.
    #[test]
    fn topic_pages_emit_structured_data() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "topic_clusters": "payments"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let page = fs::read_to_string(site.join("topics/payments/index.html"))
            .expect("page written");

        let start = page
            .find(r#"<script type="application/ld+json">"#)
            .expect("a JSON-LD block");
        let body = &page[start..];
        let json = &body[body.find('{').expect("json starts")
            ..=body.rfind('}').expect("json ends")];
        let parsed: serde_json::Value =
            serde_json::from_str(json).expect("JSON-LD must parse");

        let graph = parsed["@graph"].as_array().expect("a @graph");
        let types: Vec<&str> =
            graph.iter().filter_map(|n| n["@type"].as_str()).collect();
        assert!(types.contains(&"CollectionPage"), "{types:?}");
        assert!(types.contains(&"BreadcrumbList"), "{types:?}");
        assert_eq!(graph[0]["mainEntity"]["@type"].as_str(), Some("ItemList"));
        assert_eq!(graph[0]["mainEntity"]["numberOfItems"], 1);
    }

    /// Tags and categories are keyword indexes, not collections.
    #[test]
    fn tag_pages_carry_no_structured_data() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": "rust"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let page = fs::read_to_string(site.join("tags/rust/index.html"))
            .expect("page written");
        assert!(
            !page.contains("application/ld+json"),
            "tag pages stay as they were: {page}"
        );
    }

    /// #587: the hub shows what a topic is, not just its slug.
    #[test]
    fn the_hub_renders_curated_topics_as_cards() {
        let (tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "topic_clusters": "payments"}"#,
        )
        .unwrap();
        let data = tmp.path().join("_data");
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("topics.toml"),
            concat!(
                "[payments]\n",
                "title = \"Payments, end to end\"\n",
                "lede = \"What moves money and what it costs.\"\n",
            ),
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let hub = fs::read_to_string(site.join("topics/index.html"))
            .expect("hub written");

        assert!(hub.contains("Payments, end to end"), "title: {hub}");
        assert!(
            hub.contains("What moves money and what it costs."),
            "lede: {hub}"
        );
        assert!(hub.contains("taxonomy-card"), "rendered as a card: {hub}");
    }

    /// Without a data file nothing changes — the feature can only add.
    #[test]
    fn topics_without_curation_render_exactly_as_before() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "topic_clusters": "payments"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let page = fs::read_to_string(site.join("topics/payments/index.html"))
            .expect("page written");
        assert!(
            page.contains(r#"<span class="term-name">payments</span>"#),
            "the term renders as written, with no curated title: {page}"
        );
        assert!(!page.contains("class=\"lede\""), "no lede: {page}");
        assert!(!page.contains("taxonomy-banner"), "no banner: {page}");
    }

    #[test]
    fn term_pages_omit_og_image_when_not_configured() {
        // Default config has `og_image: None` — the tag/index gates
        // must not emit an empty/broken `og:image` meta tag.
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": "rust"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let term_html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(
            !term_html.contains("og:image"),
            "term page should not carry og:image:\n{term_html}"
        );
        let index_html =
            fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(
            !index_html.contains("og:image"),
            "index page should not carry og:image:\n{index_html}"
        );
    }

    // -------------------------------------------------------------------
    // after_compile — tags and categories generation (built-in templates)
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_generates_index_and_term_pages_for_tags() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p1.meta.json"),
            r#"{"title": "P1", "tags": ["rust", "web"]}"#,
        )
        .unwrap();
        fs::write(
            meta.join("p2.meta.json"),
            r#"{"title": "P2", "tags": ["rust"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        assert!(site.join("tags/index.html").exists());
        assert!(site.join("tags/rust/index.html").exists());
        assert!(site.join("tags/web/index.html").exists());

        let rust =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(rust.contains("P1"));
        assert!(rust.contains("P2"));

        let web = fs::read_to_string(site.join("tags/web/index.html")).unwrap();
        assert!(web.contains("P1"));
        assert!(!web.contains("P2"));
    }

    #[test]
    fn after_compile_generates_index_and_term_pages_for_categories() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p1.meta.json"),
            r#"{"title": "P1", "categories": ["tutorials"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("categories/index.html").exists());
        assert!(site.join("categories/tutorials/index.html").exists());
    }

    #[test]
    fn after_compile_generates_index_and_term_pages_for_topics() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p1.meta.json"),
            r#"{"title": "P1", "topic_clusters": "cloud-native-banking"}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("topics/index.html").exists());
        assert!(site.join("topics/cloud-native-banking/index.html").exists());
    }

    #[test]
    fn after_compile_index_shows_page_count_per_term() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        fs::write(
            meta.join("b.meta.json"),
            r#"{"title": "B", "tags": ["rust"]}"#,
        )
        .unwrap();
        fs::write(
            meta.join("c.meta.json"),
            r#"{"title": "C", "tags": ["rust", "web"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let index = fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(index.contains("(3)"), "rust should have 3 posts:\n{index}");
        assert!(index.contains("(1)"), "web should have 1 post:\n{index}");
    }

    #[test]
    fn after_compile_index_lists_terms_alphabetically_case_insensitive() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["banana", "Apple", "cherry"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let index = fs::read_to_string(site.join("tags/index.html")).unwrap();
        let apple = index.find("Apple").expect("Apple in index");
        let banana = index.find("banana").expect("banana in index");
        let cherry = index.find("cherry").expect("cherry in index");
        assert!(apple < banana, "Apple should sort before banana");
        assert!(banana < cherry, "banana should sort before cherry");
    }

    #[test]
    fn after_compile_tags_and_categories_coexist_independently() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"], "categories": ["tutorials"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("tags/rust/index.html").exists());
        assert!(site.join("categories/tutorials/index.html").exists());
    }

    #[test]
    fn after_compile_idempotent_overwrites_existing_pages() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).expect("first run");
        TaxonomyPlugin.after_compile(&ctx).expect("second run");
        assert!(site.join("tags/rust/index.html").exists());
    }

    #[test]
    fn after_compile_emits_doctype_lang_charset_in_index() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("p.meta.json"),
            r#"{"title": "P", "tags": ["rust"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let html = fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        // Built-in base.html renders `lang="en"` when no config supplies one.
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("<meta charset=\"utf-8\">"));
        assert!(html.contains("Tags"));
    }

    #[test]
    fn after_compile_term_page_links_back_to_source_url() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("hello.meta.json"),
            r#"{"title": "Hello", "tags": ["rust"]}"#,
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        let html =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(
            html.contains(r#"href="/hello.html""#),
            "term page should link back to /hello.html:\n{html}"
        );
    }

    // -------------------------------------------------------------------
    // collect_json_files — recursion + filtering
    // -------------------------------------------------------------------

    /// Regression: a theme whose page layouts are `StaticWeaver` (the
    /// default engine) put a `base.html` in `template_dir` that `MiniJinja`
    /// cannot parse. Falling back to that directory aborted the entire
    /// build with `syntax error: unexpected character (in base.html:26)`,
    /// attributed to `tag.html` — a file the author never wrote.
    // `resolve_user_template_dir` only exists with `templates` on.
    #[cfg(feature = "templates")]
    #[test]
    fn user_templates_come_only_from_the_tera_subdirectory() {
        let dir = tempdir().expect("tempdir");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&templates).unwrap();
        // A StaticWeaver layout, which is not valid MiniJinja.
        fs::write(
            templates.join("base.html"),
            "{{#extends \"base\"}}{{#block \"main\"}}{{!content}}{{/block}}",
        )
        .unwrap();

        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), &templates);
        assert_eq!(
            resolve_user_template_dir(&ctx),
            None,
            "layouts dir must not be offered to MiniJinja"
        );

        // A real `tera/` directory is still honoured.
        let tera = templates.join("tera");
        fs::create_dir_all(&tera).unwrap();
        assert_eq!(resolve_user_template_dir(&ctx), Some(tera));
    }

    #[test]
    fn collect_json_files_returns_empty_for_missing_directory() {
        let dir = tempdir().expect("tempdir");
        let result = collect_json_files(&dir.path().join("missing")).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn collect_json_files_filters_non_json_extensions() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("a.json"), "{}").unwrap();
        fs::write(dir.path().join("b.txt"), "x").unwrap();
        fs::write(dir.path().join("c"), "x").unwrap();

        let result = collect_json_files(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn collect_json_files_recurses_into_nested_subdirectories() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("top.json"), "{}").unwrap();
        fs::write(nested.join("deep.json"), "{}").unwrap();

        let result = collect_json_files(dir.path()).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn collect_json_files_returns_results_sorted() {
        let dir = tempdir().expect("tempdir");
        for name in ["zebra.json", "apple.json", "mango.json"] {
            fs::write(dir.path().join(name), "{}").unwrap();
        }
        let result = collect_json_files(dir.path()).unwrap();
        let names: Vec<_> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["apple.json", "mango.json", "zebra.json"]);
    }

    // -------------------------------------------------------------------
    // TaxonomyTerm — public type smoke test
    // -------------------------------------------------------------------

    #[test]
    fn taxonomy_term_can_be_constructed_and_cloned() {
        let term = TaxonomyTerm {
            name: "Rust".to_string(),
            slug: "rust".to_string(),
            pages: vec![PageRef::new("Hello", "/hello.html")],
        };
        let copy = term;
        assert_eq!(copy.name, "Rust");
        assert_eq!(copy.slug, "rust");
        assert_eq!(copy.pages.len(), 1);
    }

    #[test]
    fn test_generate_taxonomy_pages_invalid_dir_returns_io_error() {
        let tmp = tempdir().unwrap();
        let file_path = tmp.path().join("file");
        fs::write(&file_path, "").unwrap();

        let mut terms = HashMap::new();
        let _ = terms.insert(
            "rust".to_string(),
            vec![PageRef::new("Title", "/hello.html")],
        );

        let ctx =
            PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
        let renderer = TaxonomyRenderer::new(&ctx);
        let res = generate_taxonomy_pages(
            &file_path,
            "tags",
            "Tags",
            &terms,
            TaxonomyKind::Tag,
            &renderer,
            None,
        );
        assert!(res.is_err());
        let err = res.unwrap_err();
        // Branch-free variant check (a `matches!` here would leave its
        // never-taken `_ => false` arm as an uncovered region).
        assert!(format!("{err:?}").contains("Io"));
    }

    // -------------------------------------------------------------------
    // Template loader — user overrides + error branches
    // -------------------------------------------------------------------

    #[test]
    #[cfg(feature = "templates")]
    fn user_templates_in_tera_dir_override_builtins() {
        let (tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        // Custom templates in <template_dir>/tera/ — both end with a
        // newline so the "already ends with \n" branch is taken.
        let tera = tmp.path().join("tera");
        fs::create_dir_all(&tera).unwrap();
        fs::write(
            tera.join("tag.html"),
            "<html>ssg-taxonomy CUSTOMTERM {{ tag }}</html>\n",
        )
        .unwrap();
        fs::write(
            tera.join("taxonomy_index.html"),
            "<html>ssg-taxonomy CUSTOMINDEX</html>\n",
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let term =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(term.contains("CUSTOMTERM"));
        assert!(term.ends_with('\n'));
        let index = fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(index.contains("CUSTOMINDEX"));
        assert!(index.ends_with('\n'));
    }

    /// Taxonomy was locale-blind: every locale's pages landed in one
    /// index, so an English tag page listed French pages beside English
    /// ones and a French reader had no tag index at all.
    #[test]
    fn split_map_by_locale_groups_pages_by_their_url_segment() {
        let mut map: TaxonomyMap = HashMap::new();
        let _ = map.insert(
            "editorial".to_string(),
            vec![
                PageRef::new("About", "/atlas/about/"),
                PageRef::new("À propos", "/atlas/fr/a-propos/"),
            ],
        );
        let locales = vec!["en".to_string(), "fr".to_string()];
        let out = split_map_by_locale(&map, &locales, "en", "/atlas");

        assert_eq!(out.len(), 2, "one map per locale: {out:?}");
        assert_eq!(out["en"]["editorial"].len(), 1);
        assert_eq!(out["en"]["editorial"][0].title, "About");
        assert_eq!(out["fr"]["editorial"].len(), 1);
        assert_eq!(out["fr"]["editorial"][0].title, "À propos");
    }

    /// A page whose first segment is not a locale belongs to the default
    /// locale, not to a phantom one named after the segment.
    #[test]
    fn split_map_by_locale_assigns_unprefixed_pages_to_the_default() {
        let mut map: TaxonomyMap = HashMap::new();
        let _ = map.insert(
            "method".to_string(),
            vec![PageRef::new("Papers", "/atlas/papers/")],
        );
        let locales = vec!["en".to_string(), "fr".to_string()];
        let out = split_map_by_locale(&map, &locales, "en", "/atlas");

        assert_eq!(out.keys().collect::<Vec<_>>(), vec!["en"], "{out:?}");
    }

    /// The default locale keeps the bare prefix, so a directory sharing
    /// its name is not mistaken for a locale-prefixed tree.
    #[test]
    fn split_map_by_locale_does_not_treat_the_default_locale_as_a_prefix() {
        let mut map: TaxonomyMap = HashMap::new();
        let _ = map.insert(
            "t".to_string(),
            vec![PageRef::new("EN dir", "/atlas/en/thing/")],
        );
        let locales = vec!["en".to_string(), "fr".to_string()];
        let out = split_map_by_locale(&map, &locales, "en", "/atlas");

        assert_eq!(out.keys().collect::<Vec<_>>(), vec!["en"], "{out:?}");
    }

    #[test]
    #[cfg(feature = "templates")]
    fn render_term_page_appends_newline_when_template_output_lacks_one() {
        // Counterpart to `user_templates_in_tera_dir_override_builtins`:
        // this custom `tag.html` does NOT end in `\n`, so
        // `render_term_page`'s `if !s.ends_with('\n') { s.push('\n'); }`
        // branch actually fires (previously only the "already has a
        // trailing newline" branch was exercised, since both the
        // built-in templates and the other override test's fixtures
        // happen to already end in `\n`).
        let (tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        let tera = tmp.path().join("tera");
        fs::create_dir_all(&tera).unwrap();
        fs::write(
            tera.join("tag.html"),
            "<html>ssg-taxonomy NO-TRAILING-NEWLINE {{ tag }}</html>",
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let term =
            fs::read_to_string(site.join("tags/rust/index.html")).unwrap();
        assert!(term.contains("NO-TRAILING-NEWLINE"));
        assert!(
            term.ends_with('\n'),
            "render_term_page must append the missing trailing newline"
        );
    }

    #[test]
    #[cfg(feature = "templates")]
    fn render_index_page_appends_newline_when_template_output_lacks_one() {
        // Same as above but for `render_index_page`'s identical
        // `ends_with('\n')` guard.
        let (tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        let tera = tmp.path().join("tera");
        fs::create_dir_all(&tera).unwrap();
        fs::write(
            tera.join("taxonomy_index.html"),
            "<html>ssg-taxonomy NO-TRAILING-NEWLINE-INDEX</html>",
        )
        .unwrap();

        TaxonomyPlugin.after_compile(&ctx).unwrap();

        let index = fs::read_to_string(site.join("tags/index.html")).unwrap();
        assert!(index.contains("NO-TRAILING-NEWLINE-INDEX"));
        assert!(
            index.ends_with('\n'),
            "render_index_page must append the missing trailing newline"
        );
    }

    #[test]
    #[cfg(feature = "templates")]
    fn nonexistent_template_dir_falls_back_to_builtins() {
        // resolve_user_template_dir returns None; the loader skips the
        // user-dir probe entirely.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let build = dir.path().join("build");
        let meta = build.join(".meta");
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&meta).unwrap();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        let ctx = PluginContext::new(
            dir.path(),
            &build,
            &site,
            &dir.path().join("no-such-templates"),
        );

        TaxonomyPlugin.after_compile(&ctx).unwrap();
        assert!(site.join("tags/rust/index.html").exists());
    }

    #[test]
    #[cfg(all(unix, feature = "templates"))]
    fn unreadable_user_term_template_fails_tag_generation() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, _site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        let tpl = tmp.path().join("tag.html");
        fs::write(&tpl, "x").unwrap();
        fs::set_permissions(&tpl, fs::Permissions::from_mode(0o000)).unwrap();

        let res = TaxonomyPlugin.after_compile(&ctx);

        let _ = fs::set_permissions(&tpl, fs::Permissions::from_mode(0o644));
        // Root CI runners bypass perms; only assert when it errored.
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(all(unix, feature = "templates"))]
    fn unreadable_user_category_template_fails_category_generation() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, _site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "categories": ["guides"]}"#,
        )
        .unwrap();
        let tpl = tmp.path().join("category.html");
        fs::write(&tpl, "x").unwrap();
        fs::set_permissions(&tpl, fs::Permissions::from_mode(0o000)).unwrap();

        let res = TaxonomyPlugin.after_compile(&ctx);

        let _ = fs::set_permissions(&tpl, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(all(unix, feature = "templates"))]
    fn unreadable_user_archive_template_fails_topic_generation() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, _site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "topic_clusters": ["wasm"]}"#,
        )
        .unwrap();
        let tpl = tmp.path().join("archive.html");
        fs::write(&tpl, "x").unwrap();
        fs::set_permissions(&tpl, fs::Permissions::from_mode(0o000)).unwrap();

        let res = TaxonomyPlugin.after_compile(&ctx);

        let _ = fs::set_permissions(&tpl, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(all(unix, feature = "templates"))]
    fn unreadable_user_index_template_fails_index_generation() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, _site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        let tpl = tmp.path().join("taxonomy_index.html");
        fs::write(&tpl, "x").unwrap();
        fs::set_permissions(&tpl, fs::Permissions::from_mode(0o000)).unwrap();

        let res = TaxonomyPlugin.after_compile(&ctx);

        let _ = fs::set_permissions(&tpl, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(feature = "templates")]
    fn user_term_template_extending_missing_base_fails_render() {
        // `{% extends "missing.html" %}` compiles but fails at render
        // time, exercising the render map_err and the loader's
        // unknown-name `None` fallback.
        let (tmp, _site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        // User templates now live in `tera/` only — a StaticWeaver
        // layout sitting in the flat template dir must never reach
        // MiniJinja. The intent of this test is unchanged: a *user*
        // template whose parent is missing still fails the render.
        let tera = tmp.path().join("tera");
        fs::create_dir_all(&tera).unwrap();
        fs::write(tera.join("tag.html"), "{% extends \"missing.html\" %}")
            .unwrap();

        let err = TaxonomyPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    #[cfg(feature = "templates")]
    fn user_index_template_extending_missing_base_fails_render() {
        let (tmp, _site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        // User templates now live in `tera/` only — a StaticWeaver
        // layout sitting in the flat template dir must never reach
        // MiniJinja. The intent of this test is unchanged: a *user*
        // template whose parent is missing still fails the render.
        let tera = tmp.path().join("tera");
        fs::create_dir_all(&tera).unwrap();
        fs::write(
            tera.join("taxonomy_index.html"),
            "{% extends \"missing.html\" %}",
        )
        .unwrap();

        let err = TaxonomyPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    // -------------------------------------------------------------------
    // Sidecar collection — IO error branches
    // -------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn unreadable_sidecar_file_fails_collection() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, _site, meta, ctx) = make_layout();
        let sidecar = meta.join("locked.meta.json");
        fs::write(&sidecar, r#"{"title": "L"}"#).unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o000))
            .unwrap();

        let res = TaxonomyPlugin.after_compile(&ctx);

        let _ =
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_meta_subdir_fails_collection() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, _site, meta, ctx) = make_layout();
        let sub = meta.join("locked");
        fs::create_dir_all(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();

        let res = TaxonomyPlugin.after_compile(&ctx);

        let _ = fs::set_permissions(&sub, fs::Permissions::from_mode(0o755));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    // -------------------------------------------------------------------
    // generate_taxonomy_pages — write error branches
    // -------------------------------------------------------------------

    #[test]
    fn term_dir_squatted_by_file_fails_generation() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        fs::create_dir_all(site.join("tags")).unwrap();
        fs::write(site.join("tags/rust"), "not a dir").unwrap();

        let err = TaxonomyPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn term_index_squatted_by_dir_fails_write() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        fs::create_dir_all(site.join("tags/rust/index.html")).unwrap();

        let err = TaxonomyPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn taxonomy_index_squatted_by_dir_fails_write() {
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(
            meta.join("a.meta.json"),
            r#"{"title": "A", "tags": ["rust"]}"#,
        )
        .unwrap();
        fs::create_dir_all(site.join("tags/index.html")).unwrap();

        let err = TaxonomyPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn write_taxonomy_page_logs_when_keeping_author_page() {
        // init_logger raises the level so the log::debug! format
        // argument region executes.
        init_logger();
        let dir = tempdir().unwrap();
        let page = dir.path().join("index.html");
        fs::write(&page, "<html>hand-written</html>").unwrap();

        write_taxonomy_page(&page, "<html>ssg-taxonomy</html>").unwrap();
        let kept = fs::read_to_string(&page).unwrap();
        assert!(kept.contains("hand-written"));
    }

    // -------------------------------------------------------------------
    // extract_terms_from_value — remaining branches
    // -------------------------------------------------------------------

    #[test]
    fn extract_terms_string_ignored_when_strings_disallowed() {
        let mut map: TaxonomyMap = HashMap::new();
        let value = serde_json::json!("rust, web");
        let page = PageRef::new("T", "/t.html");
        extract_terms_from_value(&value, &mut map, &page, false);
        assert!(map.is_empty());
    }

    #[test]
    fn extract_terms_array_skips_whitespace_only_parts() {
        let mut map: TaxonomyMap = HashMap::new();
        let value = serde_json::json!(["ok", " , "]);
        let page = PageRef::new("T", "/t.html");
        extract_terms_from_value(&value, &mut map, &page, true);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("ok"));
    }

    #[test]
    fn extract_terms_string_skips_empty_parts() {
        let mut map: TaxonomyMap = HashMap::new();
        let value = serde_json::json!("a,,b");
        let page = PageRef::new("T", "/t.html");
        extract_terms_from_value(&value, &mut map, &page, true);
        assert_eq!(map.len(), 2);
    }

    // -------------------------------------------------------------------
    // Fallback renderer — locale handling without the `templates` feature
    // -------------------------------------------------------------------
    //
    // These only compile with default features off, which is the whole
    // point: that renderer is a separate implementation and used to
    // ignore `locale` entirely, so a French tree emitted the site's
    // default language and an un-prefixed canonical. Nothing caught it,
    // because nothing tested this configuration.
    //
    // The coverage job runs a second `--no-default-features` pass so
    // these are measured rather than counted as dead lines on a diff.
    #[cfg(not(feature = "templates"))]
    mod no_templates_renderer {
        use super::*;

        fn cfg_ctx(base: &PluginContext) -> PluginContext {
            let cfg = crate::cmd::SsgConfig::builder()
                .site_name("Example".to_string())
                .base_url("https://example.com".to_string())
                .build()
                .expect("config");
            PluginContext::with_config(
                &base.content_dir,
                &base.build_dir,
                &base.site_dir,
                &base.template_dir,
                cfg,
            )
        }

        #[test]
        fn default_locale_keeps_the_site_language_and_bare_canonical() {
            let (_tmp, _site, _meta, base) = make_layout();
            let ctx = cfg_ctx(&base);
            let renderer = TaxonomyRenderer::new(&ctx);

            assert_eq!(renderer.locale_path_segment(), "");
            assert_eq!(
                renderer.canonical("/tags/rust/"),
                r#"<link rel="canonical" href="https://example.com/tags/rust/">"#
            );
        }

        #[test]
        fn scoped_locale_overrides_language_and_prefixes_canonical() {
            let (_tmp, _site, _meta, base) = make_layout();
            let ctx = cfg_ctx(&base);
            let renderer = TaxonomyRenderer::new(&ctx).for_locale(
                Some("fr"),
                "/fr",
                "/fr",
            );

            assert_eq!(renderer.lang(), "fr");
            assert_eq!(renderer.locale_path_segment(), "/fr");
            assert_eq!(
                renderer.canonical("/tags/rust/"),
                r#"<link rel="canonical" href="https://example.com/fr/tags/rust/">"#,
                "canonical must point at the file actually written"
            );
        }

        #[test]
        fn locale_without_a_language_keeps_the_site_tag() {
            // `None` marks the default locale. Substituting the bare
            // locale code here is what turned `en-GB` into `en`.
            let (_tmp, _site, _meta, base) = make_layout();
            let cfg = crate::cmd::SsgConfig::builder()
                .site_name("Example".to_string())
                .base_url("https://example.com".to_string())
                .language("en-GB".to_string())
                .build()
                .expect("config");
            let ctx = PluginContext::with_config(
                &base.content_dir,
                &base.build_dir,
                &base.site_dir,
                &base.template_dir,
                cfg,
            );
            let renderer = TaxonomyRenderer::new(&ctx).for_locale(None, "", "");

            assert_eq!(renderer.lang(), "en-GB");
        }

        #[test]
        fn lang_falls_back_to_en_without_a_config() {
            let (_tmp, _site, _meta, base) = make_layout();
            let renderer = TaxonomyRenderer::new(&base);
            assert_eq!(renderer.lang(), "en");
            assert_eq!(renderer.canonical("/tags/rust/"), "");
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        /// `slugify` output must contain only (Unicode) alphanumerics and
        /// hyphens, with no leading/trailing/consecutive hyphens.
        ///
        /// NOTE: proptest discovered that `slugify` preserves Unicode
        /// alphanumeric characters (e.g. `𐞀`). This is intentional —
        /// the existing test suite asserts `"café"` -> `"café"`.
        #[test]
        fn slugify_valid_chars(input in "\\PC*") {
            let slug = slugify(&input);
            for ch in slug.chars() {
                prop_assert!(
                    ch.is_alphanumeric() || ch == '-',
                    "unexpected char {:?} in slug {:?}", ch, slug,
                );
            }
            prop_assert!(
                !slug.starts_with('-'),
                "slug must not start with hyphen: {:?}", slug,
            );
            prop_assert!(
                !slug.ends_with('-'),
                "slug must not end with hyphen: {:?}", slug,
            );
            prop_assert!(
                !slug.contains("--"),
                "slug must not contain consecutive hyphens: {:?}", slug,
            );
        }
    }
}
