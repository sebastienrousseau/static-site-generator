// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::taxonomy`, including the
//! per-term landing pages from issue #586 (port 5).

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::plugin::{Plugin, PluginContext};
use ssg::taxonomy::TaxonomyPlugin;
use std::fs;
use tempfile::TempDir;

#[test]
fn taxonomy_plugin_name_is_stable() {
    assert!(!TaxonomyPlugin.name().is_empty(), "should not be empty");
}

/// Fixture site with the comma-separated `tags` string frontmatter the
/// bundled examples (and most real sites) use.
fn fixture_site() -> (TempDir, PluginContext) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let build = tmp.path().join("build");
    let site = tmp.path().join("public");
    let meta = build.join(".meta");
    fs::create_dir_all(&meta).unwrap();
    fs::create_dir_all(&site).unwrap();

    fs::write(
        meta.join("a11y.meta.json"),
        r#"{"title": "A11y Basics", "tags": "accessibility, WCAG"}"#,
    )
    .unwrap();
    fs::write(
        meta.join("types.meta.json"),
        r#"{"title": "Typography", "tags": "accessibility, design"}"#,
    )
    .unwrap();

    let cfg = ssg::cmd::SsgConfig::builder()
        .site_name("Fixture".to_string())
        .base_url("https://fixture.test".to_string())
        .build()
        .expect("config");
    let ctx =
        PluginContext::with_config(tmp.path(), &build, &site, tmp.path(), cfg);
    (tmp, ctx)
}

#[test]
fn per_tag_landing_pages_are_generated_from_string_tags() {
    // #586 port 5: /tags/<slug>/index.html per term, hub included.
    let (_tmp, ctx) = fixture_site();
    TaxonomyPlugin.after_compile(&ctx).unwrap();

    let site = &ctx.site_dir;
    assert!(site.join("tags/index.html").exists(), "hub page");
    assert!(site.join("tags/accessibility/index.html").exists());
    assert!(site.join("tags/wcag/index.html").exists(), "slugified WCAG");
    assert!(site.join("tags/design/index.html").exists());

    let a11y =
        fs::read_to_string(site.join("tags/accessibility/index.html")).unwrap();
    assert!(a11y.contains("A11y Basics"));
    assert!(a11y.contains("Typography"));
    let design =
        fs::read_to_string(site.join("tags/design/index.html")).unwrap();
    assert!(design.contains("Typography"));
    assert!(!design.contains("A11y Basics"));
}

#[test]
fn landing_pages_inline_essential_head_elements() {
    // Pages generated in after_compile bypass the fused transform
    // chain (canonical/JSON-LD/a11y transform plugins never see
    // them), so DOCTYPE, lang, and canonical must be inlined.
    let (_tmp, ctx) = fixture_site();
    TaxonomyPlugin.after_compile(&ctx).unwrap();

    let html =
        fs::read_to_string(ctx.site_dir.join("tags/accessibility/index.html"))
            .unwrap();
    assert!(html.contains("<!DOCTYPE html>"), "{html}");
    assert!(html.contains("<html lang="), "{html}");
    assert!(
        html.contains(
            "<link rel=\"canonical\" href=\"https://fixture.test/tags/accessibility/\""
        ),
        "{html}"
    );
}

#[test]
fn rebuild_is_byte_deterministic() {
    let (_tmp, ctx) = fixture_site();
    TaxonomyPlugin.after_compile(&ctx).unwrap();
    let first =
        fs::read_to_string(ctx.site_dir.join("tags/index.html")).unwrap();
    TaxonomyPlugin.after_compile(&ctx).unwrap();
    let second =
        fs::read_to_string(ctx.site_dir.join("tags/index.html")).unwrap();
    assert_eq!(first, second);
}
