// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Edge adapter contract test (issue #546 AC4 + AC5 + AC6 + AC7 + AC8).
//!
//! Drives the same key set the Cloudflare Worker and Vercel Edge
//! Function consume:
//!
//! - load the manifest written by `IsrManifestPlugin`,
//! - fetch raw markdown + layout via a `ContentProvider`,
//! - render through `ssg_wasm::render_page_isr_impl`,
//! - assert `Cache-Control` value matches per-route `isr.*` override,
//! - assert `urls_for_source(...)` returns the expected URL set for
//!   webhook-driven invalidation.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;

use ssg::isr_manifest::{
    IsrManifestPlugin, CONTENT_RELATIVE_DIR, MANIFEST_RELATIVE_PATH,
};
use ssg::plugin::{Plugin, PluginContext};
use ssg_core::{ContentProvider, FsContentProvider, Manifest};
use ssg_wasm::render_page_isr_impl;

fn make_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("content/posts")).unwrap();
    fs::create_dir_all(root.join("templates")).unwrap();
    fs::create_dir_all(root.join("public")).unwrap();

    fs::write(
        root.join("content/index.md"),
        "---\ntitle: Home\n---\n# Welcome\n",
    )
    .unwrap();
    fs::write(
        root.join("content/posts/alpha.md"),
        "---\ntitle: Alpha\nisr:\n  s_maxage: 600\n  swr: 3600\n---\n# Alpha body",
    )
    .unwrap();
    fs::write(
        root.join("templates/index.html"),
        "<html><head><title>{{ title }}</title></head><body>{{ content }}</body></html>",
    )
    .unwrap();
    fs::write(
        root.join("templates/page.html"),
        "<html><body class=\"page\">{{ content }}</body></html>",
    )
    .unwrap();

    let ctx = PluginContext {
        content_dir: root.join("content"),
        build_dir: root.join("public"),
        site_dir: root.join("public"),
        template_dir: root.join("templates"),
        config: None,
        cache: None,
        memory_budget: None,
        html_files: None,
        dep_graph: None,
        dry_run: false,
    };
    IsrManifestPlugin.after_compile(&ctx).unwrap();
    tmp
}

#[test]
fn ac6_default_swr_cache_control() {
    let tmp = make_fixture();
    let site = tmp.path().join("public");
    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(site.join(MANIFEST_RELATIVE_PATH)).unwrap(),
    )
    .unwrap();

    // Index has no isr.* override → use default.
    let entry = manifest.get("/index.html").unwrap();
    assert!(entry.cache.is_none(), "no override = entry.cache is None");
    let cc = manifest.default_cache.to_cache_control();
    assert_eq!(cc, "s-maxage=60, stale-while-revalidate=86400");
}

#[test]
fn ac7_per_route_frontmatter_cache_control() {
    let tmp = make_fixture();
    let site = tmp.path().join("public");
    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(site.join(MANIFEST_RELATIVE_PATH)).unwrap(),
    )
    .unwrap();

    // /posts/alpha had isr.s_maxage=600 + isr.swr=3600 in frontmatter.
    let entry = manifest.get("/posts/alpha/index.html").unwrap();
    let cache = entry.cache.as_ref().expect("per-route cache override");
    assert_eq!(cache.s_maxage, 600);
    assert_eq!(cache.swr, 3600);
    assert_eq!(
        cache.to_cache_control(),
        "s-maxage=600, stale-while-revalidate=3600"
    );
}

#[test]
fn ac4_ac5_end_to_end_render_from_kv_payload() {
    // Mirrors what the Worker / Edge Function does: load manifest,
    // fetch md + layout from the staged dist/.ssg/content/ tree
    // (which is what gets uploaded to KV / Edge Config), then call
    // render_page_isr_impl. Asserts the rendered HTML contains the
    // expected slots.
    let tmp = make_fixture();
    let site = tmp.path().join("public");
    let content_root = site.join(CONTENT_RELATIVE_DIR);
    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(site.join(MANIFEST_RELATIVE_PATH)).unwrap(),
    )
    .unwrap();

    // ContentProvider rooted at the staged tree — same surface the
    // KV-backed and Edge-Config-backed adapters present.
    let provider = FsContentProvider::new(&content_root);

    let entry = manifest.get("/posts/alpha/index.html").unwrap();
    let md_key = entry
        .sources
        .iter()
        .find(|s| s.starts_with("content/"))
        .unwrap();
    let tpl_key = entry
        .sources
        .iter()
        .find(|s| s.starts_with("templates/") && s.ends_with("index.html"))
        .unwrap();

    let md = provider.fetch_string(md_key).unwrap();
    let layout = provider.fetch_string(tpl_key).unwrap();

    let html = render_page_isr_impl(
        &md,
        &layout,
        "{\"url\": \"/posts/alpha/index.html\", \"site_name\": \"Demo\"}",
    )
    .unwrap();

    assert!(html.contains("<title>Alpha</title>"));
    assert!(html.contains("<h1>Alpha body</h1>"));
}

#[test]
fn ac8_webhook_finds_affected_urls() {
    let tmp = make_fixture();
    let site = tmp.path().join("public");
    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(site.join(MANIFEST_RELATIVE_PATH)).unwrap(),
    )
    .unwrap();

    // Editing alpha.md must invalidate exactly its own URL (no tags
    // / archives in this minimal fixture).
    let affected = manifest.urls_for_source("content/posts/alpha.md");
    assert_eq!(affected, vec!["/posts/alpha/index.html".to_string()]);

    // Editing the shared template invalidates every page (both posts
    // and index list it as a dep).
    let mut t = manifest.urls_for_source("templates/index.html");
    t.sort();
    assert_eq!(
        t,
        vec![
            "/index.html".to_string(),
            "/posts/alpha/index.html".to_string()
        ]
    );

    // Editing an unknown source invalidates nothing.
    let none = manifest.urls_for_source("content/posts/ghost.md");
    assert!(none.is_empty(), "{:?}", none);
}
