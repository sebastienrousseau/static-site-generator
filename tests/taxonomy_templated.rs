// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Regression suite for issue #542 — the taxonomy plugin must render
//! tag/category/archive pages through the site's template engine so
//! they share the site's `base.html`, CSS, nav, footer, and lang
//! attribute. Before the refactor the plugin emitted hardcoded HTML
//! with `<html lang="en">` and no site chrome.
//!
//! Each test below maps to one acceptance criterion in #542:
//!
//! - AC1 — user-provided `templates/tera/tag.html` is honoured.
//! - AC2 — built-in fallback extends a `base.html` (CSS link present).
//! - AC3 — `<html lang="…">` comes from `SsgConfig::language`.
//! - AC4 — categories and topics/archives go through the same engine.
//! - AC5 — scaffold-style fixture renders with a stylesheet link.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ssg::cmd::{ImageConfig, SsgConfig};
use ssg::plugin::{Plugin, PluginContext};
use ssg::taxonomy::TaxonomyPlugin;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

/// Constructs a `PluginContext` with optional `SsgConfig` whose
/// `template_dir`, `build_dir`, and `site_dir` are inside `root`.
fn make_ctx(root: &Path, config: Option<SsgConfig>) -> PluginContext {
    let content = root.join("content");
    let build = root.join("build");
    let site = root.join("site");
    let template_dir = root.join("templates");
    let meta = build.join(".meta");
    for d in [&content, &build, &site, &template_dir, &meta] {
        fs::create_dir_all(d).expect("mkdir");
    }
    if let Some(cfg) = config {
        PluginContext::with_config(&content, &build, &site, &template_dir, cfg)
    } else {
        PluginContext::new(&content, &build, &site, &template_dir)
    }
}

/// Standard `SsgConfig` with the supplied language code; everything
/// else is harmless defaults so the test stays focused on lang.
fn config_with_language(root: &Path, language: &str) -> SsgConfig {
    SsgConfig {
        listings: Vec::new(),
        site_name: "Test Site".to_string(),
        site_title: "Test Title".to_string(),
        site_description: "Test description.".to_string(),
        base_url: "https://example.test".to_string(),
        language: language.to_string(),
        content_dir: root.join("content"),
        output_dir: root.join("build"),
        template_dir: root.join("templates"),
        theme: None,
        serve_dir: None,
        i18n: None,
        cdn_prefix: None,
        og_image: None,
        image: ImageConfig::default(),
        edge_headers: ssg::cmd::EdgeHeadersConfig::default(),
        agents: None,
        transitions: false,
        security: ssg::cmd::SecurityConfig::default(),
        no_taxonomy_pages: false,
    }
}

/// Writes a `tera/base.html` that includes a unique nav-link sentinel
/// and a CSS link so the fallback-extends-base path can be asserted on.
fn write_scaffold_base(root: &Path) -> PathBuf {
    let tera = root.join("templates/tera");
    fs::create_dir_all(&tera).unwrap();
    let base = tera.join("base.html");
    fs::write(
        &base,
        r##"<!DOCTYPE html>
<html lang="{{ site.language | default('en') }}">
<head>
  <meta charset="utf-8">
  <title>{% block title %}{{ site.title | default('Site') }}{% endblock %}</title>
  <link rel="stylesheet" href="/assets/style.css">
</head>
<body>
  <header><nav><a href="/" class="nav-home">SCAFFOLD-NAV-LINK</a></nav></header>
  <main>{% block content %}{% endblock %}</main>
  <footer>SCAFFOLD-FOOTER</footer>
</body>
</html>
"##,
    )
    .unwrap();
    base
}

/// Writes a JSON sidecar to `<build>/.meta/<stem>.meta.json`.
fn write_sidecar(root: &Path, stem: &str, body: &str) {
    let meta = root.join("build/.meta");
    fs::create_dir_all(&meta).unwrap();
    fs::write(meta.join(format!("{stem}.meta.json")), body).unwrap();
}

// ---------------------------------------------------------------------
// AC1 — user-provided template wins
// ---------------------------------------------------------------------

#[test]
fn ac1_user_provided_tag_template_is_honoured() {
    let tmp: TempDir = tempdir().unwrap();
    let root = tmp.path();

    // User's tag.html uses a sentinel string only their template emits.
    let tera = root.join("templates/tera");
    fs::create_dir_all(&tera).unwrap();
    // A minimal base.html so any {% extends %} resolves; but the user's
    // tag.html itself doesn't extend — we want to prove the file wins.
    fs::write(
        tera.join("base.html"),
        "<!DOCTYPE html><html lang=\"en\"><body>{% block content %}{% endblock %}</body></html>\n",
    )
    .unwrap();
    fs::write(
        tera.join("tag.html"),
        r#"<!DOCTYPE html><html lang="{{ site.language | default('en') }}">
<head><meta charset="utf-8"><title>USER-TAG-TEMPLATE: {{ tag }}</title></head>
<body><h1>USER-TAG-TEMPLATE: {{ tag }}</h1>
<ul>{% for p in posts %}<li>{{ p.title }}</li>{% endfor %}</ul>
</body></html>
"#,
    )
    .unwrap();

    write_sidecar(root, "hello", r#"{"title": "Hello", "tags": ["rust"]}"#);

    let cfg = config_with_language(root, "en-GB");
    let ctx = make_ctx(root, Some(cfg));
    TaxonomyPlugin.after_compile(&ctx).expect("plugin runs");

    let out = fs::read_to_string(root.join("site/tags/rust/index.html"))
        .expect("term page exists");

    assert!(
        out.contains("USER-TAG-TEMPLATE: rust"),
        "user template sentinel must appear:\n{out}"
    );
    // The user's template also references {{ site.language }} from
    // SsgConfig, confirming the same context shape page templates use.
    assert!(
        out.contains("<html lang=\"en-GB\""),
        "site.language must be wired into the context:\n{out}"
    );
    assert!(out.contains("Hello"));
}

// ---------------------------------------------------------------------
// AC2 — built-in fallback extends base.html
// ---------------------------------------------------------------------

#[test]
fn ac2_builtin_fallback_extends_users_base_html() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Provide ONLY a base.html — no tag.html. The plugin should fall
    // back to the embedded tag.html template, which extends base.html.
    let _base = write_scaffold_base(root);

    write_sidecar(root, "post", r#"{"title": "Post", "tags": ["rust"]}"#);

    let cfg = config_with_language(root, "en-GB");
    let ctx = make_ctx(root, Some(cfg));
    TaxonomyPlugin.after_compile(&ctx).expect("plugin runs");

    let term = fs::read_to_string(root.join("site/tags/rust/index.html"))
        .expect("term page exists");
    let index = fs::read_to_string(root.join("site/tags/index.html"))
        .expect("index page exists");

    // CSS link from scaffold base must be present (proves base.html
    // was inherited, not a hardcoded fallback).
    assert!(
        term.contains(r#"<link rel="stylesheet" href="/assets/style.css">"#),
        "term page must inherit the site CSS link:\n{term}"
    );
    assert!(
        index.contains(r#"<link rel="stylesheet" href="/assets/style.css">"#),
        "index page must inherit the site CSS link:\n{index}"
    );

    // The nav-link sentinel from base.html proves nav was inherited too.
    assert!(
        term.contains("SCAFFOLD-NAV-LINK"),
        "term page must include site nav:\n{term}"
    );
    assert!(
        term.contains("SCAFFOLD-FOOTER"),
        "term page must include site footer:\n{term}"
    );

    // The built-in template's structural marker (the article class)
    // confirms the fallback template — not an inlined raw HTML string —
    // produced the body.
    assert!(
        term.contains("taxonomy-page taxonomy-tag"),
        "fallback tag template should render its article class:\n{term}"
    );
}

// ---------------------------------------------------------------------
// AC3 — lang attribute comes from SsgConfig.language
// ---------------------------------------------------------------------

#[test]
fn ac3_html_lang_attribute_comes_from_site_config() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // No base.html — built-in base.html should be used and honour lang.
    write_sidecar(root, "post", r#"{"title": "Post", "tags": ["rust"]}"#);

    let cfg = config_with_language(root, "fr");
    let ctx = make_ctx(root, Some(cfg));
    TaxonomyPlugin.after_compile(&ctx).expect("plugin runs");

    for page in ["site/tags/index.html", "site/tags/rust/index.html"] {
        let html = fs::read_to_string(root.join(page))
            .unwrap_or_else(|_| panic!("page {page} should exist"));
        assert!(
            html.contains(r#"<html lang="fr">"#),
            "{page} must have lang=fr, got:\n{html}"
        );
        // And it must NOT have the hardcoded English value.
        assert!(
            !html.contains(r#"<html lang="en">"#),
            "{page} must not hardcode lang=en:\n{html}"
        );
    }
}

// ---------------------------------------------------------------------
// AC4 — categories and topics/archives use the same engine path
// ---------------------------------------------------------------------

#[test]
fn ac4_categories_and_topics_render_through_template_engine() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let _base = write_scaffold_base(root);

    write_sidecar(
        root,
        "p1",
        r#"{
            "title": "P1",
            "tags": ["rust"],
            "categories": ["tutorials"],
            "topic_clusters": "cloud-native-banking"
        }"#,
    );

    let cfg = config_with_language(root, "en-GB");
    let ctx = make_ctx(root, Some(cfg));
    TaxonomyPlugin.after_compile(&ctx).expect("plugin runs");

    // All three taxonomies must produce term + index pages.
    let pages = [
        "site/tags/index.html",
        "site/tags/rust/index.html",
        "site/categories/index.html",
        "site/categories/tutorials/index.html",
        "site/topics/index.html",
        "site/topics/cloud-native-banking/index.html",
    ];

    for p in &pages {
        let html = fs::read_to_string(root.join(p))
            .unwrap_or_else(|_| panic!("missing {p}"));
        assert!(
            html.contains(
                r#"<link rel="stylesheet" href="/assets/style.css">"#
            ),
            "{p} must inherit base.html (CSS link), got:\n{html}"
        );
        assert!(
            html.contains("SCAFFOLD-NAV-LINK"),
            "{p} must include site nav, got:\n{html}"
        );
    }

    // Per-kind structural markers confirm the correct built-in template
    // was used for each taxonomy.
    let cat =
        fs::read_to_string(root.join("site/categories/tutorials/index.html"))
            .unwrap();
    assert!(
        cat.contains("taxonomy-page taxonomy-category"),
        "category page should use the category template:\n{cat}"
    );
    let topic = fs::read_to_string(
        root.join("site/topics/cloud-native-banking/index.html"),
    )
    .unwrap();
    assert!(
        topic.contains("taxonomy-page taxonomy-archive"),
        "topic/archive page should use the archive template:\n{topic}"
    );
}

// ---------------------------------------------------------------------
// AC5 — scaffold-style fixture: stylesheet present in DOM
// ---------------------------------------------------------------------

#[test]
fn ac5_scaffold_style_fixture_emits_stylesheet_link() {
    // The acceptance criterion references `cargo run -- new`, but the
    // CLI doesn't currently expose a `new` subcommand. The scaffold's
    // *output shape* is what matters: a base.html that links the site
    // CSS, and taxonomy pages that inherit that link. We reproduce that
    // shape directly from a fixture rather than shelling out, then
    // assert the visible-in-DOM stylesheet link survives.
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let _base = write_scaffold_base(root);

    write_sidecar(root, "first", r#"{"title": "First", "tags": ["rust"]}"#);
    write_sidecar(
        root,
        "second",
        r#"{"title": "Second", "tags": ["rust", "web"]}"#,
    );

    let cfg = config_with_language(root, "en-GB");
    let ctx = make_ctx(root, Some(cfg));
    TaxonomyPlugin.after_compile(&ctx).expect("plugin runs");

    let term = fs::read_to_string(root.join("site/tags/rust/index.html"))
        .expect("rust term page");
    let index = fs::read_to_string(root.join("site/tags/index.html"))
        .expect("tag index");

    assert!(
        term.contains(r#"<link rel="stylesheet" href="/assets/style.css">"#),
        "AC5: scaffold CSS link must be present in term page:\n{term}"
    );
    assert!(
        index.contains(r#"<link rel="stylesheet" href="/assets/style.css">"#),
        "AC5: scaffold CSS link must be present in index page:\n{index}"
    );

    // Sanity — the previous hardcoded HTML emitter never included a
    // stylesheet, so the assertions above are a real regression guard.
    assert!(
        term.contains("First"),
        "term page should list the tagged posts:\n{term}"
    );
}
