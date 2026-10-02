#![allow(clippy::unwrap_used, clippy::expect_used)]
// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Comprehensive regression test suite for the SSG library.
//!
//! Covers every public module and function through integration-level
//! tests that exercise the full API surface end-to-end.

// `DeployPlugin` / `SbomPlugin` are deprecated but still shipped for one
// release, and these tests are what keep them working until removal.
#![allow(deprecated)]

use anyhow::Result;
use ssg::cache::BuildCache;
use ssg::cmd::{
    Cli, CliError, LanguageCode, SsgConfig, DEFAULT_HOST, DEFAULT_PORT,
    DEFAULT_SITE_NAME, DEFAULT_SITE_TITLE, MAX_CONFIG_SIZE, RESERVED_NAMES,
};
use ssg::plugin::{Plugin, PluginContext, PluginManager};
use ssg::plugins::{DeployPlugin, ImageOptiPlugin, MinifyPlugin};
use ssg::schema::{generate_schema, write_schema};
use ssg::stream::{
    process_batch, stream_copy, stream_hash, stream_lines, BatchResult,
    MAX_BATCH_SIZE, STREAM_BUFFER_SIZE,
};
use ssg::watch::{FileWatcher, WatchConfig, MAX_WATCH_ITERATIONS};
use ssg::{
    collect_files_recursive, copy_dir_all, copy_dir_with_progress,
    create_directories, create_log_file, is_safe_path, log_arguments,
    log_initialization, verify_and_copy_files, verify_file_safety, Paths,
    SsgError, MAX_DIR_DEPTH,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::tempdir;

// =====================================================================
// Module: lib.rs — Paths and PathsBuilder
// =====================================================================

#[test]
fn paths_default_values() {
    let p = Paths::default_paths();
    assert_eq!(p.site, PathBuf::from("public"));
    assert_eq!(p.content, PathBuf::from("content"));
    assert_eq!(p.build, PathBuf::from("build"));
    assert_eq!(p.template, PathBuf::from("templates"));
}

#[test]
fn paths_builder_defaults() -> Result<()> {
    let p = Paths::builder().build()?;
    assert_eq!(p.site, PathBuf::from("public"));
    Ok(())
}

#[test]
fn paths_builder_custom() -> Result<()> {
    let tmp = tempdir()?;
    let p = Paths::builder()
        .site(tmp.path().join("s"))
        .content(tmp.path().join("c"))
        .build_dir(tmp.path().join("b"))
        .template(tmp.path().join("t"))
        .build()?;
    assert_eq!(p.site, tmp.path().join("s"));
    assert_eq!(p.template, tmp.path().join("t"));
    Ok(())
}

#[test]
fn paths_builder_relative_to() -> Result<()> {
    let tmp = tempdir()?;
    fs::create_dir_all(tmp.path().join("public"))?;
    fs::create_dir_all(tmp.path().join("content"))?;
    fs::create_dir_all(tmp.path().join("build"))?;
    fs::create_dir_all(tmp.path().join("templates"))?;
    let p = Paths::builder().relative_to(tmp.path()).build()?;
    assert_eq!(p.site, tmp.path().join("public"));
    Ok(())
}

#[test]
fn paths_validate_rejects_traversal() {
    let err = Paths::builder().site("../escape").build().unwrap_err();
    assert!(matches!(err, SsgError::PathTraversal { .. }));
}

#[test]
fn paths_validate_rejects_double_slash() {
    let err = Paths::builder().site("bad//path").build().unwrap_err();
    assert!(matches!(err, SsgError::Validation { .. }));
}

// =====================================================================
// Module: lib.rs — File operations
// =====================================================================

#[test]
fn is_safe_path_accepts_valid() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path().join("safe");
    fs::create_dir(&dir)?;
    assert!(is_safe_path(&dir.canonicalize()?)?);
    Ok(())
}

#[test]
fn is_safe_path_rejects_traversal() -> Result<()> {
    assert!(!is_safe_path(Path::new("../../etc"))?);
    Ok(())
}

#[test]
fn is_safe_path_nonexistent_safe() -> Result<()> {
    assert!(is_safe_path(Path::new("nonexistent_dir"))?);
    Ok(())
}

#[test]
fn verify_file_safety_accepts_regular() -> Result<()> {
    let tmp = tempdir()?;
    let f = tmp.path().join("ok.txt");
    fs::write(&f, "safe")?;
    verify_file_safety(&f)?;
    Ok(())
}

#[test]
fn verify_file_safety_rejects_oversized() -> Result<()> {
    let tmp = tempdir()?;
    let f = tmp.path().join("big.bin");
    let file = fs::File::create(&f)?;
    file.set_len(11 * 1024 * 1024)?; // 11 MB
    assert!(verify_file_safety(&f).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn verify_file_safety_rejects_symlink() -> Result<()> {
    let tmp = tempdir()?;
    let target = tmp.path().join("target.txt");
    let link = tmp.path().join("link.txt");
    fs::write(&target, "data")?;
    std::os::unix::fs::symlink(&target, &link)?;
    assert!(verify_file_safety(&link).is_err());
    Ok(())
}

#[test]
fn verify_and_copy_files_copies_tree() -> Result<()> {
    let tmp = tempdir()?;
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(src.join("sub"))?;
    fs::write(src.join("a.txt"), "a")?;
    fs::write(src.join("sub/b.txt"), "b")?;

    verify_and_copy_files(&src, &dst)?;
    assert_eq!(fs::read_to_string(dst.join("a.txt"))?, "a");
    assert_eq!(fs::read_to_string(dst.join("sub/b.txt"))?, "b");
    Ok(())
}

#[test]
fn verify_and_copy_files_rejects_nonexistent() {
    let result = verify_and_copy_files(
        Path::new("/nonexistent_src"),
        Path::new("/tmp/dst"),
    );
    assert!(result.is_err());
}

#[test]
fn verify_and_copy_files_async_copies() -> Result<()> {
    let tmp = tempdir()?;
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(&src)?;
    fs::write(src.join("x.txt"), "x")?;

    ssg::verify_and_copy_files_async(&src, &dst)?;
    assert_eq!(fs::read_to_string(dst.join("x.txt"))?, "x");
    Ok(())
}

#[test]
fn copy_dir_all_copies_nested() -> Result<()> {
    let tmp = tempdir()?;
    let src = tmp.path().join("s");
    let dst = tmp.path().join("d");
    fs::create_dir_all(src.join("a/b"))?;
    fs::write(src.join("a/b/c.txt"), "deep")?;

    copy_dir_all(&src, &dst)?;
    assert_eq!(fs::read_to_string(dst.join("a/b/c.txt"))?, "deep");
    Ok(())
}

#[test]
fn copy_dir_with_progress_works() -> Result<()> {
    let tmp = tempdir()?;
    let src = tmp.path().join("s");
    let dst = tmp.path().join("d");
    fs::create_dir_all(&src)?;
    fs::write(src.join("f.txt"), "data")?;

    copy_dir_with_progress(&src, &dst)?;
    assert!(dst.join("f.txt").exists());
    Ok(())
}

#[test]
fn collect_files_recursive_finds_all() -> Result<()> {
    let tmp = tempdir()?;
    fs::create_dir_all(tmp.path().join("sub"))?;
    fs::write(tmp.path().join("a.txt"), "")?;
    fs::write(tmp.path().join("sub/b.txt"), "")?;

    let mut files = Vec::new();
    collect_files_recursive(tmp.path(), &mut files)?;
    assert_eq!(files.len(), 2);
    Ok(())
}

#[test]
fn create_directories_creates_all() -> Result<()> {
    let tmp = tempdir()?;
    let paths = Paths {
        site: tmp.path().join("site"),
        content: tmp.path().join("content"),
        build: tmp.path().join("build"),
        template: tmp.path().join("template"),
    };
    create_directories(&paths)?;
    assert!(paths.site.exists());
    assert!(paths.content.exists());
    assert!(paths.build.exists());
    assert!(paths.template.exists());
    Ok(())
}

// =====================================================================
// Module: lib.rs — Logging
// =====================================================================

#[test]
fn create_log_file_creates() -> Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("test.log");
    let f = create_log_file(path.to_str().unwrap())?;
    assert!(f.metadata()?.is_file());
    Ok(())
}

#[test]
fn log_initialization_writes() -> Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("init.log");
    let mut f = fs::File::create(&path)?;
    let date = ssg::now_iso();
    log_initialization(&mut f, &date)?;
    let content = fs::read_to_string(&path)?;
    assert!(content.contains("INFO"));
    Ok(())
}

#[test]
fn log_arguments_writes() -> Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("args.log");
    let mut f = fs::File::create(&path)?;
    let date = ssg::now_iso();
    log_arguments(&mut f, &date)?;
    let content = fs::read_to_string(&path)?;
    assert!(content.contains("process"));
    Ok(())
}

// =====================================================================
// Module: lib.rs — Constants
// =====================================================================

#[test]
fn max_dir_depth_is_128() {
    assert_eq!(MAX_DIR_DEPTH, 128);
}

// =====================================================================
// Module: cmd.rs — Configuration
// =====================================================================

#[test]
fn ssg_config_default_has_site_name() {
    let config = SsgConfig::default();
    assert_eq!(config.site_name, DEFAULT_SITE_NAME);
    assert_eq!(config.site_title, DEFAULT_SITE_TITLE);
}

#[test]
fn ssg_config_builder_validates() {
    let result = SsgConfig::builder().site_name(String::new()).build();
    assert!(matches!(result, Err(CliError::ValidationError(_))));
}

#[test]
fn ssg_config_builder_builds_valid() -> Result<(), CliError> {
    let config = SsgConfig::builder()
        .site_name("test".to_string())
        .base_url("http://example.com".to_string())
        .build()?;
    assert_eq!(config.site_name, "test");
    Ok(())
}

#[test]
fn ssg_config_from_toml_string() {
    let toml = r#"
site_name = "parsed"
content_dir = "content"
output_dir = "public"
template_dir = "templates"
base_url = "http://example.com"
site_title = "Parsed"
site_description = "From TOML"
language = "en-GB"
"#;
    let config: SsgConfig = toml.parse().unwrap();
    assert_eq!(config.site_name, "parsed");
}

#[test]
fn ssg_config_from_file() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
site_name = "file"
content_dir = "c"
output_dir = "o"
template_dir = "t"
base_url = "http://example.com"
site_title = "File"
site_description = "From file"
language = "en-GB"
"#,
    )?;
    let config = SsgConfig::from_file(&path)?;
    assert_eq!(config.site_name, "file");
    Ok(())
}

#[test]
fn ssg_config_rejects_oversized_file() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("big.toml");
    fs::write(&path, "x".repeat(MAX_CONFIG_SIZE + 1)).unwrap();
    assert!(matches!(
        SsgConfig::from_file(&path),
        Err(CliError::ValidationError(_))
    ));
}

// =====================================================================
// Module: cmd.rs — URL and path validation
// =====================================================================

#[test]
fn validate_url_accepts_http() {
    assert!(ssg::cmd::validate_url("http://example.com").is_ok());
}

#[test]
fn validate_url_accepts_https() {
    assert!(ssg::cmd::validate_url("https://example.com").is_ok());
}

#[test]
fn validate_url_rejects_javascript() {
    assert!(ssg::cmd::validate_url("javascript:alert(1)").is_err());
}

#[test]
fn validate_url_rejects_ftp() {
    assert!(ssg::cmd::validate_url("ftp://example.com").is_err());
}

#[test]
fn validate_url_rejects_angle_brackets() {
    assert!(ssg::cmd::validate_url("http://a.com<script>").is_err());
}

// =====================================================================
// Module: cmd.rs — Language codes
// =====================================================================

#[test]
fn language_code_valid() {
    assert!(LanguageCode::new("en-GB").is_ok());
    assert!(LanguageCode::new("fr-FR").is_ok());
}

#[test]
fn language_code_invalid_format() {
    assert!(LanguageCode::new("english").is_err());
    assert!(LanguageCode::new("EN-gb").is_err());
    assert!(LanguageCode::new("e-GB").is_err());
}

#[test]
fn language_code_display() {
    let code = LanguageCode::new("en-GB").unwrap();
    assert_eq!(format!("{code}"), "en-GB");
}

// =====================================================================
// Module: cmd.rs — CLI
// =====================================================================

#[test]
fn cli_build_creates_command() {
    let cmd = Cli::build();
    assert_eq!(cmd.get_name(), "ssg");
}

#[test]
fn cli_constants() {
    assert_eq!(DEFAULT_PORT, 8000);
    assert_eq!(DEFAULT_HOST, "127.0.0.1");
    assert!(RESERVED_NAMES.contains(&"con"));
    assert!(RESERVED_NAMES.contains(&"nul"));
}

// =====================================================================
// Module: plugin.rs — Plugin system
// =====================================================================

#[derive(Debug)]
struct TestPlugin {
    name: &'static str,
}

impl Plugin for TestPlugin {
    fn name(&self) -> &str {
        self.name
    }
    fn after_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
        Ok(())
    }
}

#[test]
fn plugin_manager_register_and_run() -> Result<()> {
    let mut pm = PluginManager::new();
    pm.register(TestPlugin { name: "test" });
    assert_eq!(pm.len(), 1);
    assert!(!pm.is_empty());
    assert_eq!(pm.names(), vec!["test"]);

    let ctx = PluginContext::new(
        Path::new("c"),
        Path::new("b"),
        Path::new("s"),
        Path::new("t"),
    );
    pm.run_before_compile(&ctx)?;
    pm.run_after_compile(&ctx)?;
    pm.run_on_serve(&ctx)?;
    Ok(())
}

#[test]
fn plugin_manager_empty_runs_succeed() -> Result<()> {
    let pm = PluginManager::new();
    let ctx = PluginContext::new(
        Path::new("c"),
        Path::new("b"),
        Path::new("s"),
        Path::new("t"),
    );
    pm.run_before_compile(&ctx)?;
    pm.run_after_compile(&ctx)?;
    pm.run_on_serve(&ctx)?;
    Ok(())
}

#[test]
fn plugin_context_fields() {
    let ctx = PluginContext::new(
        Path::new("/a"),
        Path::new("/b"),
        Path::new("/c"),
        Path::new("/d"),
    );
    assert_eq!(ctx.content_dir, PathBuf::from("/a"));
    assert_eq!(ctx.build_dir, PathBuf::from("/b"));
    assert_eq!(ctx.site_dir, PathBuf::from("/c"));
    assert_eq!(ctx.template_dir, PathBuf::from("/d"));
}

// =====================================================================
// Module: plugins.rs — Built-in plugins
// =====================================================================

#[test]
fn minify_plugin_processes_html() -> Result<()> {
    let tmp = tempdir()?;
    fs::write(tmp.path().join("page.html"), "<p>  spaced  </p>")?;
    let ctx = PluginContext::new(
        Path::new("c"),
        Path::new("b"),
        tmp.path(),
        Path::new("t"),
    );
    MinifyPlugin.after_compile(&ctx)?;
    let content = fs::read_to_string(tmp.path().join("page.html"))?;
    assert!(!content.contains("  "));
    Ok(())
}

#[test]
fn image_opti_plugin_scans_images() -> Result<()> {
    let tmp = tempdir()?;
    fs::write(tmp.path().join("photo.png"), "PNG")?;
    let ctx = PluginContext::new(
        Path::new("c"),
        Path::new("b"),
        tmp.path(),
        Path::new("t"),
    );
    ImageOptiPlugin.after_compile(&ctx)?;
    Ok(())
}

#[test]
fn deploy_plugin_runs() -> Result<()> {
    let tmp = tempdir()?;
    let ctx = PluginContext::new(
        Path::new("c"),
        Path::new("b"),
        tmp.path(),
        Path::new("t"),
    );
    let p = DeployPlugin::new("staging");
    p.after_compile(&ctx)?;
    Ok(())
}

#[test]
fn all_builtin_plugins_register() {
    let mut pm = PluginManager::new();
    pm.register(MinifyPlugin);
    pm.register(ImageOptiPlugin);
    pm.register(DeployPlugin::new("prod"));
    assert_eq!(pm.len(), 3);
    assert_eq!(pm.names(), vec!["minify", "image-opti", "deploy"]);
}

// =====================================================================
// Module: cache.rs — Incremental builds
// =====================================================================

#[test]
fn build_cache_load_missing_returns_empty() -> Result<()> {
    let tmp = tempdir()?;
    let cache = BuildCache::load(&tmp.path().join("missing.json"))?;
    assert!(cache.is_empty());
    Ok(())
}

#[test]
fn build_cache_save_and_load_roundtrip() -> Result<()> {
    let tmp = tempdir()?;
    let cache_path = tmp.path().join("cache.json");
    let content_dir = tmp.path().join("content");
    fs::create_dir_all(&content_dir)?;
    fs::write(content_dir.join("a.md"), "# Hello")?;

    let mut cache = BuildCache::new(&cache_path);
    cache.update(&content_dir)?;
    cache.save()?;

    let loaded = BuildCache::load(&cache_path)?;
    assert_eq!(loaded.len(), 1);
    Ok(())
}

#[test]
fn build_cache_detects_changes() -> Result<()> {
    let tmp = tempdir()?;
    let cache_path = tmp.path().join("cache.json");
    let content_dir = tmp.path().join("content");
    fs::create_dir_all(&content_dir)?;
    fs::write(content_dir.join("a.md"), "original")?;

    let mut cache = BuildCache::new(&cache_path);
    cache.update(&content_dir)?;

    // Modify file
    fs::write(content_dir.join("a.md"), "modified")?;
    let changed = cache.changed_files(&content_dir)?;
    assert_eq!(changed.len(), 1);
    Ok(())
}

#[test]
fn build_cache_detects_no_changes() -> Result<()> {
    let tmp = tempdir()?;
    let cache_path = tmp.path().join("cache.json");
    let content_dir = tmp.path().join("content");
    fs::create_dir_all(&content_dir)?;
    fs::write(content_dir.join("a.md"), "stable")?;

    let mut cache = BuildCache::new(&cache_path);
    cache.update(&content_dir)?;

    let changed = cache.changed_files(&content_dir)?;
    assert!(changed.is_empty(), "{:?}", changed);
    Ok(())
}

#[test]
fn build_cache_detects_new_files() -> Result<()> {
    let tmp = tempdir()?;
    let cache_path = tmp.path().join("cache.json");
    let content_dir = tmp.path().join("content");
    fs::create_dir_all(&content_dir)?;

    let cache = BuildCache::new(&cache_path);
    fs::write(content_dir.join("new.md"), "new")?;

    let changed = cache.changed_files(&content_dir)?;
    assert_eq!(changed.len(), 1);
    Ok(())
}

// =====================================================================
// Module: stream.rs — Streaming I/O
// =====================================================================

#[test]
fn stream_copy_preserves_content() -> Result<()> {
    let tmp = tempdir()?;
    let src = tmp.path().join("src.bin");
    let dst = tmp.path().join("dst.bin");
    let data = vec![42u8; 100_000]; // 100 KB
    fs::write(&src, &data)?;

    let bytes = stream_copy(&src, &dst)?;
    assert_eq!(bytes, 100_000);
    assert_eq!(fs::read(&dst)?, data);
    Ok(())
}

#[test]
fn stream_hash_deterministic() -> Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("f.txt");
    fs::write(&path, "consistent")?;

    let h1 = stream_hash(&path)?;
    let h2 = stream_hash(&path)?;
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 16);
    Ok(())
}

#[test]
fn stream_hash_differs_for_different_content() -> Result<()> {
    let tmp = tempdir()?;
    let a = tmp.path().join("a.txt");
    let b = tmp.path().join("b.txt");
    fs::write(&a, "alpha")?;
    fs::write(&b, "beta")?;
    assert_ne!(stream_hash(&a)?, stream_hash(&b)?);
    Ok(())
}

#[test]
fn process_batch_copies_all_files() -> Result<()> {
    let tmp = tempdir()?;
    let src = tmp.path().join("in");
    let dst = tmp.path().join("out");
    fs::create_dir_all(&src)?;
    for i in 0..20 {
        fs::write(src.join(format!("f{i}.txt")), format!("data{i}"))?;
    }

    let result = process_batch(&src, &dst, stream_copy)?;
    assert_eq!(result.files_processed, 20);
    assert!(result.throughput > 0.0);
    Ok(())
}

#[test]
fn stream_lines_processes_each_line() -> Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("lines.txt");
    fs::write(&path, "one\ntwo\nthree")?;

    let mut lines = Vec::new();
    let _ = stream_lines(&path, |_i, line| {
        lines.push(line.to_string());
        Ok(())
    })?;
    assert_eq!(lines, vec!["one", "two", "three"]);
    Ok(())
}

#[test]
fn stream_constants() {
    assert_eq!(STREAM_BUFFER_SIZE, 8192);
    assert_eq!(MAX_BATCH_SIZE, 100_000);
}

#[test]
fn batch_result_fields() {
    let r = BatchResult {
        files_processed: 5,
        bytes_read: 500,
        bytes_written: 450,
        duration_ms: 1.0,
        throughput: 5000.0,
    };
    assert_eq!(r.files_processed, 5);
    assert_eq!(r.bytes_read, 500);
}

// =====================================================================
// Module: watch.rs — File watching
// =====================================================================

#[test]
fn watch_config_accessors() {
    let cfg =
        WatchConfig::new(PathBuf::from("content"), Duration::from_secs(2));
    assert_eq!(cfg.directory(), Path::new("content"));
    assert_eq!(cfg.poll_interval(), Duration::from_secs(2));
}

#[test]
fn file_watcher_initial_snapshot() -> Result<()> {
    let tmp = tempdir()?;
    fs::write(tmp.path().join("a.txt"), "a")?;
    fs::write(tmp.path().join("b.txt"), "b")?;

    let cfg = WatchConfig::new(tmp.path().into(), Duration::from_millis(10));
    let watcher = FileWatcher::new(cfg)?;
    assert_eq!(watcher.tracked_file_count(), 2);
    Ok(())
}

#[test]
fn file_watcher_detects_new_file() -> Result<()> {
    let tmp = tempdir()?;
    let cfg = WatchConfig::new(tmp.path().into(), Duration::from_millis(10));
    let mut watcher = FileWatcher::new(cfg)?;

    fs::write(tmp.path().join("new.txt"), "new")?;
    let changes = watcher.check_for_changes()?;
    assert!(!changes.is_empty(), "should not be empty");
    Ok(())
}

#[test]
fn file_watcher_no_changes() -> Result<()> {
    let tmp = tempdir()?;
    fs::write(tmp.path().join("stable.txt"), "stable")?;

    let cfg = WatchConfig::new(tmp.path().into(), Duration::from_millis(10));
    let mut watcher = FileWatcher::new(cfg)?;

    let changes = watcher.check_for_changes()?;
    assert!(changes.is_empty(), "{:?}", changes);
    Ok(())
}

#[test]
fn watch_constants() {
    assert_eq!(MAX_WATCH_ITERATIONS, 1_000_000);
}

// =====================================================================
// Module: schema.rs — JSON Schema
// =====================================================================

#[test]
fn schema_has_all_config_fields() {
    let schema = generate_schema();
    let props = schema["properties"].as_object().unwrap();
    assert!(props.contains_key("site_name"));
    assert!(props.contains_key("content_dir"));
    assert!(props.contains_key("output_dir"));
    assert!(props.contains_key("template_dir"));
    assert!(props.contains_key("serve_dir"));
    assert!(props.contains_key("base_url"));
    assert!(props.contains_key("site_title"));
    assert!(props.contains_key("site_description"));
    assert!(props.contains_key("language"));
}

#[test]
fn schema_write_creates_file() -> Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("schema.json");
    write_schema(&path)?;
    assert!(path.exists());
    let content = fs::read_to_string(&path)?;
    assert!(content.contains("site_name"));
    Ok(())
}

#[test]
fn schema_language_has_pattern() {
    let schema = generate_schema();
    let lang = &schema["properties"]["language"];
    assert!(lang["pattern"].as_str().is_some());
}

// =====================================================================
// Module: process.rs — Argument processing
// =====================================================================

#[test]
fn process_error_display() {
    use ssg::process::ProcessError;
    let err = ProcessError::MissingArgument("test".into());
    assert!(format!("{err}").contains("test"));
}

#[test]
fn get_argument_returns_value() {
    use clap::{arg, Command};
    use ssg::process::get_argument;
    let matches = Command::new("t")
        .arg(arg!(--"name" <NAME> "Name"))
        .get_matches_from(vec!["t", "--name", "val"]);
    assert_eq!(get_argument(&matches, "name").unwrap(), "val");
}

#[test]
fn get_argument_missing_errors() {
    use clap::{arg, Command};
    use ssg::process::get_argument;
    // Define the arg but don't provide it — get_one returns None
    let matches = Command::new("t")
        .arg(arg!(--"name" <NAME> "Name").required(false))
        .get_matches_from(vec!["t"]);
    assert!(get_argument(&matches, "name").is_err());
}

#[test]
fn ensure_directory_creates() -> Result<()> {
    use ssg::process::ensure_directory;
    let tmp = tempdir()?;
    let dir = tmp.path().join("new_dir");
    ensure_directory(&dir, "test")?;
    assert!(dir.exists());
    Ok(())
}

// =====================================================================
// End-to-end: Full pipeline
// =====================================================================

#[test]
fn e2e_cache_then_copy_pipeline() -> Result<()> {
    let tmp = tempdir()?;
    let content = tmp.path().join("content");
    let build = tmp.path().join("build");
    let cache_path = tmp.path().join(".ssg-cache.json");

    fs::create_dir_all(&content)?;
    fs::write(content.join("page.md"), "# Hello")?;

    // First build: everything is new
    let mut cache = BuildCache::load(&cache_path)?;
    let changed = cache.changed_files(&content)?;
    assert_eq!(changed.len(), 1);

    // Copy changed files
    fs::create_dir_all(&build)?;
    for src in &changed {
        let name = src.file_name().unwrap();
        let _ = stream_copy(src, &build.join(name))?;
    }
    assert!(build.join("page.md").exists());

    // Update cache
    cache.update(&content)?;
    cache.save()?;

    // Second build: no changes
    let cache2 = BuildCache::load(&cache_path)?;
    let changed2 = cache2.changed_files(&content)?;
    assert!(changed2.is_empty(), "{:?}", changed2);
    Ok(())
}

#[test]
fn e2e_plugin_pipeline() -> Result<()> {
    let tmp = tempdir()?;
    let site = tmp.path().join("site");
    fs::create_dir_all(&site)?;
    fs::write(site.join("index.html"), "<h1>  Hello   World  </h1>")?;

    let mut pm = PluginManager::new();
    pm.register(MinifyPlugin);
    pm.register(ImageOptiPlugin);
    pm.register(DeployPlugin::new("test"));

    let ctx = PluginContext::new(
        Path::new("content"),
        Path::new("build"),
        &site,
        Path::new("templates"),
    );

    pm.run_before_compile(&ctx)?;
    pm.run_after_compile(&ctx)?;
    pm.run_on_serve(&ctx)?;

    // Minify should have collapsed whitespace
    let html = fs::read_to_string(site.join("index.html"))?;
    assert!(!html.contains("   "));
    Ok(())
}

#[test]
fn e2e_stream_batch_with_hash_verification() -> Result<()> {
    let tmp = tempdir()?;
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(&src)?;

    for i in 0..50 {
        fs::write(src.join(format!("f{i}.txt")), format!("content-{i}"))?;
    }

    let result = process_batch(&src, &dst, stream_copy)?;
    assert_eq!(result.files_processed, 50);

    // Verify content integrity via hash
    for i in 0..50 {
        let src_hash = stream_hash(&src.join(format!("f{i}.txt")))?;
        let dst_hash = stream_hash(&dst.join(format!("f{i}.txt")))?;
        assert_eq!(src_hash, dst_hash, "hash mismatch for f{i}.txt");
    }
    Ok(())
}

#[test]
fn e2e_full_directory_lifecycle() -> Result<()> {
    let tmp = tempdir()?;
    let paths = Paths {
        site: tmp.path().join("public"),
        content: tmp.path().join("content"),
        build: tmp.path().join("build"),
        template: tmp.path().join("templates"),
    };

    // Create directories
    create_directories(&paths)?;

    // Write content
    fs::write(paths.content.join("index.md"), "# Home")?;
    fs::write(paths.content.join("about.md"), "# About")?;

    // Collect files
    let mut files = Vec::new();
    collect_files_recursive(&paths.content, &mut files)?;
    assert_eq!(files.len(), 2);

    // Copy to build
    verify_and_copy_files(&paths.content, &paths.build)?;
    assert!(paths.build.join("index.md").exists());
    assert!(paths.build.join("about.md").exists());

    // Log
    let mut log =
        create_log_file(tmp.path().join("build.log").to_str().unwrap())?;
    let date = ssg::now_iso();
    log_initialization(&mut log, &date)?;
    log_arguments(&mut log, &date)?;

    Ok(())
}
