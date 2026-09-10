// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Build pipeline: plugin orchestration and site compilation.

use std::path::{Path, PathBuf};

use crate::error::SsgError;
use staticdatagen::compile;

use crate::cmd::SsgConfig;
use crate::{
    accessibility, ai, assets, content, csp, deploy, drafts, highlight, i18n,
    islands, livereload, pagination, plugin, plugins as plugins_mod,
    postprocess, search, seo, shortcodes, streaming, taxonomy, walk,
};

// ---------------------------------------------------------------------------
// BuildError — serialisable build error for browser overlay delivery
// ---------------------------------------------------------------------------

/// Serialisable build error for browser overlay delivery.
#[derive(Debug, Clone, serde::Serialize)]
#[allow(dead_code)]
pub struct BuildError {
    /// Source file path (if extractable from the error chain).
    pub file: Option<String>,
    /// Line number (if extractable).
    pub line: Option<usize>,
    /// Human-readable error message.
    pub message: String,
}

impl BuildError {
    /// Creates a `BuildError` from an `SsgError` error, attempting to extract
    /// file path and line number from the error chain.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::pipeline::BuildError;
    /// use ssg::SsgError;
    ///
    /// let err = SsgError::Validation { field: "x".into(), message: "nope".into() };
    /// let be = BuildError::from_error(&err);
    /// assert!(be.message.contains("nope"));
    /// ```
    #[must_use]
    #[allow(dead_code)]
    pub fn from_error(err: &SsgError) -> Self {
        let message = format!("{err:#}");
        let file = extract_file_from_error(&message);
        Self {
            file,
            line: None,
            message,
        }
    }

    /// Serializes to a WebSocket JSON message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::pipeline::BuildError;
    ///
    /// let be = BuildError { file: None, line: None, message: "boom".into() };
    /// let msg = be.to_ws_message();
    /// assert!(msg.contains("\"type\":\"error\""));
    /// assert!(msg.contains("boom"));
    /// ```
    #[must_use]
    #[allow(dead_code)]
    pub fn to_ws_message(&self) -> String {
        serde_json::json!({
            "type": "error",
            "file": self.file,
            "line": self.line,
            "message": self.message,
        })
        .to_string()
    }
}

/// Returns the JSON message to clear the error overlay.
///
/// # Examples
///
/// ```rust
/// use ssg::pipeline::clear_error_message;
///
/// assert!(clear_error_message().contains("clear-error"));
/// ```
#[must_use]
#[allow(dead_code)]
pub fn clear_error_message() -> String {
    r#"{"type":"clear-error"}"#.to_string()
}

/// Extracts a file path from an error message by scanning for path-like
/// tokens ending in known extensions.
#[allow(dead_code)]
fn extract_file_from_error(msg: &str) -> Option<String> {
    for word in msg.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| {
            !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-'
        });
        if trimmed.contains('/')
            && (trimmed.ends_with(".md")
                || trimmed.ends_with(".html")
                || trimmed.ends_with(".toml")
                || trimmed.ends_with(".yml")
                || trimmed.ends_with(".yaml"))
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// CLI-driven options that don't live in `SsgConfig` itself.
///
/// Extracted from clap matches so the run pipeline can be unit-tested
/// without going through `Cli::build()`. **Internal**: this is a
/// CLI-implementation type, not part of the library surface. The
/// containing module is `pub(crate)`, so this `pub` is effectively
/// crate-local — clippy's `redundant_pub_crate` flagged the prior
/// `pub(crate)` here. See
/// [API stability audit](../../docs/architecture/api-stability-audit.md)
/// (Tier C) for context.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunOptions {
    /// Suppress banner and timing print-outs.
    pub quiet: bool,
    /// Include draft files (skip the `DraftPlugin` filter).
    pub include_drafts: bool,
    /// Optional deploy target — `netlify`, `vercel`, `cloudflare`, `github`.
    pub deploy_target: Option<String>,
    /// Validate content schemas only (no build).
    pub validate_only: bool,
    /// Number of parallel threads for Rayon (`--jobs`).
    /// `None` means use all available CPUs.
    pub jobs: Option<usize>,
    /// Peak memory budget in MB for streaming compilation.
    /// `None` means use the default (512 MB).
    pub max_memory_mb: Option<usize>,
    /// Run the agentic AI pipeline to audit and fix content.
    #[allow(dead_code)]
    pub ai_fix: bool,
    /// Preview AI fixes without writing files.
    #[allow(dead_code)]
    pub ai_fix_dry_run: bool,
    /// Use the cached dependency graph to skip work on unchanged
    /// sources (`ssg build --incremental`, issue #524).
    pub incremental: bool,
    /// Disable the deterministic LLM inference cache (issue #528).
    /// Surfaces as `--no-llm-cache` on the CLI and is exported to
    /// `LlmConfig::default` via the `SSG_NO_LLM_CACHE` env var so
    /// any code path constructing an `LlmConfig` from defaults
    /// (CLI helpers, integration tests, plugin re-entrants) sees a
    /// consistent setting.
    pub no_llm_cache: bool,
    /// Emit ISR build manifest + raw KV payloads under `dist/.ssg/`
    /// (issue #546). Off by default — when false the build is
    /// byte-identical to v0.0.43 (AC9).
    pub isr: bool,
}

impl RunOptions {
    /// Builds a `RunOptions` from a parsed `clap::ArgMatches`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::cmd::Cli;
    /// use ssg::pipeline::RunOptions;
    ///
    /// let matches = Cli::build().get_matches_from(vec!["ssg", "--quiet"]);
    /// let opts = RunOptions::from_matches(&matches);
    /// assert!(opts.quiet);
    /// ```
    pub fn from_matches(matches: &clap::ArgMatches) -> Self {
        Self {
            quiet: matches.get_flag("quiet"),
            include_drafts: matches.get_flag("drafts"),
            deploy_target: matches.get_one::<String>("deploy").cloned(),
            validate_only: matches.get_flag("validate"),
            jobs: matches.get_one::<usize>("jobs").copied(),
            max_memory_mb: matches.get_one::<usize>("max-memory").copied(),
            ai_fix: matches.get_flag("ai-fix"),
            ai_fix_dry_run: matches.get_flag("ai-fix-dry-run"),
            incremental: matches
                .try_contains_id("incremental")
                .unwrap_or(false)
                && matches.get_flag("incremental"),
            no_llm_cache: matches
                .try_contains_id("no-llm-cache")
                .unwrap_or(false)
                && matches.get_flag("no-llm-cache"),
            isr: matches.try_contains_id("isr").unwrap_or(false)
                && matches.get_flag("isr"),
        }
    }

    /// Builds a `RunOptions` from subcommand-style matches.
    ///
    /// The subcommand parser exposes a narrower flag set — `--quiet`,
    /// `--drafts`, `--jobs`, and on the `build` subcommand
    /// `--max-memory`. Anything else falls back to the defaults so
    /// downstream callers don't have to special-case missing IDs.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::cmd::Cli;
    /// use ssg::pipeline::RunOptions;
    ///
    /// let matches = Cli::subcommand_app().get_matches_from(vec!["ssg", "build"]);
    /// let sub_m = matches.subcommand_matches("build").unwrap();
    /// let opts = RunOptions::from_subcommand_matches(sub_m);
    /// assert!(!opts.quiet);
    /// ```
    pub fn from_subcommand_matches(sub_m: &clap::ArgMatches) -> Self {
        let opt_flag = |name: &str| -> bool {
            sub_m.try_contains_id(name).unwrap_or(false) && sub_m.get_flag(name)
        };
        let opt_one = |name: &str| -> Option<usize> {
            if sub_m.try_contains_id(name).unwrap_or(false) {
                sub_m.get_one::<usize>(name).copied()
            } else {
                None
            }
        };
        let opt_str = |name: &str| -> Option<String> {
            if sub_m.try_contains_id(name).unwrap_or(false) {
                sub_m.get_one::<String>(name).cloned()
            } else {
                None
            }
        };
        Self {
            quiet: opt_flag("quiet"),
            include_drafts: opt_flag("drafts"),
            // `deploy` lives on the deploy subcommand as `--target`.
            // We map it across so the existing
            // `register_default_plugins(..., deploy_target)` keeps its
            // contract.
            deploy_target: opt_str("target"),
            validate_only: false,
            jobs: opt_one("jobs"),
            max_memory_mb: opt_one("max-memory"),
            ai_fix: false,
            ai_fix_dry_run: false,
            incremental: opt_flag("incremental"),
            no_llm_cache: opt_flag("no-llm-cache"),
            isr: opt_flag("isr"),
        }
    }
}

/// Resolves distinct build and site directories for compilation.
///
/// `staticdatagen::compile` finalizes output by renaming the build directory
/// into the site directory. If both paths are identical, finalization fails.
/// This helper guarantees distinct paths when needed.
///
/// # Examples
///
/// ```rust
/// use ssg::cmd::SsgConfig;
/// use ssg::pipeline::resolve_build_and_site_dirs;
///
/// let cfg = SsgConfig::default();
/// let (build, site) = resolve_build_and_site_dirs(&cfg);
/// // When serve_dir is unset and equals output_dir, build dir differs.
/// assert_ne!(build, site);
/// ```
pub fn resolve_build_and_site_dirs(config: &SsgConfig) -> (PathBuf, PathBuf) {
    let site_dir = config
        .serve_dir
        .clone()
        .unwrap_or_else(|| config.output_dir.clone());

    let build_dir = if site_dir == config.output_dir {
        config.output_dir.with_extension("build-tmp")
    } else {
        config.output_dir.clone()
    };

    (build_dir, site_dir)
}

/// Builds a fully-populated plugin manager and plugin context for a build.
///
/// Extracted so unit tests can construct the same wiring without
/// needing to fake CLI argument parsing.
///
/// # Examples
///
/// ```rust
/// use ssg::cmd::SsgConfig;
/// use ssg::pipeline::{build_pipeline, RunOptions};
///
/// let cfg = SsgConfig::default();
/// let opts = RunOptions::default();
/// let (_plugins, _ctx, build, site) = build_pipeline(&cfg, &opts);
/// assert_ne!(build, site);
/// ```
pub fn build_pipeline(
    config: &SsgConfig,
    opts: &RunOptions,
) -> (
    plugin::PluginManager,
    plugin::PluginContext,
    PathBuf,
    PathBuf,
) {
    let (build_dir, site_dir) = resolve_build_and_site_dirs(config);

    // Issue #528 — propagate `--no-llm-cache` to every `LlmConfig`
    // constructed downstream by exporting `SSG_NO_LLM_CACHE=1` once
    // here. The env-var approach avoids threading a new parameter
    // through `register_default_plugins` and through every direct
    // `LlmConfig::default()` call site (CLI helpers, tests,
    // integration entry points). The plugin reads the env var inside
    // its `Default` impl.
    if opts.no_llm_cache {
        std::env::set_var("SSG_NO_LLM_CACHE", "1");
    }

    let mut ctx = plugin::PluginContext::with_config(
        &config.content_dir,
        &build_dir,
        &site_dir,
        &config.template_dir,
        config.clone(),
    );

    // Set memory budget if --max-memory was specified
    if let Some(mb) = opts.max_memory_mb {
        ctx.memory_budget = Some(streaming::MemoryBudget::from_mb(mb));
    }

    let mut plugins = plugin::PluginManager::new();
    register_default_plugins(
        &mut plugins,
        config,
        opts.include_drafts,
        opts.deploy_target.as_deref(),
    );
    if opts.isr {
        register_isr_plugins(&mut plugins);
    }

    (plugins, ctx, build_dir, site_dir)
}

/// Appends ISR-specific plugins (currently just
/// [`crate::isr_manifest::IsrManifestPlugin`]).
///
/// Pulled out of [`register_default_plugins`] so the default plugin
/// graph stays byte-identical when `--isr` is not passed (AC9 of
/// issue #546). Anything registered here MUST be a strict superset
/// of the v0.0.43 output; failing AC9 fails the entire epic.
///
/// # Examples
///
/// ```rust
/// use ssg::pipeline::register_isr_plugins;
/// use ssg::plugin::PluginManager;
///
/// let mut pm = PluginManager::new();
/// register_isr_plugins(&mut pm);
/// // ISR plugins get appended without panicking.
/// ```
pub fn register_isr_plugins(plugins: &mut plugin::PluginManager) {
    plugins.register(crate::isr_manifest::IsrManifestPlugin::new());
    // Edge RPC schema emitter (issue #548). Registered alongside ISR
    // because both target the same `dist/.ssg/` artefact directory
    // and both are no-ops without the matching opt-in. When zero
    // `#[ssg_rpc]` functions are linked, the plugin writes nothing,
    // preserving the v0.0.43 byte-identical promise.
    plugins.register(crate::rpc_schema::RpcSchemaPlugin::new());
}

/// Runs the build half of the pipeline: `before_compile` → compile →
/// `after_compile`. Does not start the dev server.
///
/// Extracted from `run()` so the actual build can be unit-tested
/// against a tempdir without booting an HTTP server.
#[cfg_attr(
    feature = "otel",
    tracing::instrument(skip(plugins, ctx), fields(
        content_dir = %content_dir.display(),
        site_dir = %site_dir.display(),
        quiet,
    ))
)]
///
/// # Examples
///
/// ```no_run
/// use ssg::cmd::SsgConfig;
/// use ssg::pipeline::{build_pipeline, execute_build_pipeline, RunOptions};
///
/// let cfg = SsgConfig::default();
/// let opts = RunOptions::default();
/// let (plugins, ctx, build, site) = build_pipeline(&cfg, &opts);
/// // Wired but not invoked here — would need real content/template dirs.
/// let _ = execute_build_pipeline(&plugins, &ctx, &build, &cfg.content_dir, &site, &cfg.template_dir, true);
/// ```
pub fn execute_build_pipeline(
    plugins: &plugin::PluginManager,
    ctx: &plugin::PluginContext,
    build_dir: &Path,
    content_dir: &Path,
    site_dir: &Path,
    template_dir: &Path,
    quiet: bool,
) -> Result<(), SsgError> {
    execute_build_pipeline_with(
        plugins,
        ctx,
        build_dir,
        content_dir,
        site_dir,
        template_dir,
        quiet,
        false,
    )
}

/// Variant of [`execute_build_pipeline`] that accepts the
/// `--incremental` flag.
///
/// When `incremental` is `true` and the persisted dependency graph at
/// `<cache_root>/depgraph.json` shows no source-side changes, the
/// full compile + transform passes are skipped — the site on disk
/// from the previous build is the authoritative output. When sources
/// did change, the full compile runs but the resulting graph is
/// persisted with fresh sha256 freshness keys so the next incremental
/// invocation can short-circuit.
///
/// The cache root is `<build_dir>/../target/ssg-cache/` when
/// `build_dir` is a sibling of `target/`; otherwise it lives directly
/// under `<site_dir>/.ssg-cache/`. The chosen path is logged.
#[cfg_attr(
    feature = "otel",
    tracing::instrument(skip(plugins, ctx), fields(
        content_dir = %content_dir.display(),
        site_dir = %site_dir.display(),
        quiet,
        incremental,
    ))
)]
///
/// # Examples
///
/// ```no_run
/// use ssg::cmd::SsgConfig;
/// use ssg::pipeline::{build_pipeline, execute_build_pipeline_with, RunOptions};
///
/// let cfg = SsgConfig::default();
/// let opts = RunOptions::default();
/// let (plugins, ctx, build, site) = build_pipeline(&cfg, &opts);
/// let _ = execute_build_pipeline_with(
///     &plugins, &ctx, &build, &cfg.content_dir, &site, &cfg.template_dir,
///     true, false,
/// );
/// ```
pub fn execute_build_pipeline_with(
    plugins: &plugin::PluginManager,
    ctx: &plugin::PluginContext,
    build_dir: &Path,
    content_dir: &Path,
    site_dir: &Path,
    template_dir: &Path,
    quiet: bool,
    incremental: bool,
) -> Result<(), SsgError> {
    let start = std::time::Instant::now();

    let cache_root = depgraph_cache_root(site_dir);

    // Load plugin cache + dep graph from the canonical cache root.
    let plugin_cache = plugin::PluginCache::load(site_dir);
    let prev_graph = crate::depgraph::DepGraph::load(&cache_root);

    let mut ctx = ctx.clone();
    ctx.cache = Some(plugin_cache);
    ctx.dep_graph = Some(prev_graph.clone());

    // ----- Incremental fast path ---------------------------------
    // Compute current hashes and diff against the cached graph. If
    // nothing changed and the previous output still exists on disk,
    // we can skip the entire compile + after_compile + transform
    // chain. This is the warm-cache <200ms target (AC4).
    if incremental {
        let current =
            crate::depgraph::current_hashes(content_dir, template_dir)?;
        let diff = prev_graph.diff(&current);
        if diff.is_empty() && prev_graph.page_count() > 0 && site_dir.exists() {
            let elapsed = start.elapsed();
            if !quiet {
                println!(
                    "Site cached ({} pages, no changes) in {:.2}ms",
                    prev_graph.page_count(),
                    elapsed.as_secs_f64() * 1000.0,
                );
            }
            return Ok(());
        }

        // Handle deletes: remove stale outputs and drop the deleted
        // entries from the persisted graph (AC5).
        if !diff.deleted.is_empty() {
            let stale_outputs = prev_graph.invalidated_outputs(&diff.deleted);
            for out in &stale_outputs {
                let _ = std::fs::remove_file(out);
            }
        }
    }

    plugins.run_before_compile(&ctx)?;

    // Use streaming compilation for large sites when --max-memory is set
    // or the site exceeds the default batch size.
    let budget = ctx
        .memory_budget
        .unwrap_or_else(streaming::MemoryBudget::default_budget);
    let explicitly_set = ctx.memory_budget.is_some();

    if streaming::should_stream(content_dir, &budget, explicitly_set) {
        let batches = streaming::batched_content_files(content_dir, &budget)?;
        for (i, batch) in batches.iter().enumerate() {
            streaming::compile_batch(
                batch,
                content_dir,
                build_dir,
                site_dir,
                template_dir,
                i,
            )?;
        }
    } else {
        // Spec A2/B1 (plan §2 item 1.2, issue #586): thread the site's
        // base URL into the compile so the content stager can inject a
        // derived `permalink:` for pages that don't declare one —
        // mirroring how the postprocess plugins source `base_url` from
        // the plugin context's config.
        let base_url = ctx.config.as_ref().map(|c| c.base_url.clone());
        let locales = ctx
            .config
            .as_ref()
            .and_then(|c| c.i18n.as_ref())
            .map(|i| i.locales.clone())
            .unwrap_or_default();
        compile_site_with_locales(
            build_dir,
            content_dir,
            site_dir,
            template_dir,
            base_url.as_deref(),
            &locales,
        )?;
    }

    // Cache HTML file list once — shared by all after_compile plugins,
    // eliminating 8+ redundant directory walks.
    ctx.cache_html_files();

    plugins.run_after_compile(&ctx)?;

    // Fused transform pass: read each HTML once → pipe through all
    // transform plugins → write once. Eliminates redundant I/O.
    plugins.run_fused_transforms(&ctx)?;

    // Master Quality Gate & Compliance Audit
    let audit_report =
        crate::plugins_group::audit::AuditPlugin::audit_directory(site_dir);
    let audit_path = site_dir.join("quality-gate-report.json");
    if let Ok(json_str) = serde_json::to_string_pretty(&audit_report) {
        let _ = std::fs::write(&audit_path, json_str);
    }
    if audit_report.passed_pillars == audit_report.total_pillars {
        log::info!(
            "[audit] Quality Gate: {}/{} pillars passed across {} pages (0 issues)",
            audit_report.passed_pillars,
            audit_report.total_pillars,
            audit_report.pages_scanned
        );
    } else {
        log::warn!(
            "[audit] Quality Gate: {}/{} pillars passed across {} pages ({} issues)",
            audit_report.passed_pillars,
            audit_report.total_pillars,
            audit_report.pages_scanned,
            audit_report.total_issues
        );
    }

    // Rebuild the dep graph from scratch on a successful compile so
    // the next `--incremental` invocation sees a consistent snapshot.
    let mut new_graph = crate::depgraph::DepGraph::new();
    if let Err(e) = crate::depgraph::populate(
        &mut new_graph,
        content_dir,
        template_dir,
        site_dir,
    ) {
        log::warn!("Failed to populate dependency graph: {e}");
    }

    if let Err(e) = new_graph.save(&cache_root) {
        log::warn!("Failed to save dependency graph: {e}");
    }

    // Rebuild and save the plugin content-hash cache.
    if let Some(ref mut cache) = ctx.cache {
        if let Ok(files) = walk::walk_files(site_dir, "html") {
            for file in &files {
                cache.update(file);
            }
        }
        if let Err(e) = cache.save(site_dir) {
            log::warn!("Failed to save plugin cache: {e}");
        }
    }

    let elapsed = start.elapsed();
    if !quiet {
        println!(
            "Site built in {:.2}s ({} plugin(s))",
            elapsed.as_secs_f64(),
            plugins.len()
        );
    }
    Ok(())
}

/// Resolves the on-disk cache root for the persisted dependency graph.
///
/// Issue #524 specifies `target/ssg-cache/`; when no `target/`
/// directory is available (e.g. tests or sites built outside cargo),
/// the cache lives at `<site_dir>/.ssg-cache/`.
///
/// # Examples
///
/// ```rust
/// use ssg::pipeline::depgraph_cache_root;
/// use std::path::Path;
///
/// let cache_root = depgraph_cache_root(Path::new("/tmp/site"));
/// assert!(cache_root.exists() || cache_root.ends_with(".ssg-cache") || cache_root.ends_with("ssg-cache"));
/// ```
#[must_use]
pub fn depgraph_cache_root(site_dir: &Path) -> PathBuf {
    let target = Path::new("target");
    if target.is_dir() {
        target.join(crate::depgraph::CACHE_DIRNAME)
    } else {
        site_dir.join(".ssg-cache")
    }
}

/// Compiles the static site from source directories.
///
/// Convenience wrapper over [`compile_site_with_base_url`] with no
/// base URL — no `permalink:` derivation happens on staged content.
/// The full build pipeline calls [`compile_site_with_base_url`] with
/// the configured `base_url` instead (spec A2/B1, plan §2 item 1.2).
///
/// # Examples
///
/// ```no_run
/// use ssg::pipeline::compile_site;
/// use std::path::Path;
///
/// // Real call requires populated content/template trees; only the
/// // signature is exercised here.
/// let _ = compile_site(
///     Path::new("build"), Path::new("content"),
///     Path::new("site"), Path::new("templates"),
/// );
/// ```
pub fn compile_site(
    build_dir: &Path,
    content_dir: &Path,
    site_dir: &Path,
    template_dir: &Path,
) -> Result<(), SsgError> {
    compile_site_with_base_url(
        build_dir,
        content_dir,
        site_dir,
        template_dir,
        None,
    )
}

/// Compiles the static site, deriving a `permalink:` for every staged
/// markdown page that declares neither `permalink` nor `url` when
/// `base_url` is provided (spec A2/B1, plan §2 item 1.2, issue #586).
///
/// The derived value is [`crate::urls::derive_permalink`] applied to
/// `(base_url, content_rel_path)` —
/// i.e. the pretty directory URL of the page's compiled output — so
/// the injected permalink, the canonical `<link>`, and the feed
/// `<link>` all come from one code path
/// ([`crate::urls::derive_page_url`]). This makes `rss-gen`'s
/// "channel.link is missing" hard-fail unreachable for pages without
/// author-specified permalinks.
///
/// Passing `base_url: None` (or an empty string) skips the permalink
/// derivation entirely and behaves like [`compile_site`].
///
/// # Examples
///
/// ```no_run
/// use ssg::pipeline::compile_site_with_base_url;
/// use std::path::Path;
///
/// // Real call requires populated content/template trees; only the
/// // signature is exercised here.
/// let _ = compile_site_with_base_url(
///     Path::new("build"), Path::new("content"),
///     Path::new("site"), Path::new("templates"),
///     Some("https://example.com"),
/// );
/// ```
pub fn compile_site_with_base_url(
    build_dir: &Path,
    content_dir: &Path,
    site_dir: &Path,
    template_dir: &Path,
    base_url: Option<&str>,
) -> Result<(), SsgError> {
    compile_site_with_locales(
        build_dir,
        content_dir,
        site_dir,
        template_dir,
        base_url,
        &[],
    )
}

/// As [`compile_site_with_base_url`], plus the configured locales so the
/// content stager can derive `locale_path` / `locale_url` per page.
///
/// Separate from the public entry point so that signature stays stable for
/// embedders; the pipeline itself always calls this one.
pub fn compile_site_with_locales(
    build_dir: &Path,
    content_dir: &Path,
    site_dir: &Path,
    template_dir: &Path,
    base_url: Option<&str>,
    locales: &[String],
) -> Result<(), SsgError> {
    // v0.0.46: `staticdatagen 0.0.10` (closes upstream #67, #68, #69,
    // #70, #71) handles missing layout keys, absent aux files
    // (`main.js`/`sw.js`), absent tags-page templates, nested locale
    // walk, and success-log ordering natively — three of the v0.0.45
    // stager shims were retired in this release. The two surviving
    // shims:
    //
    //   * `collect_template_vars` + `stage_content_with_site_defaults`
    //     pre-fill empty `key: ""` frontmatter entries for every
    //     `{{ var }}` reference the templates make. staticweaver
    //     0.0.3 has `with_lax_undefined(true)` (closes upstream
    //     staticweaver#28) but staticdatagen 0.0.10 doesn't yet opt
    //     into it. Tracked: <https://github.com/sebastienrousseau/staticdatagen/issues/99>.
    //
    //   * The same staging pass also collapses multi-line double-quoted
    //     YAML scalars before staticdatagen sees them. `metadata-gen 0.0.5`
    //     (closes upstream metadata-gen#20) handles this natively,
    //     but staticdatagen 0.0.10 still pins `metadata-gen = "0.0.4"`.
    //     Tracked: <https://github.com/sebastienrousseau/staticdatagen/issues/100>.
    //
    // Once those two upstream follow-ups land, the residual shim
    // collapses to ~50 LOC.
    //
    // The same staging pass also threads `base_url` through so the
    // stager can inject a derived `permalink:` for pages that declare
    // neither `permalink` nor `url` (spec A2/B1, plan §2 item 1.2,
    // issue #586).
    let template_vars =
        crate::content_stager::collect_template_vars(template_dir)
            .map_err(|e| SsgError::io(e, template_dir))?;

    let staged_content =
        crate::content_stager::stage_content_with_site_defaults(
            content_dir,
            build_dir,
            &template_vars,
            base_url,
            locales,
        )
        .map_err(|e| SsgError::io(e, content_dir))?;

    // `compile` reads the StaticWeaver templates from the root of the
    // template directory, choosing one per page from its layout. Which
    // of them a given site needs therefore depends on its content —
    // a project with only `page.html` builds fine — so this is not a
    // precondition that can be checked up front.
    //
    // What it is is undiagnosable when it fails. StaticWeaver surfaces a
    // bare `No such file or directory` carrying no path, and the wrapper
    // could only name `build_dir`: the directory being written *to*. A
    // user following `examples/basic`, which shipped without templates,
    // was told `I/O error at 'public.build-tmp'` about a file under
    // `templates/` that had never existed (issue #752, and again on a
    // hand-made project in v0.0.60).
    //
    // So keep the behaviour and fix the diagnosis: on a not-found,
    // report the template directory and which of the usual four are
    // absent from it.
    compile(build_dir, &staged_content, site_dir, template_dir).map_err(
        |e| {
            eprintln!("    Error compiling site: {e:?}");
            // Portability: the not-found test must not be a match on
            // English Unix wording. Windows says "The system cannot
            // find the file specified.", so `contains("No such file or
            // directory")` silently never fired there and the
            // diagnostic below was dead on that platform — caught by
            // the windows-latest leg of CI, not by review.
            //
            // Prefer the error kind, which is platform-independent.
            // Fall back to the platform's *own* text for ENOENT, taken
            // from the OS at runtime rather than hardcoded, for the
            // case where the chain has flattened the io::Error into a
            // formatted string.
            let enoent = std::io::Error::from_raw_os_error(2).to_string();
            // "No such file or directory (os error 2)" on Unix; "The
            // system cannot find the file specified. (os error 2)" on
            // Windows. Compare on the prose alone — the numeric suffix
            // is not guaranteed to survive re-formatting.
            let enoent_prose =
                enoent.split(" (os error").next().unwrap_or(&enoent);
            let not_found = e.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
            }) || format!("{e:?}").contains(enoent_prose);
            if not_found {
                const USUAL_ROOT_TEMPLATES: [&str; 4] =
                    ["template.html", "index.html", "page.html", "post.html"];
                let absent: Vec<&str> = USUAL_ROOT_TEMPLATES
                    .iter()
                    .copied()
                    .filter(|name| !template_dir.join(name).is_file())
                    .collect();
                if !absent.is_empty() {
                    return SsgError::Validation {
                        field: "templates".to_string(),
                        message: format!(
                            "the compile step could not read a template. \
                             {} does not contain {}. A site needs the \
                             templates its content asks for at the template \
                             directory root, plus the MiniJinja set under \
                             `{}`. Run `ssg --new <name>` to scaffold a \
                             project with both, or copy them from \
                             `examples/basic/templates/`.",
                            template_dir.display(),
                            absent.join(", "),
                            template_dir.join("tera").display(),
                        ),
                    };
                }
            }
            SsgError::io(
                std::io::Error::other(format!("Failed to compile site: {e:?}")),
                build_dir,
            )
        },
    )?;

    // Copy any static assets from template_dir (e.g. styles.css, theme-init.js, favicon.ico, images)
    // to site_dir so they are available in public/ and fingerprinted by assets plugin.
    copy_static_template_assets(template_dir, site_dir)?;
    if let Some(parent) = template_dir.parent() {
        let assets_dir = parent.join("assets");
        if assets_dir.is_dir() {
            let site_assets = site_dir.join("assets");
            let _ = std::fs::create_dir_all(&site_assets);
            copy_static_template_assets(&assets_dir, &site_assets)?;
        }
    }
    Ok(())
}

fn copy_static_template_assets(src: &Path, dst: &Path) -> Result<(), SsgError> {
    if !src.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(src).map_err(|e| SsgError::io(e, src))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(
                ext,
                "css"
                    | "js"
                    | "ico"
                    | "svg"
                    | "png"
                    | "jpg"
                    | "jpeg"
                    | "webp"
                    | "avif"
                    | "woff"
                    | "woff2"
                    | "ttf"
                    | "json"
                    | "map"
            ) && !name_str.ends_with(".tera.html")
            {
                let target = dst.join(name);
                let _ = std::fs::copy(&path, &target);
            }
        } else if path.is_dir()
            && name_str != "tera"
            && !name_str.starts_with('.')
        {
            let target_dir = dst.join(name);
            let _ = std::fs::create_dir_all(&target_dir);
            let _ = copy_static_template_assets(&path, &target_dir);
        }
    }
    Ok(())
}

/// Registers the default plugin pipeline.
///
/// Plugins execute in registration order. The ordering is:
/// 1. SEO plugins (meta tags, canonical URLs, robots.txt)
/// 2. Search index generation
/// 3. HTML minification — last in registration order, but note that
///    `after_compile` hooks all run *before* any `transform_html`, so
///    minification precedes the fused transform pass rather than
///    following it (see the note at its registration below)
/// 4. Live reload (`on_serve` only)
///
/// # Examples
///
/// ```rust
/// use ssg::cmd::SsgConfig;
/// use ssg::pipeline::register_default_plugins;
/// use ssg::plugin::PluginManager;
///
/// let cfg = SsgConfig::default();
/// let mut pm = PluginManager::new();
/// register_default_plugins(&mut pm, &cfg, false, None);
/// assert!(pm.len() > 0);
/// ```
pub fn register_default_plugins(
    plugins: &mut plugin::PluginManager,
    config: &SsgConfig,
    include_drafts: bool,
    deploy_target: Option<&str>,
) {
    let base_url = config.base_url.clone();

    // Before-compile plugins
    plugins.register(content::ContentValidationPlugin);
    plugins.register(drafts::DraftPlugin::new(include_drafts));
    plugins.register(shortcodes::ShortcodePlugin);

    // Template engine (must run first in after_compile)
    #[cfg(feature = "templates")]
    plugins.register(
        crate::template_plugin::TemplatePlugin::from_template_dir(
            &config.template_dir,
        ),
    );

    // Post-processing fixes for staticdatagen output (run early,
    // before SEO plugins read/modify the HTML)
    plugins.register(postprocess::SitemapFixPlugin);
    plugins.register(postprocess::NewsSitemapFixPlugin);
    plugins.register(postprocess::RssAggregatePlugin);
    plugins.register(postprocess::AtomFeedPlugin);
    plugins.register(postprocess::JsonFeedPlugin);
    plugins.register(postprocess::ManifestFixPlugin);
    plugins.register(postprocess::HtmlFixPlugin);
    // `postprocess::SbomPlugin` ("sbom-generator") is deliberately not
    // registered. It wrote the same `sbom.cdx.json` as `crate::sbom::SbomPlugin`
    // ("sbom"), which registers later and therefore overwrote it — the build
    // serialised the dependency tree twice and threw one copy away. The
    // surviving plugin is the more complete of the two: it also injects the
    // `<link rel="sbom">` into every document head.

    // Agentic discovery (#552): agents.txt + .well-known/ai-plugin.json
    // + .well-known/mcp.json. No-op when `[agents]` is absent from
    // `ssg.toml`, so existing sites see no behavioural change.
    plugins.register(postprocess::AgenticDiscoveryPlugin);

    // Syntax highlighting
    plugins.register(highlight::HighlightPlugin::default());

    // SEO plugins
    plugins.register(seo::SeoPlugin);
    plugins
        .register(seo::JsonLdPlugin::from_site(&base_url, &config.site_name));
    plugins.register(seo::CanonicalPlugin::new(base_url.clone()));
    plugins.register(seo::RobotsPlugin::new(base_url));

    // AI readiness
    plugins.register(ai::AiPlugin);

    // Agent JSON API (#586 port 3): /api/agents/{index,posts,topics,
    // person}.json. Default-on like AiPlugin; programmatic opt-out via
    // AgentApiPlugin::disabled(). (The oEmbed emitter — port 4 — is
    // opt-in and therefore NOT registered here; see crate::oembed.)
    plugins.register(crate::agent_api::AgentApiPlugin::default());

    // Taxonomy and pagination
    plugins.register(taxonomy::TaxonomyPlugin);
    plugins.register(pagination::PaginationPlugin::default());

    // Search & optimization
    plugins.register(search::SearchPlugin);

    // Accessibility validation
    plugins.register(accessibility::AccessibilityPlugin);

    // Master Quality Gate & Compliance Audit
    plugins.register(crate::plugins_group::audit::AuditPlugin);

    // Image optimization (WebP, responsive srcset)
    #[cfg(feature = "image-optimization")]
    plugins.register(crate::image_plugin::ImageOptimizationPlugin::default());

    // I18n hreflang injection and per-locale sitemaps
    if let Some(ref i18n_cfg) = config.i18n {
        if i18n_cfg.locales.len() > 1 {
            plugins.register(i18n::I18nPlugin::new(i18n_cfg.clone()));
        }
    }

    // Interactive islands (Web Components)
    plugins.register(islands::IslandPlugin);

    // View Transitions API + lazy-nav client (issue #547, opt-in).
    // Registered after islands so the transitions client can call the
    // `<ssg-island>` `detach()` hook on the outgoing page.
    if crate::view_transitions::ViewTransitionsPlugin::enabled(config) {
        plugins.register(crate::view_transitions::ViewTransitionsPlugin::new());
    }

    // CSP hardening: extract inline styles/scripts to external files with SRI
    plugins.register(csp::CspPlugin);

    // SBOM emission + per-page link (resolves #457). Runs before
    // FingerprintPlugin so the SBOM filename itself isn't subject to
    // content-hash renaming (consumers fetch a stable URL).
    plugins.register(crate::sbom::SbomPlugin);

    // Asset fingerprinting + SRI (after all content transforms)
    plugins.register(assets::FingerprintPlugin);

    // Minification. Registered last, but that does not make it run last:
    // MinifyPlugin only implements `after_compile`, and every
    // `after_compile` hook runs before any `transform_html`, so it
    // rewrites markup that later transforms then read.
    //
    // Two consequences worth knowing before relying on ordering here:
    //
    //   - Without the `minify` feature the walk is top-level only, so
    //     nested pages are untouched and the two halves of a site are
    //     minified inconsistently.
    //   - `html-generator` minifies some pages during generation, before
    //     any plugin runs at all, which no plugin ordering can affect.
    //     That is why the i18n language-switcher marker is an element
    //     rather than a comment: comments do not survive it.
    //
    // Moving it into a post-transform phase was tried and reverted: it
    // changes the observable behaviour of the public `run_after_compile`,
    // which silently stopped minifying for every caller outside this
    // function. Doing it properly needs its own change with a migration
    // note.
    plugins.register(plugins_mod::MinifyPlugin);

    // Edge-runtime header emitter (issue #550). Opt-in via the
    // `[edge_headers] targets = [...]` section of ssg.toml; the
    // plugin is a no-op when targets is empty so unconditional
    // registration here is safe and keeps the wiring simple.
    plugins.register(postprocess::EdgeHeadersPlugin);

    // Deployment config generation (opt-in via --deploy flag)
    if let Some(target) = deploy_target {
        let dt = match target {
            "netlify" => Some(deploy::DeployTarget::Netlify),
            "vercel" => Some(deploy::DeployTarget::Vercel),
            "cloudflare" => Some(deploy::DeployTarget::CloudflarePages),
            "github" => Some(deploy::DeployTarget::GithubPages),
            _ => {
                log::warn!("Unknown deploy target: {target}");
                None
            }
        };
        if let Some(dt) = dt {
            plugins.register(deploy::DeployPlugin::new(dt));
        }
    }

    // Dev server
    plugins.register(livereload::LiveReloadPlugin::default());
}

#[cfg(test)]
mod tests {

    /// The default pipeline must register each plugin name once. Two
    /// `SbomPlugin` implementations were both registered before 0.0.58: they
    /// wrote the same `sbom.cdx.json`, so the later one silently overwrote the
    /// earlier and the dependency tree was serialised twice per build.
    #[test]
    fn default_plugins_have_no_duplicate_names() {
        let config = SsgConfig::default();
        let mut plugins = plugin::PluginManager::new();
        register_default_plugins(&mut plugins, &config, false, None);

        let mut seen = std::collections::BTreeMap::new();
        for info in plugins.inventory() {
            *seen.entry(info.name).or_insert(0usize) += 1;
        }
        let dupes: Vec<_> = seen
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|(k, _)| *k)
            .collect();
        assert!(dupes.is_empty(), "duplicate plugin names: {dupes:?}");
    }

    /// WS0.5's acceptance condition: "`ssg plugins list` shows exactly one
    /// Deploy and one SBOM plugin".
    ///
    /// Without a target the deploy plugin does not register at all, which is
    /// correct for a plain build but means the deploy stage cannot be
    /// inspected. `--target` mirrors what `ssg deploy` would register.
    #[test]
    fn one_deploy_plugin_registers_when_a_target_is_given() {
        let config = SsgConfig::default();

        let mut without = plugin::PluginManager::new();
        register_default_plugins(&mut without, &config, false, None);
        assert!(
            !without
                .inventory()
                .iter()
                .any(|p| p.name.contains("deploy")),
            "a plain build should register no deploy plugin"
        );

        let mut with = plugin::PluginManager::new();
        register_default_plugins(&mut with, &config, false, Some("netlify"));
        let deploy: Vec<_> = with
            .inventory()
            .into_iter()
            .filter(|p| p.name.contains("deploy"))
            .collect();
        assert_eq!(
            deploy.len(),
            1,
            "expected exactly one deploy plugin, got {deploy:?}"
        );
        assert_eq!(with.len(), without.len() + 1);
    }

    /// Exactly one SBOM emitter, and it is the one that also links the
    /// document head — see `postprocess::SbomPlugin`'s deprecation note.
    #[test]
    fn exactly_one_sbom_plugin_is_registered() {
        let config = SsgConfig::default();
        let mut plugins = plugin::PluginManager::new();
        register_default_plugins(&mut plugins, &config, false, None);

        let sbom: Vec<_> = plugins
            .inventory()
            .into_iter()
            .filter(|p| p.name.contains("sbom"))
            .collect();
        assert_eq!(sbom.len(), 1, "expected one SBOM plugin, got {sbom:?}");
        assert_eq!(sbom[0].name, "sbom");
    }

    /// The inventory is ordered, and that order is execution order — the
    /// property `ssg plugins list` reports and the README count derives from.
    #[test]
    fn inventory_is_in_registration_order() {
        let config = SsgConfig::default();
        let mut plugins = plugin::PluginManager::new();
        register_default_plugins(&mut plugins, &config, false, None);

        let inv = plugins.inventory();
        assert!(!inv.is_empty());
        for (i, info) in inv.iter().enumerate() {
            assert_eq!(info.order, i);
        }
        assert_eq!(inv.len(), plugins.len());
    }
    use super::*;

    #[test]
    fn test_build_error_serialization() {
        let err = BuildError {
            file: Some("content/post.md".to_string()),
            line: Some(42),
            message: "unexpected token".to_string(),
        };
        let json = err.to_ws_message();
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["file"], "content/post.md");
        assert_eq!(parsed["line"], 42);
        assert_eq!(parsed["message"], "unexpected token");
    }

    #[test]
    fn test_clear_error_message() {
        let msg = clear_error_message();
        let parsed: serde_json::Value =
            serde_json::from_str(&msg).expect("valid JSON");
        assert_eq!(parsed["type"], "clear-error");
    }

    #[test]
    fn test_extract_file_from_error_md() {
        let msg = "cannot read content/posts/hello.md: permission denied";
        assert_eq!(
            extract_file_from_error(msg),
            Some("content/posts/hello.md".to_string())
        );
    }

    #[test]
    fn test_extract_file_from_error_html() {
        let msg = "template error in templates/base.html";
        assert_eq!(
            extract_file_from_error(msg),
            Some("templates/base.html".to_string())
        );
    }

    #[test]
    fn test_extract_file_from_error_toml() {
        let msg = "parse error in config/site.toml at line 5";
        assert_eq!(
            extract_file_from_error(msg),
            Some("config/site.toml".to_string())
        );
    }

    #[test]
    fn test_extract_file_from_error_none() {
        let msg = "something went wrong with no file path";
        assert_eq!(extract_file_from_error(msg), None);
    }

    #[test]
    fn test_build_error_from_error() {
        let err = SsgError::Io {
            path: PathBuf::from("output/index.html"),
            source: std::io::Error::other("disk full"),
        };
        let be = BuildError::from_error(&err);
        assert_eq!(be.file, Some("output/index.html".to_string()));
        assert!(be.line.is_none());
        assert!(be.message.contains("disk full"));
    }

    // -----------------------------------------------------------------
    // BuildError — additional coverage
    // -----------------------------------------------------------------

    #[test]
    fn test_build_error_no_file_no_line() {
        let err = BuildError {
            file: None,
            line: None,
            message: "something broke".to_string(),
        };
        let json = err.to_ws_message();
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed["type"], "error");
        assert!(parsed["file"].is_null());
        assert!(parsed["line"].is_null());
        assert_eq!(parsed["message"], "something broke");
    }

    #[test]
    fn test_build_error_clone() {
        let err = BuildError {
            file: Some("a/b.md".to_string()),
            line: Some(10),
            message: "oops".to_string(),
        };
        let cloned = err.clone();
        assert_eq!(cloned.file, err.file);
        assert_eq!(cloned.line, err.line);
        assert_eq!(cloned.message, err.message);
    }

    #[test]
    fn test_build_error_debug() {
        let err = BuildError {
            file: None,
            line: None,
            message: "debug test".to_string(),
        };
        let debug = format!("{err:?}");
        assert!(debug.contains("BuildError"));
        assert!(debug.contains("debug test"));
    }

    #[test]
    fn test_build_error_from_error_no_file() {
        let err = SsgError::Core(ssg_core::Error::FrontmatterParse {
            syntax: "generic error without any file path".to_string(),
        });
        let be = BuildError::from_error(&err);
        assert!(be.file.is_none());
        assert!(be.message.contains("generic error"));
    }

    #[test]
    fn test_build_error_from_error_yml_extension() {
        let err = SsgError::Io {
            path: PathBuf::from("config/site.yml"),
            source: std::io::Error::other("parse error"),
        };
        let be = BuildError::from_error(&err);
        assert_eq!(be.file, Some("config/site.yml".to_string()));
    }

    #[test]
    fn test_build_error_from_error_yaml_extension() {
        let err = SsgError::Io {
            path: PathBuf::from("data/settings.yaml"),
            source: std::io::Error::other("error at line 3"),
        };
        let be = BuildError::from_error(&err);
        assert_eq!(be.file, Some("data/settings.yaml".to_string()));
    }

    // -----------------------------------------------------------------
    // extract_file_from_error — additional coverage
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_file_with_punctuation_around_path() {
        let msg = "error: 'templates/base.html' not found";
        let result = extract_file_from_error(msg);
        assert_eq!(result, Some("templates/base.html".to_string()));
    }

    #[test]
    fn test_extract_file_no_slash_in_word() {
        let msg = "file not found: base.html";
        let result = extract_file_from_error(msg);
        assert!(result.is_none(), "no slash means no file path extraction");
    }

    #[test]
    fn test_extract_file_multiple_paths_returns_first() {
        let msg = "failed to read src/a.md and src/b.html";
        let result = extract_file_from_error(msg);
        assert_eq!(result, Some("src/a.md".to_string()));
    }

    #[test]
    fn test_extract_file_toml_with_trailing_colon() {
        let msg = "invalid key in config/site.toml: 'foo'";
        let result = extract_file_from_error(msg);
        assert_eq!(result, Some("config/site.toml".to_string()));
    }

    // -----------------------------------------------------------------
    // clear_error_message — sanity
    // -----------------------------------------------------------------

    #[test]
    fn test_clear_error_message_is_valid_json() {
        let msg = clear_error_message();
        let parsed: serde_json::Value =
            serde_json::from_str(&msg).expect("valid JSON");
        assert_eq!(parsed["type"], "clear-error");
        // Ensure no extra keys leak
        assert_eq!(parsed.as_object().unwrap().len(), 1);
    }

    // -----------------------------------------------------------------
    // resolve_build_and_site_dirs — coverage from pipeline module
    // -----------------------------------------------------------------

    #[test]
    fn test_resolve_dirs_no_serve_dir() {
        use crate::cmd::SsgConfig;
        use std::path::PathBuf;
        let mut config = SsgConfig::default();
        config.output_dir = PathBuf::from("out");
        config.serve_dir = None;

        let (build, site) = resolve_build_and_site_dirs(&config);
        assert_eq!(site, PathBuf::from("out"));
        // build should differ from site
        assert_ne!(build, site);
    }

    #[test]
    fn test_resolve_dirs_serve_differs_from_output() {
        use crate::cmd::SsgConfig;
        use std::path::PathBuf;
        let mut config = SsgConfig::default();
        config.output_dir = PathBuf::from("build");
        config.serve_dir = Some(PathBuf::from("public"));

        let (build, site) = resolve_build_and_site_dirs(&config);
        assert_eq!(build, PathBuf::from("build"));
        assert_eq!(site, PathBuf::from("public"));
    }

    #[test]
    fn test_resolve_dirs_serve_equals_output() {
        use crate::cmd::SsgConfig;
        use std::path::PathBuf;
        let mut config = SsgConfig::default();
        config.output_dir = PathBuf::from("dist");
        config.serve_dir = Some(PathBuf::from("dist"));

        let (build, site) = resolve_build_and_site_dirs(&config);
        assert_eq!(site, PathBuf::from("dist"));
        assert_ne!(build, site);
        assert!(build.to_string_lossy().contains("build-tmp"));
    }

    // -----------------------------------------------------------------
    // RunOptions — construction from matches
    // -----------------------------------------------------------------

    #[test]
    fn test_run_options_defaults() {
        use crate::cmd::Cli;
        let cli = Cli::build();
        let matches = cli.try_get_matches_from(vec!["ssg"]).unwrap();
        let opts = RunOptions::from_matches(&matches);

        assert!(!opts.quiet);
        assert!(!opts.include_drafts);
        assert!(opts.deploy_target.is_none());
        assert!(!opts.validate_only);
        assert!(opts.jobs.is_none());
        assert!(opts.max_memory_mb.is_none());
        assert!(!opts.ai_fix);
        assert!(!opts.ai_fix_dry_run);
    }

    #[test]
    fn test_run_options_ai_fix_flags() {
        use crate::cmd::Cli;
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec!["ssg", "--ai-fix", "--ai-fix-dry-run"])
            .unwrap();
        let opts = RunOptions::from_matches(&matches);
        assert!(opts.ai_fix);
        assert!(opts.ai_fix_dry_run);
    }

    #[test]
    fn test_run_options_from_matches_incremental_no_llm_cache_isr_flags() {
        // `from_matches`'s `incremental` / `no_llm_cache` / `isr` fields
        // each short-circuit on `try_contains_id`; the legacy `Cli`
        // defines all three ids, so this drives the true-arm of every
        // `&&` (the id is present *and* the flag was actually passed).
        use crate::cmd::Cli;
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec![
                "ssg",
                "--incremental",
                "--no-llm-cache",
                "--isr",
            ])
            .unwrap();
        let opts = RunOptions::from_matches(&matches);
        assert!(opts.incremental);
        assert!(opts.no_llm_cache);
        assert!(opts.isr);
    }

    #[test]
    fn test_run_options_debug() {
        use crate::cmd::Cli;
        let cli = Cli::build();
        let matches = cli.try_get_matches_from(vec!["ssg"]).unwrap();
        let opts = RunOptions::from_matches(&matches);
        let debug = format!("{opts:?}");
        assert!(debug.contains("RunOptions"));
        assert!(debug.contains("quiet"));
    }

    #[test]
    fn test_run_options_clone() {
        use crate::cmd::Cli;
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec!["ssg", "--quiet", "--jobs", "2"])
            .unwrap();
        let opts = RunOptions::from_matches(&matches);
        let cloned = opts.clone();
        assert_eq!(cloned.quiet, opts.quiet);
        assert_eq!(cloned.jobs, opts.jobs);
    }

    // -----------------------------------------------------------------
    // register_default_plugins — plugin count and ordering
    // -----------------------------------------------------------------

    #[test]
    fn test_register_default_plugins_minimum_count() {
        use crate::cmd::SsgConfig;
        use crate::plugin::PluginManager;

        let config = SsgConfig::default();
        let mut pm = PluginManager::new();
        register_default_plugins(&mut pm, &config, false, None);

        // We expect a substantial number of default plugins
        let count = pm.len();
        assert!(
            count >= 15,
            "expected at least 15 default plugins, got {count}"
        );
    }

    #[test]
    fn test_register_default_plugins_includes_key_plugins() {
        use crate::cmd::SsgConfig;
        use crate::plugin::PluginManager;

        let config = SsgConfig::default();
        let mut pm = PluginManager::new();
        register_default_plugins(&mut pm, &config, false, None);

        let names = pm.names();
        assert!(names.contains(&"content-validation"));
        assert!(names.contains(&"drafts"));
        assert!(names.contains(&"shortcodes"));
        assert!(names.contains(&"seo"));
        assert!(names.contains(&"search"));
        assert!(names.contains(&"minify"));
        assert!(names.contains(&"livereload"));
    }

    #[test]
    fn test_register_default_plugins_with_deploy_adds_deploy_plugin() {
        use crate::cmd::SsgConfig;
        use crate::plugin::PluginManager;

        let config = SsgConfig::default();
        let mut pm_without = PluginManager::new();
        register_default_plugins(&mut pm_without, &config, false, None);
        let count_without = pm_without.len();

        let mut pm_with = PluginManager::new();
        register_default_plugins(&mut pm_with, &config, false, Some("netlify"));

        assert_eq!(pm_with.len(), count_without + 1);
        assert!(pm_with.names().contains(&"deploy"));
    }

    #[test]
    fn test_register_default_plugins_unknown_deploy_skipped() {
        use crate::cmd::SsgConfig;
        use crate::plugin::PluginManager;

        let config = SsgConfig::default();
        let mut pm = PluginManager::new();
        register_default_plugins(
            &mut pm,
            &config,
            false,
            Some("nonexistent-platform"),
        );

        assert!(
            !pm.names().contains(&"deploy"),
            "unknown deploy target should not register a deploy plugin"
        );
    }

    // -----------------------------------------------------------------
    // build_pipeline — basic wiring
    // -----------------------------------------------------------------

    #[test]
    fn test_build_pipeline_returns_valid_dirs() {
        use crate::cmd::SsgConfig;

        let temp = tempfile::tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("content");
        config.output_dir = temp.path().join("public");
        config.template_dir = temp.path().join("templates");

        let opts = RunOptions {
            quiet: true,
            include_drafts: false,
            deploy_target: None,
            validate_only: false,
            jobs: None,
            max_memory_mb: None,
            ai_fix: false,
            ai_fix_dry_run: false,
            incremental: false,
            no_llm_cache: false,
            isr: false,
        };

        let (plugins, ctx, build_dir, site_dir) =
            build_pipeline(&config, &opts);

        assert!(!plugins.is_empty());
        assert_ne!(build_dir, site_dir);
        assert_eq!(ctx.content_dir, temp.path().join("content"));
    }

    // -----------------------------------------------------------------
    // RunOptions::from_subcommand_matches — populated values
    // -----------------------------------------------------------------

    #[test]
    fn test_run_options_from_subcommand_reads_max_memory() {
        use crate::cmd::Cli;
        let matches = Cli::subcommand_app().get_matches_from(vec![
            "ssg",
            "build",
            "--max-memory",
            "64",
        ]);
        let sub_m = matches.subcommand_matches("build").unwrap();
        let opts = RunOptions::from_subcommand_matches(sub_m);
        assert_eq!(opts.max_memory_mb, Some(64));
    }

    // -----------------------------------------------------------------
    // build_pipeline — env export, ISR registration
    // -----------------------------------------------------------------

    #[test]
    fn test_build_pipeline_no_llm_cache_exports_env_flag() {
        // Serialised env-var scoping (mirrors the llm.rs pattern).
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("SSG_NO_LLM_CACHE").ok();
        std::env::remove_var("SSG_NO_LLM_CACHE");

        let config = SsgConfig::default();
        let opts = RunOptions {
            no_llm_cache: true,
            ..RunOptions::default()
        };
        let (plugins, _ctx, _build, _site) = build_pipeline(&config, &opts);
        let seen = std::env::var("SSG_NO_LLM_CACHE").ok();

        // Restore machine state before asserting.
        match prev {
            Some(v) => std::env::set_var("SSG_NO_LLM_CACHE", v),
            None => std::env::remove_var("SSG_NO_LLM_CACHE"),
        }
        assert_eq!(seen.as_deref(), Some("1"));
        assert!(!plugins.is_empty());
    }

    #[test]
    fn test_register_isr_plugins_appends_isr_pair() {
        use crate::plugin::PluginManager;
        let mut pm = PluginManager::new();
        register_isr_plugins(&mut pm);
        assert_eq!(pm.len(), 2, "ISR manifest + RPC schema plugins");
    }

    #[test]
    fn test_build_pipeline_isr_flag_appends_plugins() {
        let config = SsgConfig::default();
        let base = build_pipeline(&config, &RunOptions::default()).0.len();
        let opts = RunOptions {
            isr: true,
            ..RunOptions::default()
        };
        let with_isr = build_pipeline(&config, &opts).0.len();
        assert_eq!(with_isr, base + 2);
    }

    // -----------------------------------------------------------------
    // register_default_plugins — conditional registrations
    // -----------------------------------------------------------------

    #[test]
    fn test_register_default_plugins_multi_locale_adds_i18n() {
        use crate::plugin::PluginManager;
        let mut config = SsgConfig::default();
        config.i18n = Some(i18n::I18nConfig {
            default_locale: "en".to_string(),
            locales: vec!["en".to_string(), "fr".to_string()],
            url_prefix: Default::default(),
        });

        let mut pm = PluginManager::new();
        register_default_plugins(&mut pm, &config, false, None);
        assert!(
            pm.names().contains(&"i18n"),
            "two locales must register the i18n plugin: {:?}",
            pm.names()
        );
    }

    #[test]
    fn test_register_default_plugins_single_locale_skips_i18n() {
        use crate::plugin::PluginManager;
        let mut config = SsgConfig::default();
        config.i18n = Some(i18n::I18nConfig::default());

        let mut pm = PluginManager::new();
        register_default_plugins(&mut pm, &config, false, None);
        assert!(!pm.names().contains(&"i18n"));
    }

    #[test]
    fn test_register_default_plugins_transitions_opt_in() {
        use crate::plugin::PluginManager;
        let mut config = SsgConfig::default();
        config.transitions = true;

        let mut pm = PluginManager::new();
        register_default_plugins(&mut pm, &config, false, None);
        assert!(pm.names().contains(&"view-transitions"));
    }

    // -----------------------------------------------------------------
    // depgraph_cache_root — no-target fallback
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::serial(cwd)]
    fn test_depgraph_cache_root_falls_back_without_target_dir() {
        // From a cwd without a `target/` directory the cache root
        // lands under the site dir.
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().expect("read current dir");
        std::env::set_current_dir(tmp.path()).expect("pushd");

        let root = depgraph_cache_root(Path::new("/tmp/site"));

        std::env::set_current_dir(&prev).expect("popd");
        assert_eq!(root, Path::new("/tmp/site").join(".ssg-cache"));
    }

    // -----------------------------------------------------------------
    // compile_site_with_base_url — template-collection failure
    // -----------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn test_compile_maps_unreadable_template_dir_to_io_error() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let content = tmp.path().join("content");
        let build = tmp.path().join("build");
        let site = tmp.path().join("public");
        let templates = tmp.path().join("templates");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::set_permissions(
            &templates,
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        let res = compile_site_with_base_url(
            &build, &content, &site, &templates, None,
        );

        let _ = std::fs::set_permissions(
            &templates,
            std::fs::Permissions::from_mode(0o755),
        );
        // Root bypasses permissions on some CI runners, so tolerate Ok.
        assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
    }

    // -----------------------------------------------------------------
    // execute_build_pipeline_with — plugin failures, streaming,
    // incremental fast path, and non-fatal cache warnings
    // -----------------------------------------------------------------

    /// Minimal compilable site fixture (mirrors
    /// tests/core/pipeline.rs): two pages + one template.
    fn build_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf)
    {
        crate::test_support::init_logger();
        let tmp = tempfile::tempdir().expect("tempdir");
        let content = tmp.path().join("content");
        let build = tmp.path().join("build");
        let site = tmp.path().join("public");
        let templates = tmp.path().join("templates");
        std::fs::create_dir_all(&content).expect("mkdir content");
        std::fs::create_dir_all(&templates).expect("mkdir templates");
        std::fs::create_dir_all(&build).expect("mkdir build");
        std::fs::write(
            content.join("index.md"),
            "---\ntitle: \"Home\"\ndescription: \"home\"\n\
             permalink: \"https://example.com/\"\n---\nhome body",
        )
        .expect("write index.md");
        std::fs::write(
            content.join("about.md"),
            "---\ntitle: \"About\"\ndescription: \"about\"\n\
             permalink: \"https://example.com/about/\"\n---\nabout body",
        )
        .expect("write about.md");
        std::fs::write(
            templates.join("page.html"),
            "<!doctype html><html><body>{{ content }}</body></html>",
        )
        .expect("write template");
        (tmp, content, build, site, templates)
    }

    /// Test plugin that fails in exactly one pipeline phase.
    #[derive(Debug)]
    struct FailingPlugin {
        phase: &'static str,
    }

    impl plugin::Plugin for FailingPlugin {
        fn name(&self) -> &'static str {
            "failing-test-plugin"
        }
        fn before_compile(
            &self,
            _ctx: &plugin::PluginContext,
        ) -> Result<(), SsgError> {
            if self.phase == "before" {
                return Err(SsgError::Validation {
                    field: "test".to_string(),
                    message: "injected before_compile failure".to_string(),
                });
            }
            Ok(())
        }
        fn after_compile(
            &self,
            _ctx: &plugin::PluginContext,
        ) -> Result<(), SsgError> {
            if self.phase == "after" {
                return Err(SsgError::Validation {
                    field: "test".to_string(),
                    message: "injected after_compile failure".to_string(),
                });
            }
            Ok(())
        }
        fn has_transform(&self) -> bool {
            self.phase == "transform"
        }
        fn transform_html(
            &self,
            _html: &str,
            _path: &Path,
            _ctx: &plugin::PluginContext,
        ) -> Result<String, SsgError> {
            Err(SsgError::Validation {
                field: "test".to_string(),
                message: "injected transform failure".to_string(),
            })
        }
    }

    /// Test plugin that sabotages the site dir *after* compile, so
    /// the post-build cache bookkeeping hits its non-fatal error arms
    /// (the compile step recreates the site dir wholesale, so
    /// obstructions must be planted from inside the pipeline).
    #[derive(Debug)]
    struct SabotagePlugin {
        mode: &'static str,
    }

    impl plugin::Plugin for SabotagePlugin {
        fn name(&self) -> &'static str {
            "sabotage-test-plugin"
        }
        fn after_compile(
            &self,
            ctx: &plugin::PluginContext,
        ) -> Result<(), SsgError> {
            if self.mode == "block-plugin-cache" {
                let _ = std::fs::create_dir_all(
                    ctx.site_dir.join(".ssg-plugin-cache.json"),
                );
            }
            #[cfg(unix)]
            if self.mode == "lock-subdir" {
                use std::os::unix::fs::PermissionsExt;
                let locked = ctx.site_dir.join("locked");
                let _ = std::fs::create_dir_all(&locked);
                let _ = std::fs::set_permissions(
                    &locked,
                    std::fs::Permissions::from_mode(0o000),
                );
            }
            Ok(())
        }
    }

    fn run_fixture_with_plugins(
        pm: &plugin::PluginManager,
        incremental: bool,
    ) -> (tempfile::TempDir, PathBuf, Result<(), SsgError>) {
        let (tmp, content, build, site, templates) = build_fixture();
        let ctx =
            plugin::PluginContext::new(&content, &build, &site, &templates);
        let res = execute_build_pipeline_with(
            pm,
            &ctx,
            &build,
            &content,
            &site,
            &templates,
            true,
            incremental,
        );
        (tmp, site, res)
    }

    #[test]
    fn test_pipeline_propagates_before_compile_failure() {
        let mut pm = plugin::PluginManager::new();
        pm.register(FailingPlugin { phase: "before" });
        let (_tmp, _site, res) = run_fixture_with_plugins(&pm, false);
        assert!(res.is_err());
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn test_pipeline_propagates_after_compile_failure() {
        let mut pm = plugin::PluginManager::new();
        pm.register(FailingPlugin { phase: "after" });
        let (_tmp, _site, res) = run_fixture_with_plugins(&pm, false);
        assert!(res.is_err());
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn test_pipeline_propagates_transform_failure() {
        let mut pm = plugin::PluginManager::new();
        pm.register(FailingPlugin { phase: "transform" });
        let (_tmp, _site, res) = run_fixture_with_plugins(&pm, false);
        assert!(res.is_err());
    }

    #[test]
    #[serial_test::serial(ssg_cache, stager_fp)]
    fn test_pipeline_streams_when_budget_explicitly_set() {
        let (_tmp, content, build, site, templates) = build_fixture();
        let mut ctx =
            plugin::PluginContext::new(&content, &build, &site, &templates);
        // An explicit budget forces the streaming/batched compile.
        ctx.memory_budget = Some(streaming::MemoryBudget::from_mb(1));

        let pm = plugin::PluginManager::new();
        execute_build_pipeline_with(
            &pm, &ctx, &build, &content, &site, &templates, true, false,
        )
        .expect("streamed build should succeed");

        assert!(
            site.join("about").join("index.html").exists(),
            "batched compile must emit the page outputs"
        );
    }

    #[test]
    #[serial_test::serial(cwd, ssg_cache, stager_fp)]
    fn test_pipeline_incremental_fast_path_and_delete_sweep() {
        let (_tmp, content, build, site, templates) = build_fixture();
        let ctx =
            plugin::PluginContext::new(&content, &build, &site, &templates);
        let pm = plugin::PluginManager::new();

        // Fresh cache root so a previous test's graph can't leak in.
        let cache_root = depgraph_cache_root(&site);
        let _ = std::fs::remove_file(
            cache_root.join(crate::depgraph::DEP_GRAPH_FILE),
        );

        // Run 1: cold cache — full build, graph persisted.
        execute_build_pipeline_with(
            &pm, &ctx, &build, &content, &site, &templates, false, true,
        )
        .expect("cold incremental build should succeed");
        let about_out = site.join("about").join("index.html");
        assert!(about_out.exists());

        // Run 2: nothing changed — the fast path must skip the
        // compile entirely, so a marker planted in the output
        // survives verbatim.
        std::fs::write(&about_out, "MARKER").unwrap();
        execute_build_pipeline_with(
            &pm, &ctx, &build, &content, &site, &templates, false, true,
        )
        .expect("warm incremental build should succeed");
        assert_eq!(
            std::fs::read_to_string(&about_out).unwrap(),
            "MARKER",
            "fast path must not recompile unchanged sources"
        );

        // Run 3: delete a source — its stale output is swept and the
        // site is rebuilt without it.
        std::fs::remove_file(content.join("about.md")).unwrap();
        execute_build_pipeline_with(
            &pm, &ctx, &build, &content, &site, &templates, false, true,
        )
        .expect("incremental rebuild after delete should succeed");
        assert!(!about_out.exists(), "deleted source's output must be swept");
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial(ssg_cache, stager_fp)]
    fn test_pipeline_warns_but_succeeds_when_populate_fails() {
        // A dangling .md symlink survives staging (symlinks are
        // skipped) but makes depgraph::populate fail post-compile —
        // the build must still succeed with a warning.
        let (_tmp, content, build, site, templates) = build_fixture();
        std::os::unix::fs::symlink(
            content.join("nowhere.md"),
            content.join("ghost.md"),
        )
        .unwrap();
        let ctx =
            plugin::PluginContext::new(&content, &build, &site, &templates);
        let pm = plugin::PluginManager::new();

        execute_build_pipeline_with(
            &pm, &ctx, &build, &content, &site, &templates, true, false,
        )
        .expect("populate failure must be non-fatal");
    }

    #[test]
    #[serial_test::serial(cwd, ssg_cache, stager_fp)]
    fn test_pipeline_warns_but_succeeds_when_graph_save_fails() {
        // A directory squatting on the graph's tmp path makes
        // DepGraph::save fail — the build must still succeed.
        let (_tmp, content, build, site, templates) = build_fixture();
        let ctx =
            plugin::PluginContext::new(&content, &build, &site, &templates);
        let pm = plugin::PluginManager::new();

        // `depgraph_cache_root` returns `target/<CACHE_DIRNAME>` when a
        // `target/` directory exists relative to the *current* working
        // directory, and falls back to `site_dir/.ssg-cache` otherwise.
        // The build clears the site directory, so under the fallback the
        // blocker is swept away and the scenario this test exists to
        // cover cannot be constructed at all.
        //
        // Which branch is taken therefore depends on whether the
        // developer's cargo writes into `./target` — a global
        // `build.target-dir` in ~/.cargo/config.toml is enough to flip
        // it, and CI runners always have `./target`. Pin it here so the
        // test means the same thing everywhere.
        let cwd_tmp = tempfile::tempdir().expect("cwd tempdir");
        std::fs::create_dir_all(cwd_tmp.path().join("target"))
            .expect("create target dir");
        let prev_cwd = std::env::current_dir().expect("read current dir");
        std::env::set_current_dir(cwd_tmp.path()).expect("pushd");

        let cache_root = depgraph_cache_root(&site);
        let blocker =
            cache_root.join(format!("{}.tmp", crate::depgraph::DEP_GRAPH_FILE));
        std::fs::create_dir_all(&blocker).unwrap();
        std::fs::write(blocker.join("keep.txt"), "x").unwrap();
        assert!(
            !blocker.starts_with(&site),
            "cache root must sit outside the site dir the build clears"
        );

        let res = execute_build_pipeline_with(
            &pm, &ctx, &build, &content, &site, &templates, true, false,
        );

        let blocked = blocker.is_dir();
        let _ = std::fs::remove_dir_all(&blocker);
        std::env::set_current_dir(&prev_cwd).expect("popd");
        res.expect("graph-save failure must be non-fatal");
        assert!(blocked, "blocker must have survived the build");
    }

    #[test]
    #[serial_test::serial(ssg_cache, stager_fp)]
    fn test_pipeline_warns_but_succeeds_when_plugin_cache_save_fails() {
        // The sabotage plugin plants a directory on the
        // `.ssg-plugin-cache.json` path after compile, so
        // PluginCache::save fails — the build must still succeed.
        let mut pm = plugin::PluginManager::new();
        pm.register(SabotagePlugin {
            mode: "block-plugin-cache",
        });
        let (_tmp, site, res) = run_fixture_with_plugins(&pm, false);
        res.expect("plugin-cache save failure must be non-fatal");
        assert!(
            site.join(".ssg-plugin-cache.json").is_dir(),
            "blocker must be present for the warn arm to have fired"
        );
    }

    #[test]
    #[serial_test::serial(ssg_cache, stager_fp)]
    fn test_execute_build_pipeline_with_config_derives_base_url_for_non_streaming_compile(
    ) {
        // `ctx.config` is only ever `Some(..)` when built through
        // `PluginContext::with_config` (as `build_pipeline` wires it
        // up); the fixture-based tests elsewhere in this module use
        // `PluginContext::new`, which leaves it `None` and never runs
        // the `ctx.config.as_ref().map(|c| c.base_url.clone())` closure
        // in the non-streaming branch of `execute_build_pipeline_with`.
        use crate::cmd::SsgConfig;
        let (_tmp, content, build, site, templates) = build_fixture();
        let config = SsgConfig {
            base_url: "https://example.com".to_string(),
            ..SsgConfig::default()
        };
        let ctx = plugin::PluginContext::with_config(
            &content, &build, &site, &templates, config,
        );
        let pm = plugin::PluginManager::new();
        execute_build_pipeline_with(
            &pm, &ctx, &build, &content, &site, &templates, true, false,
        )
        .expect("build with a configured base_url should succeed");
        assert!(
            site.join("about").join("index.html").exists(),
            "compile must still emit page outputs when config carries a base_url"
        );
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial(cwd, ssg_cache, stager_fp)]
    fn test_pipeline_incremental_propagates_current_hashes_failure() {
        // `current_hashes(content_dir, template_dir)?` is the first
        // thing the incremental fast path does. Walking an existing
        // but unreadable content dir makes `fs::read_dir` fail inside
        // `walk_files_bounded_depth`, so the `?` here propagates —
        // a branch none of the other incremental tests exercise since
        // they all use a normally-readable fixture.
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, content, build, site, templates) = build_fixture();
        let ctx =
            plugin::PluginContext::new(&content, &build, &site, &templates);
        let pm = plugin::PluginManager::new();

        std::fs::set_permissions(
            &content,
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        let res = execute_build_pipeline_with(
            &pm, &ctx, &build, &content, &site, &templates, true, true,
        );

        let _ = std::fs::set_permissions(
            &content,
            std::fs::Permissions::from_mode(0o755),
        );
        assert!(
            res.is_err(),
            "unreadable content_dir must fail current_hashes and propagate"
        );
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial(ssg_cache, stager_fp)]
    fn test_pipeline_tolerates_unwalkable_site_dir() {
        // The sabotage plugin plants an unreadable subdirectory in
        // the site dir after compile, so the post-build HTML walk
        // fails; the cache-update block skips silently and the build
        // still succeeds.
        use std::os::unix::fs::PermissionsExt;
        let mut pm = plugin::PluginManager::new();
        pm.register(SabotagePlugin {
            mode: "lock-subdir",
        });
        let (_tmp, site, res) = run_fixture_with_plugins(&pm, false);

        let locked = site.join("locked");
        let was_locked = locked.is_dir();
        let _ = std::fs::set_permissions(
            &locked,
            std::fs::Permissions::from_mode(0o755),
        );
        res.expect("unwalkable site dir must be non-fatal");
        assert!(was_locked, "sabotage dir must have survived the build");
    }
}
