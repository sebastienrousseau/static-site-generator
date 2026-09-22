// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![allow(missing_docs)]
#![cfg(feature = "test-fault-injection")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Fault-injection integration tests.
//!
//! These tests use the [`fail`](https://docs.rs/fail) crate to
//! activate failpoints sprinkled in front of `fs::write` /
//! `fs::create_dir_all` call sites in the library, and assert that
//! every error path is correctly propagated as an `anyhow::Error`
//! with the right context.
//!
//! Failpoints are **process-global state**, so this entire test
//! suite lives in its own integration test binary (separate from
//! the lib test binary). That isolation is what lets the regular
//! lib tests in `src/scaffold.rs` continue to run with the
//! `test-fault-injection` feature enabled — they live in a
//! different process and never see the activated failpoints.
//!
//! Run with:
//!
//! ```sh
//! cargo test --features test-fault-injection --test fault_injection
//! ```
//!
//! Each test serializes its activate → run → deactivate sequence
//! via [`serial_test::serial`] so concurrent tests in this binary
//! don't fight over the same global failpoint state. The teardown
//! is performed in a `Drop` guard so a panicking assertion still
//! cleans up.

// `DeployPlugin` / `SbomPlugin` are deprecated but still shipped for one
// release, and these tests are what keep them working until removal.
#![allow(deprecated)]

use serial_test::serial;
use ssg::cache::BuildCache;
use ssg::cmd::SsgConfig;
use ssg::markdown_ext::MarkdownExtPlugin;
use ssg::plugin::{Plugin, PluginContext};
use ssg::plugins::MinifyPlugin;
use ssg::scaffold::scaffold_project_at;
use std::fs;
use tempfile::tempdir;

/// RAII guard that disables a failpoint on drop.
struct FailGuard<'a>(&'a str);

impl Drop for FailGuard<'_> {
    fn drop(&mut self) {
        let _ = fail::cfg(self.0, "off");
    }
}

/// Activates `name`, runs `scaffold_project_at` against a fresh
/// tempdir, deactivates the failpoint, and returns the resulting
/// error so the caller can assert against its message.
fn run_scaffold_with_failpoint(name: &str) -> anyhow::Error {
    let _guard = FailGuard(name);
    fail::cfg(name, "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    scaffold_project_at("fault-test-site", dir.path())
        .expect_err("scaffold should fail when failpoint is active")
}

#[test]
#[serial]
fn scaffold_fault_create_dir_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::create-dir");
    assert!(format!("{err:?}").contains("scaffold::create-dir"));
}

#[test]
#[serial]
fn scaffold_fault_write_config_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-config");
    assert!(format!("{err:?}").contains("scaffold::write-config"));
}

#[test]
#[serial]
fn scaffold_fault_write_index_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-index");
    assert!(format!("{err:?}").contains("scaffold::write-index"));
}

#[test]
#[serial]
fn scaffold_fault_write_about_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-about");
    assert!(format!("{err:?}").contains("scaffold::write-about"));
}

#[test]
#[serial]
fn scaffold_fault_write_post_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-post");
    assert!(format!("{err:?}").contains("scaffold::write-post"));
}

#[test]
#[serial]
fn scaffold_fault_write_base_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-base");
    assert!(format!("{err:?}").contains("scaffold::write-base"));
}

#[test]
#[serial]
fn scaffold_fault_write_page_tpl_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-page-tpl");
    assert!(format!("{err:?}").contains("scaffold::write-page-tpl"));
}

#[test]
#[serial]
fn scaffold_fault_write_post_tpl_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-post-tpl");
    assert!(format!("{err:?}").contains("scaffold::write-post-tpl"));
}

#[test]
#[serial]
fn scaffold_fault_write_index_tpl_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-index-tpl");
    assert!(format!("{err:?}").contains("scaffold::write-index-tpl"));
}

#[test]
#[serial]
fn scaffold_fault_write_css_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-css");
    assert!(format!("{err:?}").contains("scaffold::write-css"));
}

#[test]
#[serial]
fn scaffold_fault_write_nav_returns_err() {
    let err = run_scaffold_with_failpoint("scaffold::write-nav");
    assert!(format!("{err:?}").contains("scaffold::write-nav"));
}

// =====================================================================
// cmd::validate_path_safety
// =====================================================================

#[test]
#[serial]
fn cmd_fault_symlink_metadata_returns_err() {
    // Activate the failpoint that sits in front of fs::symlink_metadata
    // inside validate_path_safety. We need an existing directory so
    // the `path.exists()` branch is taken.
    let _guard = FailGuard("cmd::symlink-metadata");
    fail::cfg("cmd::symlink-metadata", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let mut config = SsgConfig::default();
    config.content_dir = dir.path().to_path_buf();
    config.output_dir = dir.path().to_path_buf();
    config.template_dir = dir.path().to_path_buf();

    let err = config.validate().expect_err("validate should fail");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("injected: cmd::symlink-metadata"),
        "expected injected error, got: {msg}"
    );
}

// =====================================================================
// cache::BuildCache load + save
// =====================================================================

#[test]
#[serial]
fn cache_fault_read_returns_err() {
    let _guard = FailGuard("cache::read");
    fail::cfg("cache::read", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let cache_path = dir.path().join("cache.json");
    fs::write(&cache_path, "{}").expect("seed cache file");

    let err = BuildCache::load(&cache_path)
        .expect_err("load should fail when cache::read failpoint is active");
    assert!(format!("{err:?}").contains("injected: cache::read"));
}

#[test]
#[serial]
fn cache_fault_parse_returns_err() {
    let _guard = FailGuard("cache::parse");
    fail::cfg("cache::parse", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let cache_path = dir.path().join("cache.json");
    fs::write(&cache_path, r#"{"fingerprints":{}}"#).expect("seed");

    let err = BuildCache::load(&cache_path)
        .expect_err("load should fail when cache::parse failpoint is active");
    assert!(format!("{err:?}").contains("injected: cache::parse"));
}

#[test]
#[serial]
fn cache_fault_write_returns_err() {
    let _guard = FailGuard("cache::write");
    fail::cfg("cache::write", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let cache_path = dir.path().join("cache.json");
    let cache = BuildCache::new(&cache_path);

    let err = cache
        .save()
        .expect_err("save should fail when cache::write failpoint is active");
    assert!(format!("{err:?}").contains("injected: cache::write"));
}

// =====================================================================
// plugins::MinifyPlugin read + write
// =====================================================================

#[test]
#[serial]
fn plugins_fault_minify_read_returns_err() {
    let _guard = FailGuard("plugins::minify-read");
    fail::cfg("plugins::minify-read", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let site = dir.path().to_path_buf();
    fs::write(site.join("index.html"), "<p>x</p>").expect("seed html");

    let ctx = PluginContext::new(&site, &site, &site, &site);
    let err = MinifyPlugin
        .after_compile(&ctx)
        .expect_err("after_compile should fail when read failpoint is active");
    assert!(format!("{err:?}").contains("injected: plugins::minify-read"));
}

#[test]
#[serial]
fn plugins_fault_minify_write_returns_err() {
    let _guard = FailGuard("plugins::minify-write");
    fail::cfg("plugins::minify-write", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let site = dir.path().to_path_buf();
    fs::write(site.join("index.html"), "<p>x</p>").expect("seed html");

    let ctx = PluginContext::new(&site, &site, &site, &site);
    let err = MinifyPlugin
        .after_compile(&ctx)
        .expect_err("after_compile should fail when write failpoint is active");
    assert!(format!("{err:?}").contains("injected: plugins::minify-write"));
}

// =====================================================================
// plugins::markdown_ext read + write
// =====================================================================

#[test]
#[serial]
fn markdown_ext_fault_read_returns_err() {
    let _guard = FailGuard("markdown_ext::read");
    fail::cfg("markdown_ext::read", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let site = dir.path().to_path_buf();
    fs::write(site.join("post.md"), "# Hi").expect("seed md");

    let ctx = PluginContext::new(&site, &site, &site, &site);
    let err = MarkdownExtPlugin.before_compile(&ctx).expect_err(
        "before_compile should fail when markdown_ext::read is active",
    );
    assert!(format!("{err:?}").contains("injected: markdown_ext::read"));
}

// markdown_ext::write failpoint sits behind an `if new != raw` gate
// (src/plugins/markdown_ext.rs:88) that is non-trivial to trigger
// from a minimal test fixture — apply_strikethrough emits identical
// output for "hello ~~world~~" because the transformation is a
// pure-text substitution. Covering this branch organically is part
// of the v0.0.41 coverage push.

// =====================================================================
// io_pool::write (issue #569 phase 1 — writer-thread pool)
// =====================================================================

#[test]
#[serial]
fn io_pool_fault_write_surfaces_at_flush() {
    let _guard = FailGuard("io_pool::write");
    fail::cfg("io_pool::write", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let pool = ssg::io_pool::IoPool::with_threads(2);
    pool.write(dir.path().join("x.html"), b"x".to_vec())
        .expect("enqueue succeeds; failure surfaces at flush");
    let err = pool
        .flush()
        .expect_err("flush must surface the injected write failure");
    assert!(format!("{err:?}").contains("injected: io_pool::write"));
}

// =====================================================================
// image_plugin::encode_avif (AVIF failures are non-fatal: logged and
// the variant is skipped — issue: per-variant resilience)
// =====================================================================

#[test]
#[serial]
#[cfg(feature = "image-optimization")]
fn image_fault_encode_avif_skips_variant_but_keeps_webp() {
    use ssg::image_plugin::ImageOptimizationPlugin;

    let _guard = FailGuard("image::encode-avif");
    fail::cfg("image::encode-avif", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let site = dir.path().join("site");
    fs::create_dir_all(&site).expect("mkdir site");
    // Wide enough for one 320w responsive variant.
    let buf = image::ImageBuffer::from_fn(400, 20, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    image::DynamicImage::ImageRgb8(buf)
        .save_with_format(site.join("photo.jpg"), image::ImageFormat::Jpeg)
        .expect("write jpeg");

    let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
    let plugin = ImageOptimizationPlugin {
        breakpoints: vec![320],
        ..Default::default()
    };
    plugin
        .after_compile(&ctx)
        .expect("AVIF encode failure must be non-fatal");

    assert!(
        site.join("optimized/photo-320w.webp").exists(),
        "WebP variant should still be produced"
    );
    assert!(
        !site.join("optimized/photo-320w.avif").exists(),
        "AVIF variant should be skipped when the encoder fails"
    );
}

// =====================================================================
// assets::remove-original (fingerprint_file: minified asset written,
// removal of the original fails)
// =====================================================================

#[test]
#[serial]
fn assets_fault_remove_original_returns_err() {
    use ssg::assets::FingerprintPlugin;

    let _guard = FailGuard("assets::remove-original");
    fail::cfg("assets::remove-original", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let site = dir.path().join("site");
    fs::create_dir_all(&site).expect("mkdir site");
    fs::write(site.join("app.js"), "const x = 1;").expect("seed js");

    let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
    let err = FingerprintPlugin.after_compile(&ctx).expect_err(
        "after_compile should fail when assets::remove-original is active",
    );
    assert!(format!("{err:?}").contains("injected: assets::remove-original"));
}

// =====================================================================
// audit::json-format (AuditReport::print_json: serialisation failure
// propagated through the `?` in print_json)
// =====================================================================

#[test]
#[serial]
fn audit_print_json_fault_returns_err() {
    use ssg::audit::AuditReport;

    let _guard = FailGuard("audit::json-format");
    fail::cfg("audit::json-format", "return").expect("activate failpoint");

    let report = AuditReport { gates: vec![] };
    let err = report
        .print_json()
        .expect_err("print_json should fail when audit::json-format is active");
    assert!(
        format!("{err:?}").contains("audit::json-format"),
        "unexpected error: {err:?}"
    );
}

// =====================================================================
// postprocess serialisation failpoints (JSON/SBOM/manifest emitters
// serialise `serde_json::Value`s that cannot fail organically — the
// failpoints make the error-mapping closures and `?` branches
// reachable)
// =====================================================================

#[test]
#[serial]
fn postprocess_fault_ai_plugin_serialize_returns_err() {
    use ssg::postprocess::agentic_discovery::write_ai_plugin_json;

    let _guard = FailGuard("postprocess::ai-plugin-serialize");
    fail::cfg("postprocess::ai-plugin-serialize", "return")
        .expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let cfg = SsgConfig::builder()
        .site_name("Example".into())
        .base_url("https://example.com".into())
        .build()
        .expect("config");
    let ctx =
        PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
    let err = write_ai_plugin_json(&ctx, &cfg)
        .expect_err("serialisation failpoint must propagate");
    assert!(format!("{err:?}")
        .contains("injected: postprocess::ai-plugin-serialize"));
}

#[test]
#[serial]
fn postprocess_fault_mcp_serialize_returns_err() {
    use ssg::postprocess::agentic_discovery::{
        write_mcp_registry, AgentsConfig,
    };

    let _guard = FailGuard("postprocess::mcp-serialize");
    fail::cfg("postprocess::mcp-serialize", "return")
        .expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let cfg = SsgConfig::builder()
        .site_name("Example".into())
        .base_url("https://example.com".into())
        .build()
        .expect("config");
    let ctx =
        PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
    let err = write_mcp_registry(&ctx, &cfg, &AgentsConfig::default())
        .expect_err("serialisation failpoint must propagate");
    assert!(format!("{err:?}").contains("injected: postprocess::mcp-serialize"));
}

#[test]
#[serial]
fn postprocess_fault_manifest_serialize_returns_err() {
    use ssg::postprocess::ManifestFixPlugin;

    let _guard = FailGuard("postprocess::manifest-serialize");
    fail::cfg("postprocess::manifest-serialize", "return")
        .expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    fs::write(
        dir.path().join("manifest.json"),
        r#"{"name":"X","description":"Done."}"#,
    )
    .expect("seed manifest");
    let ctx =
        PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
    let err = ManifestFixPlugin
        .after_compile(&ctx)
        .expect_err("serialisation failpoint must propagate");
    assert!(format!("{err:?}")
        .contains("injected: postprocess::manifest-serialize"));
}

#[test]
#[serial]
fn postprocess_fault_json_feed_serialize_falls_back_to_compact() {
    use ssg::postprocess::JsonFeedPlugin;

    let _guard = FailGuard("postprocess::json-feed-serialize");
    fail::cfg("postprocess::json-feed-serialize", "return")
        .expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let page = dir.path().join("post");
    fs::create_dir_all(&page).expect("mkdir page");
    fs::write(
        page.join("page.meta.json"),
        r#"{"title":"Post","item_pub_date":"Thu, 11 Apr 2026 06:06:06 +0000"}"#,
    )
    .expect("seed sidecar");
    let ctx =
        PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
    JsonFeedPlugin
        .after_compile(&ctx)
        .expect("pretty-print failure falls back to compact encoding");

    let feed = fs::read_to_string(dir.path().join("feed.json"))
        .expect("feed.json written");
    assert!(feed.contains("\"title\":\"Post\""));
    assert!(
        !feed.contains('\n'),
        "compact fallback encoding has no newlines"
    );
}

#[test]
#[serial]
fn postprocess_fault_vercel_render_returns_err() {
    use ssg::cmd::EdgeHeadersConfig;
    use ssg::postprocess::EdgeHeadersPlugin;

    let _guard = FailGuard("postprocess::vercel-render");
    fail::cfg("postprocess::vercel-render", "return")
        .expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let site = dir.path().join("site");
    fs::create_dir_all(&site).expect("mkdir site");
    let mut edge = EdgeHeadersConfig::default();
    edge.targets = vec!["vercel".to_string()];
    let cfg = SsgConfig::builder()
        .site_name("Example".into())
        .base_url("https://example.com".into())
        .edge_headers(edge)
        .build()
        .expect("config");
    let ctx = PluginContext::with_config(
        dir.path(),
        dir.path(),
        &site,
        dir.path(),
        cfg,
    );
    let err = EdgeHeadersPlugin
        .after_compile(&ctx)
        .expect_err("vercel render failpoint must propagate");
    assert!(format!("{err:?}").contains("injected: postprocess::vercel-render"));
}

#[test]
#[serial]
fn postprocess_fault_sidecar_entry_degrades_to_empty_metadata() {
    use ssg::postprocess::RssAggregatePlugin;

    let _guard = FailGuard("postprocess::sidecar-entry");
    fail::cfg("postprocess::sidecar-entry", "return")
        .expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let original = r#"<rss version="2.0"><channel><title>T</title><link>x</link><description>D</description><item><title>Solo</title></item></channel></rss>"#;
    fs::write(dir.path().join("rss.xml"), original).expect("seed rss");
    let page = dir.path().join("post");
    fs::create_dir_all(&page).expect("mkdir page");
    fs::write(page.join("page.meta.json"), r#"{"title":"Post"}"#)
        .expect("seed sidecar");

    let ctx =
        PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
    RssAggregatePlugin
        .after_compile(&ctx)
        .expect("sidecar read failure degrades to a no-op");

    let after = fs::read_to_string(dir.path().join("rss.xml"))
        .expect("rss.xml still present");
    assert_eq!(
        after, original,
        "with sidecar reads failing, the feed must be left untouched"
    );
}

// =====================================================================
// lib.rs — create_directories / Paths::validate seams (issue: 100%
// region coverage; these error branches are unreachable without
// injection because `is_safe_path` only fails when an existing path
// fails `canonicalize`, and `symlink_metadata` cannot fail once
// `exists()` returned true)
// =====================================================================

/// Builds a `Paths` value rooted in `base` with all four directories
/// pre-created, so `create_directories` reaches every `is_safe_path`
/// call.
fn paths_under(base: &std::path::Path) -> ssg::Paths {
    let paths = ssg::Paths {
        site: base.join("public"),
        content: base.join("content"),
        build: base.join("build"),
        template: base.join("templates"),
    };
    for dir in [&paths.site, &paths.content, &paths.build, &paths.template] {
        fs::create_dir_all(dir).expect("pre-create dir");
    }
    paths
}

/// Drives `create_directories` with the `lib::is-safe-path` failpoint
/// configured to pass through `skip` calls and then fail, so each of
/// the four `?` call sites (content, build, site, template) can be
/// exercised individually.
fn create_directories_with_nth_path_failing(skip: usize) {
    let _guard = FailGuard("lib::is-safe-path");
    let cfg = if skip == 0 {
        "1*return->off".to_string()
    } else {
        format!("{skip}*off->1*return->off")
    };
    fail::cfg("lib::is-safe-path", &cfg).expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let paths = paths_under(dir.path());
    let err = ssg::create_directories(&paths)
        .expect_err("create_directories must propagate injected failure");
    assert!(format!("{err:?}").contains("injected: lib::is-safe-path"));
}

#[test]
#[serial]
fn lib_fault_is_safe_path_content_returns_err() {
    create_directories_with_nth_path_failing(0);
}

#[test]
#[serial]
fn lib_fault_is_safe_path_build_returns_err() {
    create_directories_with_nth_path_failing(1);
}

#[test]
#[serial]
fn lib_fault_is_safe_path_site_returns_err() {
    create_directories_with_nth_path_failing(2);
}

#[test]
#[serial]
fn lib_fault_is_safe_path_template_returns_err() {
    create_directories_with_nth_path_failing(3);
}

#[test]
#[serial]
fn lib_fault_symlink_metadata_in_paths_validate_returns_err() {
    let _guard = FailGuard("lib::symlink-metadata");
    fail::cfg("lib::symlink-metadata", "return").expect("activate failpoint");

    let dir = tempdir().expect("tempdir");
    let paths = paths_under(dir.path());
    let err = paths
        .validate()
        .expect_err("validate must propagate injected metadata failure");
    assert!(format!("{err:?}").contains("injected: lib::symlink-metadata"));
}
