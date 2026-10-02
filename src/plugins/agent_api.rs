// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Agent JSON API emitter (issue #586, port 3 of 5).
//!
//! Port of the site's Python-pipeline `AgentApiPlugin` (~382 LOC): a
//! stable, machine-readable JSON API for AI crawlers and agent
//! toolchains. Complements [`crate::ai::AiPlugin`] (llms.txt /
//! llms-full.txt) and the agentic-discovery emitters
//! (`agents.txt` + `ai-plugin.json` + `mcp.json`, issue #552) with a
//! *queryable* content surface rather than a prose one.
//!
//! ## Files emitted (under `<site>/api/agents/`)
//!
//! | File | Shape |
//! | --- | --- |
//! | `index.json`  | API descriptor: site identity, entry counts, absolute links to the other three documents |
//! | `posts.json`  | Array of `{title, url, date, description, tags, locale, wordCount}` — one entry per public content page |
//! | `topics.json` | Object mapping each tag / topic-cluster term to the sorted member post URLs |
//! | `person.json` | JSON-LD (`schema.org`) `Person` entity for the site author |
//!
//! ## Data sources
//!
//! Frontmatter is read from the `.meta.json` sidecars the compiler
//! emits under `<build>/.meta/` (the same convention consumed by
//! [`crate::taxonomy::TaxonomyPlugin`] and the MCP `auto_resources`
//! walker). When the build-dir sidecars are absent (ad-hoc pipelines),
//! `*.meta.json` files next to the HTML in `site_dir` are used as a
//! fallback.
//!
//! `wordCount` resolution order (per the tracker):
//! 1. the sidecar's `word_count` field (stamped by
//!    `frontmatter::emit_sidecars`),
//! 2. a `wordCount` field lifted from a rendered
//!    `BlogPosting`/`Article` JSON-LD block in the page HTML,
//! 3. a manual word count over [`ssg_core::strip_html_tags`] output.
//!
//! ## Default-on and lifecycle
//!
//! Registered unconditionally by
//! `pipeline::register_default_plugins` — the same *default-on, no
//! `ssg.toml` knob* convention [`crate::ai::AiPlugin`] follows (the
//! repo's other opt-in plugins gate by **not being registered**, e.g.
//! [`crate::search_index::VectorSearchPlugin`]). Programmatic opt-out
//! is available via [`AgentApiPlugin::disabled`]. Runs in
//! `after_compile` and honours `ctx.dry_run`.
//!
//! ## Determinism
//!
//! Output must be byte-identical across rebuilds (`determinism.yml`
//! hashes the site): posts are sorted by URL, tag lists are sorted +
//! deduplicated, topics use a [`BTreeMap`], and `serde_json`'s default
//! `BTreeMap`-backed object keeps key order canonical. No timestamps
//! are embedded.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Directory (relative to the site root) the API documents land in.
const API_DIR: &str = "api/agents";

/// One public content page, normalised from its `.meta.json` sidecar.
///
/// # Examples
///
/// ```
/// use ssg::agent_api::PostEntry;
/// let p = PostEntry {
///     title: "Hello".into(),
///     url: "https://example.com/hello.html".into(),
///     date: "2026-01-01".into(),
///     description: "Greeting".into(),
///     tags: vec!["rust".into()],
///     locale: "en".into(),
///     word_count: 42,
///     author: None,
///     topic_clusters: Vec::new(),
/// };
/// assert_eq!(p.word_count, 42);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostEntry {
    /// Page title (sidecar `title` — pages without one are skipped).
    pub title: String,
    /// Absolute URL (base URL + `/{rel}.html`).
    pub url: String,
    /// Publication date string, verbatim from frontmatter (may be empty).
    pub date: String,
    /// Meta description / excerpt / subtitle (may be empty).
    pub description: String,
    /// Sorted, deduplicated tag terms.
    pub tags: Vec<String>,
    /// BCP-47-ish locale (`locale` → `language` → site language → `en`).
    pub locale: String,
    /// Resolved word count (see module docs for the fallback chain).
    pub word_count: u64,
    /// Raw author string, used only to derive `person.json`.
    pub author: Option<String>,
    /// Sorted `topic_clusters` terms (merged into `topics.json`).
    pub topic_clusters: Vec<String>,
}

/// Plugin that emits `/api/agents/{index,posts,topics,person}.json`.
///
/// # Examples
///
/// ```
/// use ssg::agent_api::AgentApiPlugin;
/// use ssg::plugin::Plugin;
/// assert_eq!(AgentApiPlugin::default().name(), "agent-api");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct AgentApiPlugin {
    enabled: bool,
}

impl Default for AgentApiPlugin {
    /// Default-on, per the #586 tracker.
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl AgentApiPlugin {
    /// Creates the default (enabled) plugin.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::agent_api::AgentApiPlugin;
    /// let _p = AgentApiPlugin::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a disabled instance — the programmatic opt-out for
    /// pipelines that must not emit the API surface.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::agent_api::AgentApiPlugin;
    /// let _p = AgentApiPlugin::disabled();
    /// ```
    #[must_use]
    pub const fn disabled() -> Self {
        Self { enabled: false }
    }
}

impl Plugin for AgentApiPlugin {
    fn name(&self) -> &'static str {
        "agent-api"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !self.enabled || ctx.dry_run || !ctx.site_dir.exists() {
            return Ok(());
        }

        let posts = collect_posts(ctx);
        let topics = build_topics_map(&posts);

        let out_dir = ctx.site_dir.join(API_DIR);
        fs::create_dir_all(&out_dir).with_path(&out_dir)?;

        let docs: [(&str, Value); 4] = [
            (
                "index.json",
                build_index_json(ctx, posts.len(), topics.len()),
            ),
            ("posts.json", build_posts_json(&posts)),
            ("topics.json", build_topics_json(&topics)),
            ("person.json", build_person_json(ctx, &posts)),
        ];

        for (name, value) in docs {
            let path = out_dir.join(name);
            let body = to_pretty(&value, &path)?;
            fs::write(&path, body).with_path(&path)?;
        }

        log::info!(
            "[agent-api] Wrote 4 document(s) ({} post(s), {} topic(s)) to {}",
            posts.len(),
            topics.len(),
            out_dir.display()
        );
        Ok(())
    }
}

/// Serialises pretty JSON with a trailing newline (Git/editor friendly,
/// byte-stable across rebuilds).
fn to_pretty(value: &Value, path: &Path) -> Result<String, SsgError> {
    fail_point!("agent_api::to-pretty", |_| {
        Err(SsgError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other("injected: agent_api::to-pretty"),
        })
    });
    let mut body =
        serde_json::to_string_pretty(value).map_err(|e| SsgError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(e),
        })?;
    body.push('\n');
    Ok(body)
}

// =====================================================================
// Collection — sidecar walk + word-count fallback chain
// =====================================================================

/// Collects public posts from `.meta.json` sidecars.
///
/// Sidecar roots are tried in priority order: `<build>/.meta/` (the
/// `emit_sidecars` convention), `<site>/.meta/` (the staged copy the
/// real pipeline leaves in the output — the audited demo site's
/// layout), then `*.meta.json` files sitting next to their HTML in
/// `site_dir` (the layout `ai.rs` consumes). Results are sorted by
/// URL for deterministic output.
///
/// # Examples
///
/// ```
/// use ssg::agent_api::collect_posts;
/// use ssg::plugin::PluginContext;
/// let tmp = tempfile::tempdir().unwrap();
/// let ctx = PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
/// assert!(collect_posts(&ctx).is_empty());
/// ```
#[must_use]
pub fn collect_posts(ctx: &PluginContext) -> Vec<PostEntry> {
    let roots = [ctx.build_dir.join(".meta"), ctx.site_dir.join(".meta")];
    let mut posts = Vec::new();
    for root in &roots {
        if root.is_dir() {
            posts = collect_from_sidecar_dir(ctx, root);
            if !posts.is_empty() {
                break;
            }
        }
    }
    if posts.is_empty() {
        posts = collect_from_site_dir(ctx);
    }
    posts.sort_by(|a, b| a.url.cmp(&b.url));
    posts
}

/// Walks `<root>/**.meta.json` sidecars (a dedicated sidecar tree).
fn collect_from_sidecar_dir(
    ctx: &PluginContext,
    meta_dir: &Path,
) -> Vec<PostEntry> {
    let files = crate::walk::walk_files(meta_dir, "json").unwrap_or_default();
    let mut posts = Vec::new();
    for sidecar in &files {
        if !is_meta_sidecar(sidecar) {
            continue;
        }
        let rel_stem = sidecar
            .strip_prefix(meta_dir)
            .unwrap_or(sidecar)
            .with_extension("")
            .with_extension("");
        if let Some(post) = read_post(ctx, sidecar, &rel_stem) {
            posts.push(post);
        }
    }
    posts
}

/// Walks `site_dir` for `*.meta.json` files sitting next to their
/// HTML. Skips the `.meta/` staging tree — that layout is handled by
/// [`collect_from_sidecar_dir`].
fn collect_from_site_dir(ctx: &PluginContext) -> Vec<PostEntry> {
    let files =
        crate::walk::walk_files(&ctx.site_dir, "json").unwrap_or_default();
    let mut posts = Vec::new();
    for sidecar in &files {
        if !is_meta_sidecar(sidecar) {
            continue;
        }
        let rel_stem = sidecar
            .strip_prefix(&ctx.site_dir)
            .unwrap_or(sidecar)
            .with_extension("")
            .with_extension("");
        if rel_stem.starts_with(".meta") {
            continue;
        }
        if let Some(post) = read_post(ctx, sidecar, &rel_stem) {
            posts.push(post);
        }
    }
    posts
}

/// Returns `true` for `<stem>.meta.json` file names.
fn is_meta_sidecar(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with(".meta.json"))
}

/// Resolves a sidecar stem (`blog/post`) to its site-relative URL
/// path and on-disk HTML file. Pretty URLs win when the compiled
/// output is directory-shaped (`<stem>/index.html` → `/<stem>/`);
/// otherwise the flat `/<stem>.html` form is used.
fn resolve_page(
    ctx: &PluginContext,
    rel_stem: &Path,
) -> (String, std::path::PathBuf) {
    let stem = rel_stem.to_string_lossy().replace('\\', "/");
    let pretty = ctx.site_dir.join(rel_stem).join("index.html");
    if pretty.exists() {
        (format!("{stem}/"), pretty)
    } else {
        (
            format!("{stem}.html"),
            ctx.site_dir.join(format!("{stem}.html")),
        )
    }
}

/// Parses one sidecar into a [`PostEntry`], or `None` when the page is
/// not public (draft/private/unpublished/error page/untitled).
fn read_post(
    ctx: &PluginContext,
    sidecar: &Path,
    rel_stem: &Path,
) -> Option<PostEntry> {
    let content = fs::read_to_string(sidecar).ok()?;
    let meta: serde_json::Map<String, Value> =
        serde_json::from_str(&content).ok()?;

    if is_excluded(rel_stem, &meta) {
        return None;
    }

    let title = str_field(&meta, "title")?;
    if title.is_empty() {
        return None;
    }

    let (rel_url, html_path) = resolve_page(ctx, rel_stem);
    let base = base_url(ctx);
    let url = if base.is_empty() {
        format!("/{rel_url}")
    } else {
        format!("{base}/{rel_url}")
    };

    let description = str_field(&meta, "description")
        .or_else(|| str_field(&meta, "excerpt"))
        .or_else(|| str_field(&meta, "subtitle"))
        .unwrap_or_default();

    let locale = str_field(&meta, "locale")
        .or_else(|| str_field(&meta, "language"))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| site_language(ctx));

    let word_count = resolve_word_count(&meta, &html_path);

    Some(PostEntry {
        title,
        url,
        date: str_field(&meta, "date").unwrap_or_default(),
        description,
        tags: terms_field(&meta, "tags"),
        locale,
        word_count,
        author: str_field(&meta, "author").filter(|a| !a.is_empty()),
        topic_clusters: terms_field(&meta, "topic_clusters"),
    })
}

/// Mirrors the exclusion rules `ai.rs` applies to `llms.txt`: error
/// pages, drafts, private and unpublished pages never surface.
fn is_excluded(rel_stem: &Path, meta: &serde_json::Map<String, Value>) -> bool {
    let file_name = rel_stem
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if file_name == "404" || file_name.starts_with("error") {
        return true;
    }
    truthy(meta.get("draft"))
        || truthy(meta.get("private"))
        || falsy(meta.get("published"))
}

/// `true` / `"true"` / `"yes"` / `"1"` (bool or string form).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => {
            matches!(s.to_lowercase().as_str(), "true" | "yes" | "1")
        }
        _ => false,
    }
}

/// Explicit `published: false` (bool or string form).
fn falsy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => !*b,
        Some(Value::String(s)) => {
            matches!(s.to_lowercase().as_str(), "false" | "no" | "0")
        }
        _ => false,
    }
}

/// Extracts a string field from the sidecar map.
fn str_field(
    meta: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<String> {
    meta.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Extracts taxonomy terms from either an array of strings or a
/// comma-separated string — the two frontmatter shapes in the wild
/// (the bundled examples use `tags: "a, b, c"`). Sorted + deduped.
fn terms_field(
    meta: &serde_json::Map<String, Value>,
    key: &str,
) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    match meta.get(key) {
        Some(Value::Array(arr)) => {
            for item in arr {
                if let Some(s) = item.as_str() {
                    push_terms(&mut terms, s);
                }
            }
        }
        Some(Value::String(s)) => push_terms(&mut terms, s),
        _ => {}
    }
    terms.sort();
    terms.dedup();
    terms
}

/// Splits on commas, trims, drops empties.
fn push_terms(terms: &mut Vec<String>, raw: &str) {
    for part in raw.split(',') {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            terms.push(trimmed.to_string());
        }
    }
}

/// Resolves the word count via the fallback chain documented on the
/// module: sidecar `word_count` → rendered JSON-LD `wordCount` →
/// manual count over the stripped HTML.
fn resolve_word_count(
    meta: &serde_json::Map<String, Value>,
    html_path: &Path,
) -> u64 {
    if let Some(n) = meta.get("word_count").and_then(Value::as_u64) {
        return n;
    }
    let Ok(html) = fs::read_to_string(html_path) else {
        return 0;
    };
    if let Some(n) = jsonld_word_count(&html) {
        return n;
    }
    ssg_core::strip_html_tags(&html).split_whitespace().count() as u64
}

/// Lifts `wordCount` from any `<script type="application/ld+json">`
/// block in the page (`BlogPosting` / `Article` emit it in the Python
/// original this port mirrors).
///
/// # Examples
///
/// ```
/// use ssg::agent_api::jsonld_word_count;
/// let html = r#"<script type="application/ld+json">
///   {"@type":"BlogPosting","wordCount":321}
/// </script>"#;
/// assert_eq!(jsonld_word_count(html), Some(321));
/// assert_eq!(jsonld_word_count("<p>no jsonld</p>"), None);
/// ```
#[must_use]
pub fn jsonld_word_count(html: &str) -> Option<u64> {
    let mut rest = html;
    while let Some(start) = rest.find("application/ld+json") {
        let after = &rest[start..];
        let open = after.find('>')?;
        let body = &after[open + 1..];
        let close = body.find("</script>")?;
        if let Ok(v) = serde_json::from_str::<Value>(&body[..close]) {
            if let Some(n) = v.get("wordCount").and_then(Value::as_u64) {
                return Some(n);
            }
        }
        rest = &body[close..];
    }
    None
}

// =====================================================================
// Document builders — pure functions, unit-testable without I/O
// =====================================================================

/// Trimmed base URL from the context config (empty when no config).
fn base_url(ctx: &PluginContext) -> String {
    ctx.config
        .as_ref()
        .map(|c| c.base_url.trim_end_matches('/').to_string())
        .unwrap_or_default()
}

/// Site language, defaulting to `en`.
fn site_language(ctx: &PluginContext) -> String {
    ctx.config
        .as_ref()
        .map(|c| c.language.clone())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "en".to_string())
}

/// Builds `index.json` — the API descriptor with counts and absolute
/// links to the other three documents.
#[must_use]
fn build_index_json(
    ctx: &PluginContext,
    post_count: usize,
    topic_count: usize,
) -> Value {
    let base = base_url(ctx);
    let link = |doc: &str| -> String {
        if base.is_empty() {
            format!("/{API_DIR}/{doc}")
        } else {
            format!("{base}/{API_DIR}/{doc}")
        }
    };
    let (name, title, description) = ctx.config.as_ref().map_or_else(
        || (String::new(), String::new(), String::new()),
        |c| {
            (
                c.site_name.clone(),
                c.site_title.clone(),
                c.site_description.clone(),
            )
        },
    );

    json!({
        "api": "ssg-agent-api",
        "version": env!("CARGO_PKG_VERSION"),
        "site": {
            "name": name,
            "title": title,
            "description": description,
            "language": site_language(ctx),
            "url": base,
        },
        "counts": {
            "posts": post_count,
            "topics": topic_count,
        },
        "links": {
            "index": link("index.json"),
            "posts": link("posts.json"),
            "topics": link("topics.json"),
            "person": link("person.json"),
        },
    })
}

/// Builds `posts.json` — a bare array (the tracker-specified shape).
#[must_use]
fn build_posts_json(posts: &[PostEntry]) -> Value {
    Value::Array(
        posts
            .iter()
            .map(|p| {
                json!({
                    "title": p.title,
                    "url": p.url,
                    "date": p.date,
                    "description": p.description,
                    "tags": p.tags,
                    "locale": p.locale,
                    "wordCount": p.word_count,
                })
            })
            .collect(),
    )
}

/// Builds the term → member-URL map from tags and topic clusters.
/// `BTreeMap` keeps term iteration (and therefore output) sorted.
#[must_use]
fn build_topics_map(posts: &[PostEntry]) -> BTreeMap<String, Vec<String>> {
    let mut topics: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for post in posts {
        for term in post.tags.iter().chain(post.topic_clusters.iter()) {
            let urls = topics.entry(term.clone()).or_default();
            if !urls.contains(&post.url) {
                urls.push(post.url.clone());
            }
        }
    }
    for urls in topics.values_mut() {
        urls.sort();
    }
    topics
}

/// Builds `topics.json` — object mapping each term to its member URLs.
#[must_use]
fn build_topics_json(topics: &BTreeMap<String, Vec<String>>) -> Value {
    let mut obj = serde_json::Map::new();
    for (term, urls) in topics {
        let _ = obj.insert(
            term.clone(),
            Value::Array(
                urls.iter().map(|u| Value::String(u.clone())).collect(),
            ),
        );
    }
    Value::Object(obj)
}

/// Builds `person.json` — a JSON-LD `schema.org/Person` entity for the
/// site author.
///
/// The author string is taken from the most frequent `author`
/// frontmatter value across public posts (ties broken
/// lexicographically for determinism), parsed from either
/// `email (Name)` or `Name <email>` form. When no post declares an
/// author the site name stands in.
#[must_use]
fn build_person_json(ctx: &PluginContext, posts: &[PostEntry]) -> Value {
    let raw = dominant_author(posts);
    let (name, email) = raw.as_deref().map_or((None, None), parse_author);

    let fallback_name = ctx
        .config
        .as_ref()
        .map(|c| c.site_name.clone())
        .unwrap_or_default();
    let name = name.filter(|n| !n.is_empty()).unwrap_or(fallback_name);

    let mut obj = serde_json::Map::new();
    let _ = obj.insert(
        "@context".to_string(),
        Value::String("https://schema.org".to_string()),
    );
    let _ =
        obj.insert("@type".to_string(), Value::String("Person".to_string()));
    let _ = obj.insert("name".to_string(), Value::String(name));
    if let Some(email) = email {
        let _ = obj.insert("email".to_string(), Value::String(email));
    }
    let base = base_url(ctx);
    if !base.is_empty() {
        let _ = obj.insert("url".to_string(), Value::String(base));
    }
    Value::Object(obj)
}

/// Most frequent author string across posts (ties: lexicographically
/// smallest — deterministic across filesystem walk orders).
fn dominant_author(posts: &[PostEntry]) -> Option<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for post in posts {
        if let Some(a) = post.author.as_deref() {
            *counts.entry(a).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
        .map(|(a, _)| a.to_string())
}

/// Parses `email (Name)` and `Name <email>` author conventions.
///
/// # Examples
///
/// ```
/// use ssg::agent_api::parse_author;
/// let (name, email) = parse_author("hello@example.com (Jane Doe)");
/// assert_eq!(name.as_deref(), Some("Jane Doe"));
/// assert_eq!(email.as_deref(), Some("hello@example.com"));
///
/// let (name, email) = parse_author("Jane Doe <hello@example.com>");
/// assert_eq!(name.as_deref(), Some("Jane Doe"));
/// assert_eq!(email.as_deref(), Some("hello@example.com"));
///
/// let (name, email) = parse_author("Jane Doe");
/// assert_eq!(name.as_deref(), Some("Jane Doe"));
/// assert!(email.is_none());
/// ```
#[must_use]
pub fn parse_author(raw: &str) -> (Option<String>, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (None, None);
    }
    // `email (Name)` — the convention the bundled examples use.
    if let (Some(open), Some(close)) = (raw.find('('), raw.rfind(')')) {
        if open < close {
            let name = raw[open + 1..close].trim();
            let email = raw[..open].trim();
            let email = email.contains('@').then(|| email.to_string());
            let name = (!name.is_empty()).then(|| name.to_string());
            if name.is_some() || email.is_some() {
                return (name, email);
            }
        }
    }
    // `Name <email>` — the RFC 5322-ish convention.
    if let (Some(open), Some(close)) = (raw.find('<'), raw.rfind('>')) {
        if open < close {
            let email = raw[open + 1..close].trim();
            let name = raw[..open].trim();
            let email = email.contains('@').then(|| email.to_string());
            let name = (!name.is_empty()).then(|| name.to_string());
            if name.is_some() || email.is_some() {
                return (name, email);
            }
        }
    }
    if raw.contains('@') && !raw.contains(' ') {
        return (None, Some(raw.to_string()));
    }
    (Some(raw.to_string()), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::SsgConfig;
    use tempfile::{tempdir, TempDir};

    // -----------------------------------------------------------------
    // Fixtures
    // -----------------------------------------------------------------

    fn make_ctx() -> (TempDir, PluginContext) {
        let dir = tempdir().expect("tempdir");
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        fs::create_dir_all(build.join(".meta")).unwrap();
        fs::create_dir_all(&site).unwrap();
        let cfg = SsgConfig::builder()
            .site_name("Example".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            dir.path(),
            &build,
            &site,
            dir.path(),
            cfg,
        );
        (dir, ctx)
    }

    fn write_sidecar(ctx: &PluginContext, name: &str, json: &str) {
        let p = ctx.build_dir.join(".meta").join(name);
        // `p` always has a parent (it was built via join).
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, json).unwrap();
    }

    fn read_doc(ctx: &PluginContext, name: &str) -> Value {
        let body =
            fs::read_to_string(ctx.site_dir.join(API_DIR).join(name)).unwrap();
        serde_json::from_str(&body).unwrap()
    }

    // -----------------------------------------------------------------
    // Plugin surface
    // -----------------------------------------------------------------

    #[test]
    fn name_is_stable() {
        assert_eq!(AgentApiPlugin::default().name(), "agent-api");
        assert_eq!(AgentApiPlugin::new().name(), "agent-api");
    }

    #[test]
    fn default_is_enabled_and_copyable() {
        let p = AgentApiPlugin::default();
        let copy = p;
        assert!(copy.enabled);
        assert!(format!("{p:?}").contains("AgentApiPlugin"));
    }

    #[test]
    fn disabled_plugin_writes_nothing() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "a.meta.json", r#"{"title":"A"}"#);
        AgentApiPlugin::disabled().after_compile(&ctx).unwrap();
        assert!(!ctx.site_dir.join(API_DIR).exists());
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (_tmp, ctx) = make_ctx();
        let ctx = ctx.with_dry_run(true);
        write_sidecar(&ctx, "a.meta.json", r#"{"title":"A"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        assert!(!ctx.site_dir.join(API_DIR).exists());
    }

    #[test]
    fn missing_site_dir_is_noop() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        let ctx =
            PluginContext::new(dir.path(), dir.path(), &missing, dir.path());
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        assert!(!missing.exists());
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn emits_all_four_documents() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "hello.meta.json",
            r#"{"title":"Hello","tags":["rust"],"word_count":10}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        for doc in ["index.json", "posts.json", "topics.json", "person.json"] {
            assert!(
                ctx.site_dir.join(API_DIR).join(doc).exists(),
                "{doc} missing"
            );
        }
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn documents_end_with_newline() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "a.meta.json", r#"{"title":"A"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let body =
            fs::read_to_string(ctx.site_dir.join(API_DIR).join("posts.json"))
                .unwrap();
        assert!(body.ends_with('\n'));
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn output_is_byte_identical_across_runs() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "a.meta.json",
            r#"{"title":"A","tags":["z","a"],"word_count":5}"#,
        );
        write_sidecar(
            &ctx,
            "b.meta.json",
            r#"{"title":"B","tags":"a, m","word_count":7}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let first =
            fs::read_to_string(ctx.site_dir.join(API_DIR).join("topics.json"))
                .unwrap();
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let second =
            fs::read_to_string(ctx.site_dir.join(API_DIR).join("topics.json"))
                .unwrap();
        assert_eq!(first, second);
    }

    // -----------------------------------------------------------------
    // posts.json shape
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn posts_json_carries_all_tracker_fields() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "post.meta.json",
            r#"{
                "title": "Post",
                "date": "2026-01-02",
                "description": "Desc",
                "tags": ["rust", "web"],
                "locale": "en_GB",
                "word_count": 123
            }"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        let p = &posts.as_array().unwrap()[0];
        assert_eq!(p["title"], "Post");
        assert_eq!(p["url"], "https://example.com/post.html");
        assert_eq!(p["date"], "2026-01-02");
        assert_eq!(p["description"], "Desc");
        assert_eq!(p["tags"], json!(["rust", "web"]));
        assert_eq!(p["locale"], "en_GB");
        assert_eq!(p["wordCount"], 123);
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn posts_sorted_by_url() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "zeta.meta.json", r#"{"title":"Z"}"#);
        write_sidecar(&ctx, "alpha.meta.json", r#"{"title":"A"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        let urls: Vec<&str> = posts
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["url"].as_str().unwrap())
            .collect();
        assert_eq!(
            urls,
            vec![
                "https://example.com/alpha.html",
                "https://example.com/zeta.html"
            ]
        );
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn nested_sidecars_map_to_nested_urls() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "blog/deep.meta.json", r#"{"title":"Deep"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(
            posts.as_array().unwrap()[0]["url"],
            "https://example.com/blog/deep.html"
        );
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn drafts_private_unpublished_and_error_pages_excluded() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "ok.meta.json", r#"{"title":"OK"}"#);
        write_sidecar(&ctx, "draft.meta.json", r#"{"title":"D","draft":true}"#);
        write_sidecar(
            &ctx,
            "draft2.meta.json",
            r#"{"title":"D2","draft":"true"}"#,
        );
        write_sidecar(
            &ctx,
            "priv.meta.json",
            r#"{"title":"P","private":"yes"}"#,
        );
        write_sidecar(
            &ctx,
            "unpub.meta.json",
            r#"{"title":"U","published":false}"#,
        );
        write_sidecar(
            &ctx,
            "unpub2.meta.json",
            r#"{"title":"U2","published":"false"}"#,
        );
        write_sidecar(&ctx, "404.meta.json", r#"{"title":"Not Found"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(posts.as_array().unwrap().len(), 1);
        assert_eq!(posts.as_array().unwrap()[0]["title"], "OK");
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn untitled_and_invalid_sidecars_skipped() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "no-title.meta.json", r#"{"date":"2026"}"#);
        write_sidecar(&ctx, "empty-title.meta.json", r#"{"title":""}"#);
        write_sidecar(&ctx, "broken.meta.json", "{not json");
        write_sidecar(&ctx, "good.meta.json", r#"{"title":"G"}"#);
        // Non-sidecar JSON must be ignored.
        fs::write(ctx.build_dir.join(".meta/plain.json"), r#"{"title":"X"}"#)
            .unwrap();
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(posts.as_array().unwrap().len(), 1);
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn comma_separated_string_tags_are_split_sorted_deduped() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "p.meta.json",
            r#"{"title":"P","tags":"web, rust , rust,,"}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(
            posts.as_array().unwrap()[0]["tags"],
            json!(["rust", "web"])
        );
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn locale_falls_back_language_then_site_then_en() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "a.meta.json", r#"{"title":"A","locale":"fr_FR"}"#);
        write_sidecar(&ctx, "b.meta.json", r#"{"title":"B","language":"de"}"#);
        write_sidecar(&ctx, "c.meta.json", r#"{"title":"C"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        let arr = posts.as_array().unwrap();
        assert_eq!(arr[0]["locale"], "fr_FR");
        assert_eq!(arr[1]["locale"], "de");
        // SsgConfig::builder always supplies a non-empty default
        // language, so the site language is the expected fallback.
        let site_lang = ctx.config.as_ref().unwrap().language.clone();
        assert!(!site_lang.is_empty(), "should not be empty");
        assert_eq!(arr[2]["locale"], *site_lang);
    }

    // -----------------------------------------------------------------
    // wordCount fallback chain
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn word_count_prefers_sidecar_field() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "p.meta.json", r#"{"title":"P","word_count":77}"#);
        fs::write(ctx.site_dir.join("p.html"), "<p>one two</p>").unwrap();
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(posts.as_array().unwrap()[0]["wordCount"], 77);
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn word_count_lifts_from_blogposting_jsonld() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "p.meta.json", r#"{"title":"P"}"#);
        fs::write(
            ctx.site_dir.join("p.html"),
            r#"<html><head><script type="application/ld+json">
               {"@type":"BlogPosting","wordCount":555}
               </script></head><body><p>a b c</p></body></html>"#,
        )
        .unwrap();
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(posts.as_array().unwrap()[0]["wordCount"], 555);
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn word_count_falls_back_to_stripped_html() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "p.meta.json", r#"{"title":"P"}"#);
        fs::write(
            ctx.site_dir.join("p.html"),
            "<html><body><p>one two three four</p></body></html>",
        )
        .unwrap();
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(posts.as_array().unwrap()[0]["wordCount"], 4);
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn word_count_zero_when_html_missing() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "p.meta.json", r#"{"title":"P"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(posts.as_array().unwrap()[0]["wordCount"], 0);
    }

    #[test]
    fn jsonld_word_count_scans_past_blocks_without_field() {
        let html = r#"
            <script type="application/ld+json">{"@type":"WebSite"}</script>
            <script type="application/ld+json">{"wordCount": 9}</script>
        "#;
        assert_eq!(jsonld_word_count(html), Some(9));
    }

    #[test]
    fn jsonld_word_count_handles_malformed_json() {
        let html = r#"<script type="application/ld+json">{oops</script>"#;
        assert_eq!(jsonld_word_count(html), None);
    }

    // -----------------------------------------------------------------
    // topics.json
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn topics_map_terms_to_sorted_member_urls() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "z.meta.json", r#"{"title":"Z","tags":["rust"]}"#);
        write_sidecar(
            &ctx,
            "a.meta.json",
            r#"{"title":"A","tags":["rust","web"]}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let topics = read_doc(&ctx, "topics.json");
        assert_eq!(
            topics["rust"],
            json!(["https://example.com/a.html", "https://example.com/z.html"])
        );
        assert_eq!(topics["web"], json!(["https://example.com/a.html"]));
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn topics_include_topic_clusters() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "p.meta.json",
            r#"{"title":"P","topic_clusters":"cloud-native"}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let topics = read_doc(&ctx, "topics.json");
        assert!(topics.get("cloud-native").is_some());
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn topics_keys_are_sorted() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "p.meta.json",
            r#"{"title":"P","tags":["zeta","alpha","mid"]}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let body =
            fs::read_to_string(ctx.site_dir.join(API_DIR).join("topics.json"))
                .unwrap();
        let a = body.find("\"alpha\"").unwrap();
        let m = body.find("\"mid\"").unwrap();
        let z = body.find("\"zeta\"").unwrap();
        assert!(a < m && m < z, "keys must serialise sorted:\n{body}");
    }

    // -----------------------------------------------------------------
    // person.json
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn person_parses_email_paren_name_convention() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "p.meta.json",
            r#"{"title":"P","author":"hello@threshold.press (Threshold)"}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let person = read_doc(&ctx, "person.json");
        assert_eq!(person["@context"], "https://schema.org");
        assert_eq!(person["@type"], "Person");
        assert_eq!(person["name"], "Threshold");
        assert_eq!(person["email"], "hello@threshold.press");
        assert_eq!(person["url"], "https://example.com");
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn person_falls_back_to_site_name_without_authors() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "p.meta.json", r#"{"title":"P"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let person = read_doc(&ctx, "person.json");
        assert_eq!(person["name"], "Example");
        assert!(person.get("email").is_none());
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn person_picks_most_frequent_author() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "a.meta.json", r#"{"title":"A","author":"Bob"}"#);
        write_sidecar(&ctx, "b.meta.json", r#"{"title":"B","author":"Alice"}"#);
        write_sidecar(&ctx, "c.meta.json", r#"{"title":"C","author":"Alice"}"#);
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let person = read_doc(&ctx, "person.json");
        assert_eq!(person["name"], "Alice");
    }

    #[test]
    fn dominant_author_tie_breaks_lexicographically() {
        let mk = |author: &str| PostEntry {
            title: "T".into(),
            url: "/t.html".into(),
            date: String::new(),
            description: String::new(),
            tags: vec![],
            locale: "en".into(),
            word_count: 0,
            author: Some(author.to_string()),
            topic_clusters: vec![],
        };
        let posts = vec![mk("Zoe"), mk("Anna")];
        assert_eq!(dominant_author(&posts).as_deref(), Some("Anna"));
    }

    #[test]
    fn parse_author_table_driven() {
        let cases: &[(&str, Option<&str>, Option<&str>)] = &[
            ("a@b.c (Name)", Some("Name"), Some("a@b.c")),
            ("Name <a@b.c>", Some("Name"), Some("a@b.c")),
            ("a@b.c", None, Some("a@b.c")),
            ("Just A Name", Some("Just A Name"), None),
            ("", None, None),
            ("   ", None, None),
            ("() ", Some("()"), None),
        ];
        for &(input, name, email) in cases {
            let (n, e) = parse_author(input);
            assert_eq!(n.as_deref(), name, "name for {input:?}");
            assert_eq!(e.as_deref(), email, "email for {input:?}");
        }
    }

    // -----------------------------------------------------------------
    // index.json
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn index_carries_counts_and_absolute_links() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "a.meta.json",
            r#"{"title":"A","tags":["rust","web"]}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let index = read_doc(&ctx, "index.json");
        assert_eq!(index["api"], "ssg-agent-api");
        assert_eq!(index["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(index["counts"]["posts"], 1);
        assert_eq!(index["counts"]["topics"], 2);
        assert_eq!(
            index["links"]["posts"],
            "https://example.com/api/agents/posts.json"
        );
        assert_eq!(
            index["links"]["person"],
            "https://example.com/api/agents/person.json"
        );
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn no_config_uses_relative_links_and_en() {
        let dir = tempdir().unwrap();
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        fs::create_dir_all(build.join(".meta")).unwrap();
        fs::create_dir_all(&site).unwrap();
        let ctx = PluginContext::new(dir.path(), &build, &site, dir.path());
        fs::write(build.join(".meta/p.meta.json"), r#"{"title":"P"}"#).unwrap();
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let body =
            fs::read_to_string(site.join(API_DIR).join("index.json")).unwrap();
        let index: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(index["links"]["index"], "/api/agents/index.json");
        assert_eq!(index["site"]["language"], "en");
        let posts: Value = serde_json::from_str(
            &fs::read_to_string(site.join(API_DIR).join("posts.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(posts.as_array().unwrap()[0]["url"], "/p.html");
    }

    // -----------------------------------------------------------------
    // site-dir sidecar fallback
    // -----------------------------------------------------------------

    #[test]
    fn falls_back_to_site_dir_sidecars() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        // No build/.meta at all — sidecars live next to the HTML.
        fs::write(site.join("p.meta.json"), r#"{"title":"P"}"#).unwrap();
        fs::write(site.join("p.html"), "<p>x y</p>").unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let posts = collect_posts(&ctx);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].title, "P");
        assert_eq!(posts[0].word_count, 2);
    }

    #[test]
    fn truthy_falsy_value_forms() {
        assert!(truthy(Some(&json!(true))));
        assert!(truthy(Some(&json!("yes"))));
        assert!(truthy(Some(&json!("1"))));
        assert!(!truthy(Some(&json!(false))));
        assert!(!truthy(Some(&json!(0))));
        assert!(!truthy(None));
        assert!(falsy(Some(&json!(false))));
        assert!(falsy(Some(&json!("no"))));
        assert!(falsy(Some(&json!("0"))));
        assert!(!falsy(Some(&json!(true))));
        assert!(!falsy(None));
    }

    // -----------------------------------------------------------------
    // Remaining branches — IO errors, parser edges, term shapes
    // -----------------------------------------------------------------

    #[test]
    fn after_compile_fails_when_api_dir_squatted_by_file() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "p.meta.json", r#"{"title":"P"}"#);
        // `site/api` is a file, so create_dir_all(site/api/agents)
        // fails.
        fs::write(ctx.site_dir.join("api"), "not a dir").unwrap();
        let err = AgentApiPlugin::default().after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn after_compile_fails_when_doc_path_squatted_by_dir() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "p.meta.json", r#"{"title":"P"}"#);
        // A directory squats `index.json`, so fs::write fails.
        fs::create_dir_all(ctx.site_dir.join(API_DIR).join("index.json"))
            .unwrap();
        let err = AgentApiPlugin::default().after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_sidecar_is_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "ok.meta.json", r#"{"title":"OK"}"#);
        write_sidecar(&ctx, "locked.meta.json", r#"{"title":"L"}"#);
        let locked = ctx.build_dir.join(".meta/locked.meta.json");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let posts = collect_posts(&ctx);

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o644));
        // Root CI runners bypass perms; the readable sidecar always
        // survives either way.
        assert!(posts.iter().any(|p| p.title == "OK"));
    }

    #[test]
    fn site_dir_walk_skips_meta_tree_and_drafts() {
        // Both sidecar roots yield zero posts (site/.meta only has a
        // draft), so the site-dir walk runs: it must skip the `.meta`
        // staging tree and non-public sidecars while keeping the good
        // one.
        let dir = tempdir().unwrap();
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        fs::create_dir_all(&build).unwrap();
        fs::create_dir_all(site.join(".meta")).unwrap();
        fs::write(
            site.join(".meta/draft.meta.json"),
            r#"{"title":"D","draft":true}"#,
        )
        .unwrap();
        fs::write(site.join("ok.meta.json"), r#"{"title":"OK"}"#).unwrap();
        fs::write(site.join("skip.meta.json"), r#"{"title":"S","draft":true}"#)
            .unwrap();
        let ctx = PluginContext::new(dir.path(), &build, &site, dir.path());

        let posts = collect_posts(&ctx);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].title, "OK");
    }

    #[test]
    fn terms_field_ignores_non_string_array_items() {
        let meta: serde_json::Map<String, Value> =
            serde_json::from_str(r#"{"tags": ["a", 42, null, "b"]}"#).unwrap();
        assert_eq!(terms_field(&meta, "tags"), vec!["a", "b"]);
    }

    #[test]
    fn jsonld_word_count_returns_none_without_tag_close() {
        // `application/ld+json` marker but no `>` afterwards.
        let html = "<script type=\"application/ld+json";
        assert!(jsonld_word_count(html).is_none());
    }

    #[test]
    fn jsonld_word_count_returns_none_without_script_close() {
        let html = "<script type=\"application/ld+json\">{\"wordCount\": 3}";
        assert!(jsonld_word_count(html).is_none());
    }

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn topics_map_dedupes_term_shared_by_tags_and_clusters() {
        let (_tmp, ctx) = make_ctx();
        write_sidecar(
            &ctx,
            "p.meta.json",
            r#"{"title":"P","tags":"x","topic_clusters":"x"}"#,
        );
        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let topics = read_doc(&ctx, "topics.json");
        let urls = topics["x"].as_array().unwrap();
        assert_eq!(urls.len(), 1, "shared term must not duplicate the URL");
    }

    #[test]
    fn parse_author_paren_form_with_empty_parts_falls_through() {
        // `()` — both name and email empty, so the paren branch does
        // not return and the raw string becomes the name.
        let (name, email) = parse_author("()");
        assert_eq!(name.as_deref(), Some("()"));
        assert!(email.is_none());
    }

    #[test]
    fn parse_author_angle_form_email_only() {
        // Empty display name: the `email.is_some()` side of the `||`
        // decides.
        let (name, email) = parse_author("<jane@example.com>");
        assert!(name.is_none());
        assert_eq!(email.as_deref(), Some("jane@example.com"));
    }

    #[test]
    fn parse_author_angle_form_with_empty_parts_falls_through() {
        let (name, email) = parse_author("<>");
        assert_eq!(name.as_deref(), Some("<>"));
        assert!(email.is_none());
    }

    // -------------------------------------------------------------------
    // resolve_page — pretty-URL branch (`<stem>/index.html` on disk)
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(agent_api_failpoint)]
    fn pretty_url_used_when_directory_index_html_exists() {
        // When the compiled output is directory-shaped
        // (`post/index.html`), the URL must be the pretty `/post/`
        // form rather than the flat `/post.html` fallback.
        let (_tmp, ctx) = make_ctx();
        write_sidecar(&ctx, "post.meta.json", r#"{"title":"Post"}"#);
        fs::create_dir_all(ctx.site_dir.join("post")).unwrap();
        fs::write(ctx.site_dir.join("post/index.html"), "<p>hi</p>").unwrap();

        AgentApiPlugin::default().after_compile(&ctx).unwrap();
        let posts = read_doc(&ctx, "posts.json");
        assert_eq!(
            posts.as_array().unwrap()[0]["url"],
            "https://example.com/post/"
        );
    }
}

// =========================================================================
// Fault injection — `agent_api::to-pretty` covers the serde_json
// serialisation error arm of `to_pretty`, which cannot fail via normal
// data (every `Value` built by this module comes from finite numbers
// and UTF-8 strings) and so is otherwise unreachable without fault
// injection.
// =========================================================================
#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::cmd::SsgConfig;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial_test::serial(agent_api_failpoint)]
    fn to_pretty_failpoint_propagates() {
        let _guard = FailGuard("agent_api::to-pretty");
        fail::cfg("agent_api::to-pretty", "return")
            .expect("activate failpoint");

        let dir = tempdir().unwrap();
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        fs::create_dir_all(build.join(".meta")).unwrap();
        fs::create_dir_all(&site).unwrap();
        let cfg = SsgConfig::builder()
            .site_name("Example".to_string())
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            dir.path(),
            &build,
            &site,
            dir.path(),
            cfg,
        );

        let err = AgentApiPlugin::default()
            .after_compile(&ctx)
            .expect_err("injected serialisation failure must propagate");
        assert!(format!("{err:?}").contains("injected: agent_api::to-pretty"));
    }
}
