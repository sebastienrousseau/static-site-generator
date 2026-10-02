// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! # Plugin architecture for SSG
//!
//! Provides a trait-based plugin system with lifecycle hooks for
//! extending the static site generation pipeline.
//!
//! ## Lifecycle hooks
//!
//! Plugins can hook into three stages of site generation:
//!
//! 1. **`before_compile`** — Runs before compilation. Use for content
//!    preprocessing, metadata injection, or source transformation.
//! 2. **`after_compile`** — Runs after compilation. Use for HTML
//!    post-processing, asset optimization, or sitemap generation.
//! 3. **`on_serve`** — Runs before the dev server starts. Use for
//!    injecting dev-mode scripts or live-reload support.
//!
//! ## Example
//!
//! ```rust
//! use ssg::plugin::{Plugin, PluginContext};
//! use anyhow::Result;
//!
//! #[derive(Debug)]
//! struct MinifyPlugin;
//!
//! impl Plugin for MinifyPlugin {
//!     fn name(&self) -> &str { "minify" }
//!
//!     fn after_compile(&self, ctx: &PluginContext) -> std::result::Result<(), ssg::error::SsgError> {
//!         println!("Minifying files in {:?}", ctx.site_dir);
//!         // Walk site_dir and minify HTML/CSS/JS files
//!         Ok(())
//!     }
//! }
//! ```

use crate::cmd::SsgConfig;
use crate::error::{PathErrorExt, SsgError};
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

// =====================================================================
// Content-addressed plugin cache
// =====================================================================

const CACHE_FILENAME: &str = ".ssg-plugin-cache.json";

/// Content-addressed cache that tracks file hashes so plugins can skip
/// unchanged files across incremental builds.
///
/// Stores `path → content_hash` mappings and persists to
/// `.ssg-plugin-cache.json` in the site directory.
#[derive(Debug, Clone, Default)]
pub struct PluginCache {
    entries: BTreeMap<PathBuf, u64>,
}

impl PluginCache {
    /// Creates an empty cache.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginCache;
    /// use std::path::Path;
    ///
    /// let c = PluginCache::new();
    /// assert!(c.has_changed(Path::new("any.path")));
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Loads a cache from `site_dir/.ssg-plugin-cache.json`.
    ///
    /// Returns an empty cache if the file does not exist or cannot be
    /// parsed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginCache;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// // Missing cache file ⇒ empty cache, no error.
    /// let _c = PluginCache::load(dir.path());
    /// ```
    #[must_use]
    pub fn load(site_dir: &Path) -> Self {
        let path = site_dir.join(CACHE_FILENAME);
        if !path.exists() {
            return Self::new();
        }
        let Ok(content) = fs::read_to_string(&path) else {
            return Self::new();
        };
        let Ok(map) = serde_json::from_str::<BTreeMap<String, u64>>(&content)
        else {
            return Self::new();
        };
        Self {
            entries: map
                .into_iter()
                .map(|(k, v)| (PathBuf::from(k), v))
                .collect(),
        }
    }

    /// Persists the cache to `site_dir/.ssg-plugin-cache.json`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginCache;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// PluginCache::new().save(dir.path()).unwrap();
    /// assert!(dir.path().join(".ssg-plugin-cache.json").exists());
    /// ```
    pub fn save(&self, site_dir: &Path) -> Result<(), SsgError> {
        let path = site_dir.join(CACHE_FILENAME);
        let serialisable: BTreeMap<String, u64> = self
            .entries
            .iter()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), *v))
            .collect();
        // Infallible: BTreeMap<String, u64> is always serialisable to
        // JSON. We keep the `map_err` for clippy::expect_used (which is
        // denied in lib); the closure body is dark but harmless.
        let json =
            serde_json::to_string_pretty(&serialisable).map_err(|e| {
                SsgError::Io {
                    path: path.clone(),
                    source: std::io::Error::other(e),
                }
            })?;
        fs::write(&path, json).with_path(&path)?;
        Ok(())
    }

    /// Returns `true` if the file at `path` has changed since the last
    /// time it was recorded, or if it has never been recorded.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginCache;
    /// use tempfile::tempdir;
    /// use std::fs;
    ///
    /// let dir = tempdir().unwrap();
    /// let f = dir.path().join("x.txt");
    /// fs::write(&f, "a").unwrap();
    /// let mut c = PluginCache::new();
    /// // Never recorded ⇒ has_changed = true.
    /// assert!(c.has_changed(&f));
    /// c.update(&f);
    /// assert!(!c.has_changed(&f));
    /// ```
    pub fn has_changed(&self, path: &Path) -> bool {
        let Ok(content) = fs::read(path) else {
            return true;
        };
        let current = Self::hash_bytes(&content);
        match self.entries.get(path) {
            Some(&cached) => cached != current,
            None => true,
        }
    }

    /// Records the current content hash for `path`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginCache;
    /// use tempfile::tempdir;
    /// use std::fs;
    ///
    /// let dir = tempdir().unwrap();
    /// let f = dir.path().join("x.txt");
    /// fs::write(&f, "hi").unwrap();
    /// let mut c = PluginCache::new();
    /// c.update(&f);
    /// assert!(!c.has_changed(&f));
    /// ```
    pub fn update(&mut self, path: &Path) {
        if let Ok(content) = fs::read(path) {
            let hash = Self::hash_bytes(&content);
            let _ = self.entries.insert(path.to_path_buf(), hash);
        }
    }

    /// Simple FNV-1a 64-bit hash of a byte slice.
    fn hash_bytes(data: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in data {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash
    }
}

/// Context passed to plugin hooks with paths and configuration.
#[derive(Debug, Clone)]
pub struct PluginContext {
    /// The content source directory.
    pub content_dir: PathBuf,
    /// The build/output directory.
    pub build_dir: PathBuf,
    /// The final site directory.
    pub site_dir: PathBuf,
    /// The template directory.
    pub template_dir: PathBuf,
    /// Site configuration (`base_url`, `site_name`, language, etc.).
    pub config: Option<SsgConfig>,
    /// Content-addressed plugin cache for incremental builds.
    pub cache: Option<PluginCache>,
    /// Memory budget for streaming compilation.
    pub memory_budget: Option<crate::streaming::MemoryBudget>,
    /// Cached list of HTML files in `site_dir`, walked once and shared
    /// across all plugins to avoid redundant filesystem traversals.
    pub html_files: Option<Arc<Vec<PathBuf>>>,
    /// Page dependency graph for incremental rebuilds.
    pub dep_graph: Option<crate::depgraph::DepGraph>,
    /// When `true`, plugins should perform validation passes only and
    /// must not write to disk. Set by the `ssg check` subcommand
    /// (issue #527). Plugins that don't have a meaningful read-only
    /// mode may safely ignore this flag.
    pub dry_run: bool,
}

impl PluginContext {
    /// Populates the cached HTML file list by walking `site_dir` once.
    /// Call this before running `after_compile` plugins to eliminate
    /// redundant directory scans (8+ plugins read the same file list).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginContext;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let mut ctx = PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
    /// ctx.cache_html_files();
    /// // Empty dir ⇒ empty cached list.
    /// assert!(ctx.get_html_files().is_empty());
    /// ```
    pub fn cache_html_files(&mut self) {
        if self.site_dir.exists() {
            let files = crate::walk::walk_files(&self.site_dir, "html")
                .unwrap_or_default();
            self.html_files = Some(Arc::new(files));
        }
    }

    /// Returns the cached HTML file list, or walks the directory if
    /// the cache hasn't been populated.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginContext;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let ctx = PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
    /// assert!(ctx.get_html_files().is_empty());
    /// ```
    #[must_use]
    pub fn get_html_files(&self) -> Vec<PathBuf> {
        if let Some(ref cached) = self.html_files {
            cached.as_ref().clone()
        } else {
            crate::walk::walk_files(&self.site_dir, "html").unwrap_or_default()
        }
    }

    /// Creates a new plugin context from directory paths.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginContext;
    /// use std::path::Path;
    ///
    /// let ctx = PluginContext::new(
    ///     Path::new("content"), Path::new("build"),
    ///     Path::new("site"),    Path::new("templates"),
    /// );
    /// assert_eq!(ctx.content_dir, Path::new("content"));
    /// ```
    #[must_use]
    pub fn new(
        content_dir: &Path,
        build_dir: &Path,
        site_dir: &Path,
        template_dir: &Path,
    ) -> Self {
        Self {
            content_dir: content_dir.to_path_buf(),
            build_dir: build_dir.to_path_buf(),
            site_dir: site_dir.to_path_buf(),
            template_dir: template_dir.to_path_buf(),
            config: None,
            cache: None,
            memory_budget: None,
            html_files: None,
            dep_graph: None,
            dry_run: false,
        }
    }

    /// Creates a new plugin context with site configuration.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::cmd::SsgConfig;
    /// use ssg::plugin::PluginContext;
    /// use std::path::Path;
    ///
    /// let cfg = SsgConfig::default();
    /// let ctx = PluginContext::with_config(
    ///     Path::new("content"), Path::new("build"),
    ///     Path::new("site"), Path::new("templates"),
    ///     cfg,
    /// );
    /// assert!(ctx.config.is_some());
    /// ```
    #[must_use]
    pub fn with_config(
        content_dir: &Path,
        build_dir: &Path,
        site_dir: &Path,
        template_dir: &Path,
        config: SsgConfig,
    ) -> Self {
        Self {
            content_dir: content_dir.to_path_buf(),
            build_dir: build_dir.to_path_buf(),
            site_dir: site_dir.to_path_buf(),
            template_dir: template_dir.to_path_buf(),
            config: Some(config),
            cache: None,
            memory_budget: None,
            html_files: None,
            dep_graph: None,
            dry_run: false,
        }
    }

    /// Sets the `dry_run` flag and returns the modified context.
    ///
    /// Used by the `ssg check` subcommand (issue #527) to signal to
    /// plugins that they should run their validation passes without
    /// writing to disk.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginContext;
    /// use std::path::Path;
    ///
    /// let ctx = PluginContext::new(
    ///     Path::new("content"), Path::new("build"),
    ///     Path::new("site"), Path::new("templates"),
    /// ).with_dry_run(true);
    /// assert!(ctx.dry_run);
    /// ```
    #[must_use]
    pub const fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }
}

/// Trait for SSG plugins.
///
/// Implement this trait to create a plugin that hooks into the site
/// generation lifecycle. All hooks have default no-op implementations,
/// so you only need to override the ones you care about.
///
/// # Stability contract
///
/// This trait is part of the SSG public API. The stability commitment
/// for the `1.0` line is:
///
/// 1. **All current hook signatures are frozen.** Once `1.0` ships, no
///    parameter, return type, or trait bound on an existing method
///    will change without a major version bump.
/// 2. **New hooks land with a default `Ok(())` implementation.**
///    Adding a new hook is therefore non-breaking — existing
///    `impl Plugin for …` blocks continue to compile.
/// 3. **`PluginContext` is `#[non_exhaustive]`.** New fields (e.g.
///    additional caches, link graphs, image metadata) can be added
///    without breaking downstream construction sites — those are
///    constructed inside SSG, not by plugin authors.
/// 4. **Removing a hook requires a major bump.** Hook removal is rare
///    and always preceded by a deprecation cycle of at least one
///    minor release with `#[deprecated]` and a migration note in the
///    CHANGELOG.
///
/// See [API stability audit](../../docs/architecture/api-stability-audit.md)
/// for the full Tier-A inventory.
pub trait Plugin: fmt::Debug + Send + Sync {
    /// Returns the unique name of this plugin.
    fn name(&self) -> &str;

    /// Called before site compilation begins.
    ///
    /// Use this hook to preprocess content files, inject metadata,
    /// or validate source directories.
    fn before_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
        Ok(())
    }

    /// Called after site compilation completes.
    ///
    /// Use this hook to post-process generated HTML, optimize assets,
    /// generate sitemaps, or perform any output transformation.
    fn after_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
        Ok(())
    }

    /// Per-file HTML transform hook — called once per HTML file during
    /// the fused transform pass.
    ///
    /// Receives the current HTML content and returns the (possibly modified)
    /// HTML. The default implementation returns the input unchanged.
    ///
    /// Plugins that implement this hook avoid redundant file I/O — the
    /// pipeline reads each HTML file once, pipes it through all plugins'
    /// `transform_html` hooks, then writes the result once.
    fn transform_html(
        &self,
        html: &str,
        _path: &Path,
        _ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        Ok(html.to_string())
    }

    /// Returns `true` if this plugin implements `transform_html`.
    /// Override to `true` to opt in to the fused transform pass.
    fn has_transform(&self) -> bool {
        false
    }

    /// Returns `true` if this plugin must always observe the full
    /// `cache.html_files()` list — even during `--incremental`
    /// rebuilds that only invalidated a handful of pages.
    ///
    /// SEO sitemap regeneration, SBOM emission, JSON-LD aggregation,
    /// and search-index builders all need the complete view of the
    /// site to produce correct output, so they opt in to `true`
    /// (the default). Plugins that genuinely work per-file (and so
    /// can be skipped for unaffected pages) override to `false`.
    /// (Issue #524 AC7.)
    fn needs_all_files(&self) -> bool {
        true
    }

    /// Called before the development server starts serving files.
    ///
    /// Use this hook to inject dev-mode scripts, set up live-reload,
    /// or modify the serve directory.
    fn on_serve(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
        Ok(())
    }
}

/// Manages registered plugins and executes lifecycle hooks.
///
/// # Example
///
/// ```rust
/// use ssg::plugin::{PluginManager, PluginContext, Plugin};
/// use ssg::error::SsgError;
/// use std::path::Path;
///
/// #[derive(Debug)]
/// struct LogPlugin;
///
/// impl Plugin for LogPlugin {
///     fn name(&self) -> &str { "logger" }
///     fn before_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
///         println!("Compiling from {:?}", ctx.content_dir);
///         Ok(())
///     }
/// }
///
/// let mut pm = PluginManager::new();
/// pm.register(LogPlugin);
/// assert_eq!(pm.len(), 1);
///
/// let ctx = PluginContext::new(
///     Path::new("content"),
///     Path::new("build"),
///     Path::new("public"),
///     Path::new("templates"),
/// );
/// pm.run_before_compile(&ctx).unwrap();
/// ```
#[derive(Debug, Default)]
pub struct PluginManager {
    plugins: Vec<Box<dyn Plugin>>,
}

/// One row of [`PluginManager::inventory`]: a registered plugin and the
/// optional hooks it opts into.
///
/// `order` is the index in registration order, which is also execution
/// order — the property that makes the taxonomy/pagination phase bug
/// visible rather than implicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginInfo<'a> {
    /// Position in execution order, starting at zero.
    pub order: usize,
    /// The plugin's stable name, as reported by [`Plugin::name`].
    pub name: &'a str,
    /// Whether the plugin participates in the fused HTML transform pass.
    pub has_transform: bool,
    /// Whether the plugin needs every file present before it runs.
    pub needs_all_files: bool,
}

impl PluginManager {
    /// Creates a new empty plugin manager.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginManager;
    ///
    /// let pm = PluginManager::new();
    /// assert!(pm.is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Registers a plugin.
    ///
    /// Plugins run in the order they are registered.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::drafts::DraftPlugin;
    /// use ssg::plugin::PluginManager;
    ///
    /// let mut pm = PluginManager::new();
    /// pm.register(DraftPlugin::new(false));
    /// assert_eq!(pm.len(), 1);
    /// ```
    pub fn register<P: Plugin + 'static>(&mut self, plugin: P) {
        self.plugins.push(Box::new(plugin));
    }

    /// Returns the number of registered plugins.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginManager;
    ///
    /// let pm = PluginManager::new();
    /// assert_eq!(pm.len(), 0);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Returns `true` if no plugins are registered.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::PluginManager;
    ///
    /// assert!(PluginManager::new().is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Returns the names of all registered plugins.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::drafts::DraftPlugin;
    /// use ssg::plugin::PluginManager;
    ///
    /// let mut pm = PluginManager::new();
    /// pm.register(DraftPlugin::new(false));
    /// assert_eq!(pm.names(), vec!["drafts"]);
    /// ```
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.name()).collect()
    }

    /// Returns an inventory of every registered plugin, in execution order.
    ///
    /// This is the single source of truth for "which plugins run, and what do
    /// they do" — consumed by `ssg plugins list` and by the README sync check,
    /// so a plugin added or removed cannot silently leave the documentation
    /// claiming a stale count.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::drafts::DraftPlugin;
    /// use ssg::plugin::PluginManager;
    ///
    /// let mut pm = PluginManager::new();
    /// pm.register(DraftPlugin::new(false));
    ///
    /// let inv = pm.inventory();
    /// assert_eq!(inv.len(), 1);
    /// assert_eq!(inv[0].name, "drafts");
    /// assert_eq!(inv[0].order, 0);
    /// ```
    #[must_use]
    pub fn inventory(&self) -> Vec<PluginInfo<'_>> {
        self.plugins
            .iter()
            .enumerate()
            .map(|(order, p)| PluginInfo {
                order,
                name: p.name(),
                has_transform: p.has_transform(),
                needs_all_files: p.needs_all_files(),
            })
            .collect()
    }

    /// Runs the `before_compile` hook on all registered plugins.
    ///
    /// Plugins execute in registration order. If any plugin returns
    /// an error, execution stops and the error is propagated.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::{PluginContext, PluginManager};
    /// use std::path::Path;
    ///
    /// let pm = PluginManager::new();
    /// let ctx = PluginContext::new(
    ///     Path::new("content"), Path::new("build"),
    ///     Path::new("site"), Path::new("templates"),
    /// );
    /// assert!(pm.run_before_compile(&ctx).is_ok());
    /// ```
    pub fn run_before_compile(
        &self,
        ctx: &PluginContext,
    ) -> Result<(), SsgError> {
        for plugin in &self.plugins {
            plugin.before_compile(ctx)?;
        }
        Ok(())
    }

    /// Runs the `after_compile` hook on all registered plugins.
    ///
    /// Plugins execute in registration order. If any plugin returns
    /// an error, execution stops and the error is propagated.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::{PluginContext, PluginManager};
    /// use std::path::Path;
    ///
    /// let pm = PluginManager::new();
    /// let ctx = PluginContext::new(
    ///     Path::new("content"), Path::new("build"),
    ///     Path::new("site"), Path::new("templates"),
    /// );
    /// assert!(pm.run_after_compile(&ctx).is_ok());
    /// ```
    pub fn run_after_compile(
        &self,
        ctx: &PluginContext,
    ) -> Result<(), SsgError> {
        for plugin in &self.plugins {
            plugin.after_compile(ctx)?;
        }
        Ok(())
    }

    /// Runs the fused HTML transform pass: reads each HTML file once,
    /// pipes through all plugins with `has_transform() == true`, writes once.
    ///
    /// This eliminates N separate read/write cycles (where N = number of
    /// transform plugins) per HTML file.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::{PluginContext, PluginManager};
    /// use std::path::Path;
    ///
    /// let pm = PluginManager::new();
    /// let ctx = PluginContext::new(
    ///     Path::new("content"), Path::new("build"),
    ///     Path::new("site"), Path::new("templates"),
    /// );
    /// // No transform plugins ⇒ trivial Ok.
    /// assert!(pm.run_fused_transforms(&ctx).is_ok());
    /// ```
    pub fn run_fused_transforms(
        &self,
        ctx: &PluginContext,
    ) -> Result<(), SsgError> {
        use rayon::prelude::*;

        let transform_plugins: Vec<_> =
            self.plugins.iter().filter(|p| p.has_transform()).collect();

        if transform_plugins.is_empty() {
            return Ok(());
        }

        let html_files = ctx.get_html_files();
        let transformed = std::sync::atomic::AtomicUsize::new(0);

        // Writer pool (issue #569 phase 1): rayon workers hand changed
        // files to dedicated writer threads instead of blocking a CPU
        // slot on `fs::write`. Unchanged files are skipped entirely —
        // on a no-op rebuild this pass writes zero files.
        let io_pool = crate::io_pool::IoPool::new();

        html_files
            .par_iter()
            .try_for_each(|path| -> Result<(), SsgError> {
                let original = fs::read_to_string(path).with_path(path)?;
                let mut html = original.clone();

                for plugin in &transform_plugins {
                    html = plugin.transform_html(&html, path, ctx)?;
                }

                if html != original {
                    io_pool.write(path, html.into_bytes())?;
                    let _ = transformed
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(())
            })?;

        // Barrier: everything after this pass (dep-graph repopulation
        // and the plugin content-hash cache rebuild in
        // `pipeline::compile_with_plugins`, dev-server reads, audits)
        // re-reads the transformed files from disk, so all queued
        // writes must be durably complete — and any write failure
        // surfaced — before this function returns.
        io_pool.flush()?;

        let count = transformed.load(std::sync::atomic::Ordering::Relaxed);
        if count > 0 {
            log::info!(
                "[pipeline] Fused transform: {count} file(s), {} plugin(s)",
                transform_plugins.len()
            );
        }
        Ok(())
    }

    /// Runs the `on_serve` hook on all registered plugins.
    ///
    /// Plugins execute in registration order. If any plugin returns
    /// an error, execution stops and the error is propagated.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::{PluginContext, PluginManager};
    /// use std::path::Path;
    ///
    /// let pm = PluginManager::new();
    /// let ctx = PluginContext::new(
    ///     Path::new("content"), Path::new("build"),
    ///     Path::new("site"), Path::new("templates"),
    /// );
    /// assert!(pm.run_on_serve(&ctx).is_ok());
    /// ```
    pub fn run_on_serve(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        for plugin in &self.plugins {
            plugin.on_serve(ctx)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct CounterPlugin {
        name: &'static str,
        before: &'static AtomicUsize,
        after: &'static AtomicUsize,
        serve: &'static AtomicUsize,
    }

    impl Plugin for CounterPlugin {
        fn name(&self) -> &str {
            self.name
        }
        fn before_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
            let _ = self.before.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn after_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
            let _ = self.after.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn on_serve(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
            let _ = self.serve.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailPlugin {
        hook: &'static str,
    }

    impl Plugin for FailPlugin {
        fn name(&self) -> &'static str {
            "fail-plugin"
        }
        fn before_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
            if self.hook == "before" {
                return Err(SsgError::Io {
                    path: PathBuf::from("fail"),
                    source: std::io::Error::other("before_compile failed"),
                });
            }
            Ok(())
        }
        fn after_compile(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
            if self.hook == "after" {
                return Err(SsgError::Io {
                    path: PathBuf::from("fail"),
                    source: std::io::Error::other("after_compile failed"),
                });
            }
            Ok(())
        }
        fn on_serve(&self, _ctx: &PluginContext) -> Result<(), SsgError> {
            if self.hook == "serve" {
                return Err(SsgError::Io {
                    path: PathBuf::from("fail"),
                    source: std::io::Error::other("on_serve failed"),
                });
            }
            Ok(())
        }
    }

    #[derive(Debug)]
    struct NoopPlugin;

    impl Plugin for NoopPlugin {
        fn name(&self) -> &'static str {
            "noop"
        }
    }

    fn test_ctx() -> PluginContext {
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            Path::new("public"),
            Path::new("templates"),
        )
    }

    #[test]
    fn test_plugin_manager_new_is_empty() {
        let pm = PluginManager::new();
        assert!(pm.is_empty());
        assert_eq!(pm.len(), 0);
        assert!(pm.names().is_empty(), "{:?}", pm.names());
    }

    #[test]
    fn test_plugin_manager_default() {
        let pm = PluginManager::default();
        assert!(pm.is_empty());
    }

    #[test]
    fn test_register_and_count() {
        let mut pm = PluginManager::new();
        pm.register(NoopPlugin);
        assert_eq!(pm.len(), 1);
        assert!(!pm.is_empty());
        assert_eq!(pm.names(), vec!["noop"]);
    }

    #[test]
    fn test_multiple_plugins_run_in_order() {
        static BEFORE_A: AtomicUsize = AtomicUsize::new(0);
        static AFTER_A: AtomicUsize = AtomicUsize::new(0);
        static SERVE_A: AtomicUsize = AtomicUsize::new(0);
        static BEFORE_B: AtomicUsize = AtomicUsize::new(0);
        static AFTER_B: AtomicUsize = AtomicUsize::new(0);
        static SERVE_B: AtomicUsize = AtomicUsize::new(0);

        let mut pm = PluginManager::new();
        pm.register(CounterPlugin {
            name: "a",
            before: &BEFORE_A,
            after: &AFTER_A,
            serve: &SERVE_A,
        });
        pm.register(CounterPlugin {
            name: "b",
            before: &BEFORE_B,
            after: &AFTER_B,
            serve: &SERVE_B,
        });

        let ctx = test_ctx();
        pm.run_before_compile(&ctx).unwrap();
        pm.run_after_compile(&ctx).unwrap();
        pm.run_on_serve(&ctx).unwrap();

        assert_eq!(BEFORE_A.load(Ordering::SeqCst), 1);
        assert_eq!(BEFORE_B.load(Ordering::SeqCst), 1);
        assert_eq!(AFTER_A.load(Ordering::SeqCst), 1);
        assert_eq!(AFTER_B.load(Ordering::SeqCst), 1);
        assert_eq!(SERVE_A.load(Ordering::SeqCst), 1);
        assert_eq!(SERVE_B.load(Ordering::SeqCst), 1);
        assert_eq!(pm.names(), vec!["a", "b"]);
    }

    #[test]
    fn test_noop_plugin_all_hooks_succeed() {
        let mut pm = PluginManager::new();
        pm.register(NoopPlugin);
        let ctx = test_ctx();
        assert!(pm.run_before_compile(&ctx).is_ok());
        assert!(pm.run_after_compile(&ctx).is_ok());
        assert!(pm.run_on_serve(&ctx).is_ok());
    }

    #[test]
    fn test_before_compile_error_propagates() {
        let mut pm = PluginManager::new();
        pm.register(FailPlugin { hook: "before" });
        let ctx = test_ctx();
        let err = pm.run_before_compile(&ctx).unwrap_err();
        // Debug output carries both the variant and the source message,
        // asserting the same facts as a `matches!` + field check without
        // an uncoverable fallthrough arm.
        let dbg = format!("{err:?}");
        assert!(dbg.contains("Io"), "expected Io variant, got: {dbg}");
        assert!(
            dbg.contains("before_compile failed"),
            "source message expected: {dbg}"
        );
    }

    #[test]
    fn test_after_compile_error_propagates() {
        let mut pm = PluginManager::new();
        pm.register(FailPlugin { hook: "after" });
        let ctx = test_ctx();
        let err = pm.run_after_compile(&ctx).unwrap_err();
        // Debug output carries both the variant and the source message,
        // asserting the same facts as a `matches!` + field check without
        // an uncoverable fallthrough arm.
        let dbg = format!("{err:?}");
        assert!(dbg.contains("Io"), "expected Io variant, got: {dbg}");
        assert!(
            dbg.contains("after_compile failed"),
            "source message expected: {dbg}"
        );
    }

    #[test]
    fn test_on_serve_error_propagates() {
        let mut pm = PluginManager::new();
        pm.register(FailPlugin { hook: "serve" });
        let ctx = test_ctx();
        let err = pm.run_on_serve(&ctx).unwrap_err();
        // Debug output carries both the variant and the source message,
        // asserting the same facts as a `matches!` + field check without
        // an uncoverable fallthrough arm.
        let dbg = format!("{err:?}");
        assert!(dbg.contains("Io"), "expected Io variant, got: {dbg}");
        assert!(
            dbg.contains("on_serve failed"),
            "source message expected: {dbg}"
        );
    }

    #[test]
    fn test_error_stops_subsequent_plugins() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);

        let mut pm = PluginManager::new();
        pm.register(FailPlugin { hook: "before" });
        pm.register(CounterPlugin {
            name: "second",
            before: &COUNTER,
            after: &COUNTER,
            serve: &COUNTER,
        });

        let ctx = test_ctx();
        assert!(pm.run_before_compile(&ctx).is_err());
        // Second plugin should not have run
        assert_eq!(COUNTER.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_empty_manager_hooks_succeed() {
        let pm = PluginManager::new();
        let ctx = test_ctx();
        assert!(pm.run_before_compile(&ctx).is_ok());
        assert!(pm.run_after_compile(&ctx).is_ok());
        assert!(pm.run_on_serve(&ctx).is_ok());
    }

    #[test]
    fn test_plugin_context_fields() {
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

    #[test]
    fn test_plugin_context_clone() {
        let ctx = test_ctx();
        let cloned = ctx.clone();
        assert_eq!(ctx.content_dir, cloned.content_dir);
        assert_eq!(ctx.site_dir, cloned.site_dir);
    }

    #[test]
    fn test_plugin_context_debug() {
        let ctx = test_ctx();
        let debug = format!("{ctx:?}");
        assert!(debug.contains("content"));
        assert!(debug.contains("build"));
    }

    #[test]
    fn test_plugin_manager_debug() {
        let mut pm = PluginManager::new();
        pm.register(NoopPlugin);
        let debug = format!("{pm:?}");
        assert!(debug.contains("NoopPlugin"));
    }

    // -----------------------------------------------------------------
    // PluginCache tests
    // -----------------------------------------------------------------

    #[test]
    fn test_cache_new_is_empty() {
        let cache = PluginCache::new();
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn test_cache_has_changed_on_missing_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("hello.txt");
        fs::write(&file, "hello").unwrap();

        let cache = PluginCache::new();
        assert!(cache.has_changed(&file), "New file should count as changed");
    }

    #[test]
    fn test_cache_has_changed_detects_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("hello.txt");
        fs::write(&file, "hello").unwrap();

        let mut cache = PluginCache::new();
        cache.update(&file);
        assert!(
            !cache.has_changed(&file),
            "File should not be changed after update"
        );
    }

    #[test]
    fn test_cache_has_changed_detects_modification() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("hello.txt");
        fs::write(&file, "hello").unwrap();

        let mut cache = PluginCache::new();
        cache.update(&file);

        // Modify the file
        fs::write(&file, "world").unwrap();
        assert!(
            cache.has_changed(&file),
            "Modified file should be detected as changed"
        );
    }

    #[test]
    fn test_cache_persistence_save_load() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("data.txt");
        fs::write(&file, "content").unwrap();

        let mut cache = PluginCache::new();
        cache.update(&file);
        cache.save(tmp.path()).unwrap();

        // Verify the cache file exists
        let cache_path = tmp.path().join(CACHE_FILENAME);
        assert!(cache_path.exists(), "Cache file should be persisted");

        // Load it back
        let loaded = PluginCache::load(tmp.path());
        assert!(
            !loaded.has_changed(&file),
            "Loaded cache should still recognise unchanged file"
        );
    }

    #[test]
    fn test_cache_load_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = PluginCache::load(tmp.path());
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn test_cache_has_changed_nonexistent_file() {
        let cache = PluginCache::new();
        assert!(
            cache.has_changed(Path::new("/nonexistent/file.txt")),
            "Nonexistent file should count as changed"
        );
    }

    // -----------------------------------------------------------------
    // PluginCache: save/load round-trip, hash determinism, empty cache
    // -----------------------------------------------------------------

    #[test]
    fn test_cache_save_load_round_trip_with_multiple_files() {
        let tmp = tempfile::tempdir().unwrap();
        let f1 = tmp.path().join("one.txt");
        let f2 = tmp.path().join("two.txt");
        fs::write(&f1, "alpha").unwrap();
        fs::write(&f2, "beta").unwrap();

        let mut cache = PluginCache::new();
        cache.update(&f1);
        cache.update(&f2);
        cache.save(tmp.path()).unwrap();

        let loaded = PluginCache::load(tmp.path());
        assert!(!loaded.has_changed(&f1));
        assert!(!loaded.has_changed(&f2));
    }

    #[test]
    fn test_cache_empty_save_load() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = PluginCache::new();
        cache.save(tmp.path()).unwrap();

        let loaded = PluginCache::load(tmp.path());
        assert!(loaded.entries.is_empty());
    }

    #[test]
    fn test_cache_hash_bytes_determinism() {
        let data = b"hello world";
        let h1 = PluginCache::hash_bytes(data);
        let h2 = PluginCache::hash_bytes(data);
        assert_eq!(h1, h2, "same input must produce same hash");
    }

    #[test]
    fn test_cache_hash_bytes_different_inputs() {
        let h1 = PluginCache::hash_bytes(b"aaa");
        let h2 = PluginCache::hash_bytes(b"bbb");
        assert_ne!(h1, h2, "different inputs should produce different hashes");
    }

    #[test]
    fn test_cache_hash_bytes_empty() {
        // Empty input should return the FNV offset basis
        let h = PluginCache::hash_bytes(b"");
        assert_eq!(h, 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn test_cache_has_changed_after_file_modification() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("data.txt");
        fs::write(&f, "version1").unwrap();

        let mut cache = PluginCache::new();
        cache.update(&f);
        assert!(!cache.has_changed(&f));

        // Modify file content
        fs::write(&f, "version2").unwrap();
        assert!(cache.has_changed(&f));

        // Update cache, should no longer be changed
        cache.update(&f);
        assert!(!cache.has_changed(&f));
    }

    #[test]
    fn test_cache_load_corrupt_json() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join(CACHE_FILENAME);
        fs::write(&cache_path, "this is not json").unwrap();

        let loaded = PluginCache::load(tmp.path());
        assert!(
            loaded.entries.is_empty(),
            "corrupt JSON should yield empty cache"
        );
    }

    #[test]
    fn test_cache_update_nonexistent_file_is_noop() {
        let mut cache = PluginCache::new();
        cache.update(Path::new("/nonexistent/file.txt"));
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn test_cache_default_is_empty() {
        let cache = PluginCache::default();
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn test_cache_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("x.txt");
        fs::write(&f, "x").unwrap();

        let mut cache = PluginCache::new();
        cache.update(&f);

        let cloned = cache.clone();
        assert!(!cloned.has_changed(&f));
    }

    #[test]
    fn test_plugin_context_with_config() {
        let config = SsgConfig::builder()
            .site_name("test".to_string())
            .base_url("https://example.com".to_string())
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            Path::new("c"),
            Path::new("b"),
            Path::new("s"),
            Path::new("t"),
            config,
        );
        assert!(ctx.config.is_some());
        assert_eq!(ctx.config.unwrap().site_name, "test");
    }

    #[test]
    fn test_needs_all_files_defaults_to_true() {
        // Issue #524 AC7: the default is conservative — every plugin
        // sees the full file list unless it explicitly opts out.
        let p = NoopPlugin;
        assert!(p.needs_all_files());
    }

    #[derive(Debug)]
    struct PerFilePlugin;
    impl Plugin for PerFilePlugin {
        fn name(&self) -> &'static str {
            "per-file"
        }
        fn needs_all_files(&self) -> bool {
            false
        }
    }

    #[test]
    fn test_needs_all_files_can_be_overridden() {
        assert!(!PerFilePlugin.needs_all_files());
        assert_eq!(PerFilePlugin.name(), "per-file");
    }

    #[test]
    fn test_fail_plugin_non_matching_hooks_succeed() {
        let ctx = test_ctx();

        // FailPlugin("before") should succeed on after_compile and on_serve
        let p = FailPlugin { hook: "before" };
        assert_eq!(p.name(), "fail-plugin");
        assert!(p.after_compile(&ctx).is_ok());
        assert!(p.on_serve(&ctx).is_ok());

        // FailPlugin("after") should succeed on before_compile and on_serve
        let p = FailPlugin { hook: "after" };
        assert!(p.before_compile(&ctx).is_ok());
        assert!(p.on_serve(&ctx).is_ok());

        // FailPlugin("serve") should succeed on before_compile and after_compile
        let p = FailPlugin { hook: "serve" };
        assert!(p.before_compile(&ctx).is_ok());
        assert!(p.after_compile(&ctx).is_ok());
    }

    /// Transform plugin that returns the input unchanged (but as a
    /// fresh `String`, like real plugins that found nothing to do).
    #[derive(Debug)]
    struct IdentityTransformPlugin;
    impl Plugin for IdentityTransformPlugin {
        fn name(&self) -> &'static str {
            "identity-transform"
        }
        fn transform_html(
            &self,
            html: &str,
            _path: &Path,
            _ctx: &PluginContext,
        ) -> Result<String, SsgError> {
            Ok(html.to_string())
        }
        fn has_transform(&self) -> bool {
            true
        }
    }

    /// Transform plugin that rewrites a marker when present.
    #[derive(Debug)]
    struct MarkerRewritePlugin;
    impl Plugin for MarkerRewritePlugin {
        fn name(&self) -> &'static str {
            "marker-rewrite"
        }
        fn transform_html(
            &self,
            html: &str,
            _path: &Path,
            _ctx: &PluginContext,
        ) -> Result<String, SsgError> {
            Ok(html.replace("CHANGE-ME", "CHANGED"))
        }
        fn has_transform(&self) -> bool {
            true
        }
    }

    /// Makes `path` read-only so any attempted rewrite fails loudly.
    #[allow(clippy::permissions_set_readonly_false)] // test cleanup only
    fn set_readonly(path: &Path, readonly: bool) {
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_readonly(readonly);
        fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn test_fused_noop_chain_rewrites_zero_files() {
        // Plan §4 3.2: on a no-op rebuild the transform pass must
        // write 0 files. The files are made read-only, so if the
        // pass attempted any write it would surface as an Err from
        // the IoPool flush barrier.
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<_> = (0..3)
            .map(|i| {
                let f = dir.path().join(format!("p{i}.html"));
                fs::write(&f, format!("<p>page {i}</p>")).unwrap();
                set_readonly(&f, true);
                f
            })
            .collect();

        assert_eq!(IdentityTransformPlugin.name(), "identity-transform");
        assert_eq!(MarkerRewritePlugin.name(), "marker-rewrite");

        let mut pm = PluginManager::new();
        pm.register(IdentityTransformPlugin);
        pm.register(MarkerRewritePlugin); // no marker present ⇒ no-op

        let mut ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        ctx.cache_html_files();

        // Zero writes ⇒ Ok even though every file is read-only.
        pm.run_fused_transforms(&ctx).unwrap();

        for (i, f) in files.iter().enumerate() {
            assert_eq!(
                fs::read_to_string(f).unwrap(),
                format!("<p>page {i}</p>")
            );
            set_readonly(f, false); // restore for tempdir cleanup
        }
    }

    #[test]
    fn test_fused_modifying_chain_writes_exactly_changed_files() {
        // The file containing the marker is rewritten; the untouched
        // file stays byte-identical AND is read-only — proving the
        // pass wrote exactly the changed set.
        let dir = tempfile::tempdir().unwrap();
        let changed = dir.path().join("changed.html");
        let untouched = dir.path().join("untouched.html");
        fs::write(&changed, "<p>CHANGE-ME</p>").unwrap();
        fs::write(&untouched, "<p>static</p>").unwrap();
        set_readonly(&untouched, true);

        let mut pm = PluginManager::new();
        pm.register(MarkerRewritePlugin);

        let mut ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        ctx.cache_html_files();

        pm.run_fused_transforms(&ctx).unwrap();

        // Barrier semantics: the rewritten bytes are visible on disk
        // immediately after run_fused_transforms returns.
        assert_eq!(fs::read_to_string(&changed).unwrap(), "<p>CHANGED</p>");
        assert_eq!(fs::read_to_string(&untouched).unwrap(), "<p>static</p>");
        set_readonly(&untouched, false);
    }

    #[test]
    fn test_fused_transform_write_failure_surfaces_at_flush() {
        // A rewrite aimed at a read-only file must produce an error,
        // not silently drop the write (IoPool flush barrier).
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("locked.html");
        fs::write(&f, "<p>CHANGE-ME</p>").unwrap();
        set_readonly(&f, true);

        let mut pm = PluginManager::new();
        pm.register(MarkerRewritePlugin);

        let mut ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        ctx.cache_html_files();

        let err = pm
            .run_fused_transforms(&ctx)
            .expect_err("write to read-only file must surface");
        let dbg = format!("{err:?}");
        assert!(dbg.contains("Io"), "expected Io variant, got: {dbg}");
        set_readonly(&f, false);
    }

    // -----------------------------------------------------------------
    // PluginCache — degraded-input branches
    // -----------------------------------------------------------------

    #[test]
    fn test_cache_load_unreadable_file_yields_empty_cache() {
        // Invalid UTF-8 bytes: the file exists but read_to_string fails,
        // taking the `let Ok(content) = … else` fallback.
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join(CACHE_FILENAME);
        fs::write(&cache_path, [0xFF, 0xFE, 0xFD]).unwrap();

        let loaded = PluginCache::load(tmp.path());
        assert!(
            loaded.entries.is_empty(),
            "unreadable cache file should yield an empty cache"
        );
    }

    #[test]
    fn test_cache_save_write_failure_returns_io_error() {
        // A directory squatting on the cache filename makes fs::write fail.
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join(CACHE_FILENAME)).unwrap();

        let err = PluginCache::new()
            .save(tmp.path())
            .expect_err("write over a directory must fail");
        let dbg = format!("{err:?}");
        assert!(dbg.contains("Io"), "expected Io variant, got: {dbg}");
    }

    // -----------------------------------------------------------------
    // PluginContext::cache_html_files — missing site_dir branch
    // -----------------------------------------------------------------

    #[test]
    fn test_cache_html_files_missing_site_dir_leaves_cache_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing-site");
        let mut ctx =
            PluginContext::new(tmp.path(), tmp.path(), &missing, tmp.path());
        ctx.cache_html_files();
        assert!(
            ctx.html_files.is_none(),
            "missing site_dir must not populate the html cache"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_cache_html_files_walk_error_yields_empty_cached_list() {
        // `site_dir` exists (so the `exists()` guard passes) but a
        // nested unreadable subdirectory makes `walk::walk_files`
        // return `Err`, exercising the `unwrap_or_default()` failure
        // arm rather than the empty-dir success arm covered elsewhere.
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let site = tmp.path().join("site");
        let locked = site.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let mut ctx =
            PluginContext::new(tmp.path(), tmp.path(), &site, tmp.path());
        ctx.cache_html_files();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert_eq!(
            ctx.html_files.as_deref(),
            Some(&Vec::new()),
            "walk error must degrade to an empty cached list, not panic"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_get_html_files_walk_error_returns_empty_uncached() {
        // Same failure as above but taken through `get_html_files()`'s
        // own `unwrap_or_default()` fallback (the `html_files` cache
        // was never populated, so it re-walks and must degrade
        // gracefully rather than propagating the error or panicking).
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let site = tmp.path().join("site");
        let locked = site.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let ctx = PluginContext::new(tmp.path(), tmp.path(), &site, tmp.path());
        let files = ctx.get_html_files();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(files.is_empty(), "walk error must yield an empty Vec");
    }

    // -----------------------------------------------------------------
    // Plugin trait — default transform_html implementation
    // -----------------------------------------------------------------

    #[test]
    fn test_default_transform_html_returns_input_unchanged() {
        let ctx = test_ctx();
        let out = NoopPlugin
            .transform_html("<p>as-is</p>", Path::new("x.html"), &ctx)
            .unwrap();
        assert_eq!(out, "<p>as-is</p>");
    }

    // -----------------------------------------------------------------
    // run_fused_transforms — early-return + error paths
    // -----------------------------------------------------------------

    #[test]
    fn test_fused_without_transform_plugins_is_trivial_ok() {
        // NoopPlugin has has_transform() == false, so the pass exits
        // before touching the filesystem.
        let mut pm = PluginManager::new();
        pm.register(NoopPlugin);
        let ctx = test_ctx();
        pm.run_fused_transforms(&ctx).unwrap();
    }

    #[test]
    fn test_fused_read_failure_on_invalid_utf8_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("broken.html"), [0xFF, 0xFE, 0xFD]).unwrap();

        let mut pm = PluginManager::new();
        pm.register(IdentityTransformPlugin);

        let mut ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        ctx.cache_html_files();

        let err = pm
            .run_fused_transforms(&ctx)
            .expect_err("invalid UTF-8 html must surface a read error");
        let dbg = format!("{err:?}");
        assert!(dbg.contains("broken.html"), "path context expected: {dbg}");
    }

    /// Transform plugin whose hook always fails.
    #[derive(Debug)]
    struct FailingTransformPlugin;
    impl Plugin for FailingTransformPlugin {
        fn name(&self) -> &'static str {
            "failing-transform"
        }
        fn transform_html(
            &self,
            _html: &str,
            path: &Path,
            _ctx: &PluginContext,
        ) -> Result<String, SsgError> {
            Err(SsgError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::other("transform_html failed"),
            })
        }
        fn has_transform(&self) -> bool {
            true
        }
    }

    #[test]
    fn test_fused_transform_error_stops_the_pass() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("page.html"), "<p>x</p>").unwrap();

        let mut pm = PluginManager::new();
        pm.register(FailingTransformPlugin);
        assert_eq!(FailingTransformPlugin.name(), "failing-transform");

        let mut ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        ctx.cache_html_files();

        let err = pm
            .run_fused_transforms(&ctx)
            .expect_err("failing transform plugin must surface its error");
        let dbg = format!("{err:?}");
        assert!(
            dbg.contains("transform_html failed"),
            "plugin error expected: {dbg}"
        );
        // The file is untouched.
        assert_eq!(
            fs::read_to_string(dir.path().join("page.html")).unwrap(),
            "<p>x</p>"
        );
    }
}
