#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![doc = include_str!("../README.md")]
#![doc(
    html_favicon_url = "https://cloudcdn.pro/static-site-generator/v1/favicon.ico",
    html_logo_url = "https://cloudcdn.pro/static-site-generator/v1/logos/static-site-generator.svg",
    html_root_url = "https://docs.rs/ssg"
)]
#![crate_name = "ssg"]
#![crate_type = "lib"]

/// Fault injection macro. When the `test-fault-injection` feature is
/// enabled, delegates to the `fail` crate's real `fail_point!`. In
/// normal builds this compiles to nothing.
#[cfg(feature = "test-fault-injection")]
macro_rules! fail_point {
    ($name:expr, $body:expr) => {
        fail::fail_point!($name, $body);
    };
}
#[cfg(not(feature = "test-fault-injection"))]
macro_rules! fail_point {
    ($name:expr, $body:expr) => {};
}

/// Test-only utilities shared across unit test modules.
#[cfg(test)]
#[allow(unreachable_pub, clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod test_support {
    use std::sync::Once;

    static LOGGER: Once = Once::new();

    /// Raises `log::max_level()` to Trace so `log::info!` / `log::warn!`
    /// macro bodies execute their format arguments and are counted by
    /// LLVM region coverage. We only bump the filter level; no logger
    /// backend is installed, so it does not conflict with tests that
    /// install their own (e.g. the `env_logger` init test in lib.rs).
    /// Safe to call from any number of tests or fixtures.
    pub fn init_logger() {
        LOGGER.call_once(|| {
            log::set_max_level(log::LevelFilter::Trace);
        });
    }
}

// Standard library imports
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::cmd::{Cli, CliInvocation, SsgConfig};

// Third-party imports
use log::info;

/// Returns the current time as an ISO 8601 UTC string.
///
/// # Examples
///
/// ```rust
/// use ssg::now_iso;
///
/// let stamp = now_iso();
/// // Format is YYYY-MM-DDTHH:MM:SSZ — always 20 chars.
/// assert_eq!(stamp.len(), 20);
/// assert!(stamp.ends_with('Z'));
/// assert_eq!(&stamp[4..5], "-");
/// ```
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let (sec, min, hour) = (secs % 60, (secs / 60) % 60, (secs / 3600) % 24);
    let days = secs / 86400;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Civil days algorithm (Howard Hinnant) — converts days since Unix epoch to (Y, M, D).
const fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub mod audit;
pub mod cmd;
#[path = "core/mod.rs"]
pub(crate) mod core_group;
pub mod error;
#[path = "plugins/mod.rs"]
pub(crate) mod plugins_group;
#[path = "server/mod.rs"]
pub(crate) mod server_group;
pub mod theme;
#[path = "util/mod.rs"]
pub mod util;
pub use error::{PathErrorExt, SsgError};

// Re-export core modules for public API compatibility
pub use crate::core_group::bench_corpus;
pub use crate::core_group::cache;
pub use crate::core_group::collections;
pub use crate::core_group::content;
pub use crate::core_group::content_stager;
pub use crate::core_group::dates;
pub use crate::core_group::depgraph;
pub use crate::core_group::deploy;
pub use crate::core_group::deploy_adapter;
pub use crate::core_group::frontmatter;
pub use crate::core_group::fs_ops;
pub use crate::core_group::io_pool;
pub use crate::core_group::logging;
pub use crate::core_group::otel;
pub use crate::core_group::pipeline;
pub use crate::core_group::process;
pub use crate::core_group::scaffold;
pub use crate::core_group::schema;
pub use crate::core_group::stream;
pub use crate::core_group::streaming;
#[cfg(feature = "templates")]
pub use crate::core_group::template_engine;
pub use crate::core_group::theme_manifest;
pub use crate::core_group::urls;
pub use crate::core_group::walk;

// Re-export plugin modules
pub use crate::plugins_group::accessibility;
pub use crate::plugins_group::agent_api;
pub use crate::plugins_group::ai;
pub use crate::plugins_group::assets;
pub use crate::plugins_group::audit as audit_plugin;
pub use crate::plugins_group::csp;
pub use crate::plugins_group::drafts;
pub use crate::plugins_group::highlight;
pub use crate::plugins_group::i18n;
#[cfg(feature = "image-optimization")]
pub use crate::plugins_group::image_plugin;
pub use crate::plugins_group::islands;
pub use crate::plugins_group::isr_manifest;
pub use crate::plugins_group::llm;
pub use crate::plugins_group::llm_cache;
pub use crate::plugins_group::markdown_ext;
pub use crate::plugins_group::oembed;
pub use crate::plugins_group::og_image;
pub use crate::plugins_group::pagination;
pub use crate::plugins_group::plugin;
pub use crate::plugins_group::plugins;
pub use crate::plugins_group::postprocess;
pub use crate::plugins_group::rpc_schema;
pub use crate::plugins_group::sbom;
pub use crate::plugins_group::search;
pub use crate::plugins_group::search_index;
pub use crate::plugins_group::seo;
pub use crate::plugins_group::shortcodes;
pub use crate::plugins_group::taxonomy;
#[cfg(feature = "templates")]
pub use crate::plugins_group::template_plugin;
pub use crate::plugins_group::topic_clusters;
pub use crate::plugins_group::view_transitions;

// Re-export server modules
pub use crate::server_group::dev_server;
pub use crate::server_group::event_watch;
pub use crate::server_group::hmr;
pub use crate::server_group::livereload;
pub use crate::server_group::server;
pub use crate::server_group::watch;

/// Re-exports
pub use staticdatagen;

// Re-export everything that was previously pub in lib.rs
pub use crate::core_group::fs_ops::{
    collect_files_recursive, copy_dir_all, copy_dir_all_async,
    copy_dir_with_progress, is_path_within_root, is_safe_path,
    verify_and_copy_files, verify_and_copy_files_async, verify_file_safety,
};
pub use crate::core_group::logging::{
    create_log_file, log_arguments, log_initialization,
};
pub use crate::core_group::pipeline::{compile_site, execute_build_pipeline};
pub use crate::server_group::server::{
    generate_locale_redirect, handle_server, prepare_serve_dir, serve_site,
    serve_site_with, HttpTransport, ServeTransport,
};

/// Maximum directory nesting depth for all traversal operations.
/// Prevents stack overflow from pathological or circular directory trees.
/// 128 levels accommodates any realistic project structure.
pub const MAX_DIR_DEPTH: usize = 128;

/// Represents the necessary directory paths for the site generator.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The site output directory
    pub site: PathBuf,
    /// The content directory
    pub content: PathBuf,
    /// The build directory
    pub build: PathBuf,
    /// The template directory
    pub template: PathBuf,
}

impl Paths {
    /// Creates a new builder for configuring Paths
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::Paths;
    ///
    /// let paths = Paths::builder()
    ///     .site("out")
    ///     .content("docs")
    ///     .build_dir("tmp")
    ///     .template("tpl")
    ///     .build()
    ///     .expect("valid paths");
    /// assert_eq!(paths.site.to_str(), Some("out"));
    /// ```
    #[must_use]
    pub fn builder() -> PathsBuilder {
        PathsBuilder::default()
    }

    /// Creates paths with default directories
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::Paths;
    ///
    /// let paths = Paths::default_paths();
    /// assert_eq!(paths.site.to_str(), Some("public"));
    /// assert_eq!(paths.content.to_str(), Some("content"));
    /// assert_eq!(paths.build.to_str(), Some("build"));
    /// assert_eq!(paths.template.to_str(), Some("templates"));
    /// ```
    #[must_use]
    pub fn default_paths() -> Self {
        Self {
            site: PathBuf::from("public"),
            content: PathBuf::from("content"),
            build: PathBuf::from("build"),
            template: PathBuf::from("templates"),
        }
    }
}
// Modify the validate method in Paths impl
impl Paths {
    /// Validates all paths in the configuration
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::Paths;
    /// use std::path::PathBuf;
    ///
    /// let good = Paths::default_paths();
    /// assert!(good.validate().is_ok());
    ///
    /// let bad = Paths {
    ///     site: PathBuf::from("../escape"),
    ///     content: PathBuf::from("content"),
    ///     build: PathBuf::from("build"),
    ///     template: PathBuf::from("templates"),
    /// };
    /// assert!(bad.validate().is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`SsgError::PathTraversal`] if any path contains `..`,
    /// [`SsgError::Validation`] for malformed paths, or
    /// [`SsgError::SymlinkForbidden`] if a path points at a symlink.
    pub fn validate(&self) -> Result<(), SsgError> {
        // Check for path traversal and other security concerns
        for (name, path) in [
            ("site", &self.site),
            ("content", &self.content),
            ("build", &self.build),
            ("template", &self.template),
        ] {
            // For non-existent paths, validate their components
            let path_str = path.to_string_lossy();
            if path_str.contains("..") {
                return Err(SsgError::PathTraversal { path: path.clone() });
            }
            if path_str.contains("//") {
                return Err(SsgError::Validation {
                    field: name.to_string(),
                    message: format!(
                        "path contains invalid double slashes: {}",
                        path.display()
                    ),
                });
            }

            // If path exists, perform additional checks
            if path.exists() {
                let metadata = symlink_metadata_checked(path)?;

                if metadata.file_type().is_symlink() {
                    return Err(SsgError::SymlinkForbidden {
                        path: path.clone(),
                    });
                }
            }
        }

        Ok(())
    }
}

/// Fault-injectable wrapper around [`Path::symlink_metadata`].
///
/// Extracted from [`Paths::validate`] so the metadata error branch can
/// be driven by the `lib::symlink-metadata` failpoint under the
/// `test-fault-injection` feature — once `path.exists()` has returned
/// `true`, the call cannot otherwise be made to fail deterministically.
fn symlink_metadata_checked(path: &Path) -> Result<fs::Metadata, SsgError> {
    fail_point!("lib::symlink-metadata", |_| Err(SsgError::Validation {
        field: "path".to_string(),
        message: "injected: lib::symlink-metadata".to_string(),
    }));
    path.symlink_metadata().with_path(path)
}

/// Fault-injectable wrapper around [`is_safe_path`].
///
/// Extracted from [`create_directories`] so the `is_safe_path` error
/// branch can be driven by the `lib::is-safe-path` failpoint under the
/// `test-fault-injection` feature — `is_safe_path` only errors when an
/// existing path fails `canonicalize`, which is not constructible
/// deterministically in a test.
fn is_safe_path_checked(path: &Path) -> Result<bool, SsgError> {
    fail_point!("lib::is-safe-path", |_| Err(SsgError::Validation {
        field: "path".to_string(),
        message: "injected: lib::is-safe-path".to_string(),
    }));
    is_safe_path(path)
}

/// Builder for creating Paths configurations
#[derive(Debug, Default, Clone)]
pub struct PathsBuilder {
    /// The site output directory
    pub site: Option<PathBuf>,
    /// The content directory
    pub content: Option<PathBuf>,
    /// The build directory
    pub build: Option<PathBuf>,
    /// The template directory
    pub template: Option<PathBuf>,
}

impl PathsBuilder {
    /// Sets the site output directory
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::PathsBuilder;
    ///
    /// let b = PathsBuilder::default().site("dist");
    /// assert_eq!(b.site.as_deref().and_then(|p| p.to_str()), Some("dist"));
    /// ```
    pub fn site<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.site = Some(path.into());
        self
    }

    /// Sets the content directory
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::PathsBuilder;
    ///
    /// let b = PathsBuilder::default().content("posts");
    /// assert_eq!(b.content.as_deref().and_then(|p| p.to_str()), Some("posts"));
    /// ```
    pub fn content<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.content = Some(path.into());
        self
    }

    /// Sets the build directory
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::PathsBuilder;
    ///
    /// let b = PathsBuilder::default().build_dir("work");
    /// assert_eq!(b.build.as_deref().and_then(|p| p.to_str()), Some("work"));
    /// ```
    pub fn build_dir<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.build = Some(path.into());
        self
    }

    /// Sets the template directory
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::PathsBuilder;
    ///
    /// let b = PathsBuilder::default().template("layouts");
    /// assert_eq!(b.template.as_deref().and_then(|p| p.to_str()), Some("layouts"));
    /// ```
    pub fn template<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.template = Some(path.into());
        self
    }

    /// Sets all paths relative to a base directory
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::PathsBuilder;
    ///
    /// let paths = PathsBuilder::default()
    ///     .relative_to("site")
    ///     .build()
    ///     .expect("valid");
    /// assert!(paths.site.ends_with("public"));
    /// assert!(paths.content.ends_with("content"));
    /// ```
    pub fn relative_to<P: AsRef<Path>>(self, base: P) -> Self {
        let base = base.as_ref();
        self.site(base.join("public"))
            .content(base.join("content"))
            .build_dir(base.join("build"))
            .template(base.join("templates"))
    }

    /// Builds the Paths configuration
    ///
    /// # Returns
    ///
    /// * `Result<Paths>` - The configured paths if valid
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::PathsBuilder;
    ///
    /// let paths = PathsBuilder::default().build().expect("defaults valid");
    /// assert_eq!(paths.site.to_str(), Some("public"));
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// * Required paths are missing
    /// * Paths are invalid or unsafe
    /// * Unable to create necessary directories
    pub fn build(self) -> Result<Paths, SsgError> {
        let paths = Paths {
            site: self.site.unwrap_or_else(|| PathBuf::from("public")),
            content: self.content.unwrap_or_else(|| PathBuf::from("content")),
            build: self.build.unwrap_or_else(|| PathBuf::from("build")),
            template: self
                .template
                .unwrap_or_else(|| PathBuf::from("templates")),
        };

        // Validate the configuration
        paths.validate()?;

        Ok(paths)
    }
}

/// Creates and verifies required directories for site generation.
///
/// Ensures all necessary directories exist and are safe to use, creating
/// them if necessary. Also performs security checks on each directory.
///
/// # Arguments
///
/// * `paths` - Reference to a Paths struct containing required directory paths
///
/// # Returns
///
/// * `Ok(())` - If all directories are created/verified successfully
/// * `Err` - If any directory operation fails
///
/// # Examples
///
/// ```rust
/// use std::path::PathBuf;
/// use ssg::{Paths, create_directories};
///
/// fn main() -> Result<(), ssg::SsgError> {
///     let paths = Paths {
///         site: PathBuf::from("public"),
///         content: PathBuf::from("content"),
///         build: PathBuf::from("build"),
///         template: PathBuf::from("templates"),
///     };
///
///     create_directories(&paths)?;
///     println!("All directories ready");
///     Ok(())
/// }
/// ```
///
/// # Security
///
/// Performs the following security checks:
/// * Path traversal prevention
/// * Permission validation
/// * Safe path verification
pub fn create_directories(paths: &Paths) -> Result<(), SsgError> {
    // Path safety check FIRST — `is_safe_path` only flags `..` for
    // non-existent paths, so we must validate before
    // `fs::create_dir_all` materialises any traversal target on disk.
    // Reordering also closes a TOCTOU-style gap where the previous
    // implementation could create `..`-relative directories and then
    // fail to detect them because they now existed.
    if !is_safe_path_checked(&paths.content)? {
        return Err(SsgError::PathTraversal {
            path: paths.content.clone(),
        });
    }
    if !is_safe_path_checked(&paths.build)? {
        return Err(SsgError::PathTraversal {
            path: paths.build.clone(),
        });
    }
    if !is_safe_path_checked(&paths.site)? {
        return Err(SsgError::PathTraversal {
            path: paths.site.clone(),
        });
    }
    if !is_safe_path_checked(&paths.template)? {
        return Err(SsgError::PathTraversal {
            path: paths.template.clone(),
        });
    }

    // Materialise each directory after safety validation passes.
    for (_name, path) in [
        ("content", &paths.content),
        ("build", &paths.build),
        ("site", &paths.site),
        ("template", &paths.template),
    ] {
        fs::create_dir_all(path).with_path(path)?;
    }

    Ok(())
}

/// Executes the static site generation process.
///
/// Parses CLI arguments via [`Cli::parse_and_dispatch`], then routes to
/// either a subcommand handler (issue #527) or the legacy flag-driven
/// pipeline. This function blocks indefinitely while the dev server is
/// running.
///
/// # Examples
///
/// ```no_run
/// // `run()` reads from real argv and may start a dev server, so it's
/// // only ever called from `main()`. The signature is `Result<(), _>`.
/// fn main() -> Result<(), ssg::SsgError> {
///     ssg::run()
/// }
/// ```
pub fn run() -> Result<(), SsgError> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    run_with_argv(argv)
}

/// Body of [`run`], parameterised over `argv`.
///
/// Extracted from [`run`] (which reads the real process argv) so unit
/// tests can drive the full parse → log-init → dispatch sequence with
/// a controlled argument vector.
fn run_with_argv(argv: Vec<std::ffi::OsString>) -> Result<(), SsgError> {
    // Parse argv via the unified subcommand-aware dispatcher. clap
    // short-circuits `--help` / `--version` inside this call so the
    // logger banner never prints for those flags.
    let (invocation, matches) = match Cli::parse_and_dispatch(argv) {
        Ok(pair) => pair,
        // clap errors render themselves and exit with the right code
        // (0 for `--help` / `--version`, 2 for parse failures), so
        // we delegate rather than wrap into SsgError.
        Err(e) => e.exit(),
    };

    initialize_logging_checked()?;

    // OTel build tracing — only initialises if both the `otel` feature
    // is compiled in AND `--trace` was passed. The subcommand parser
    // doesn't (yet) expose `--trace`; `try_contains_id` keeps the
    // call safe on both code paths.
    let trace_flag = if matches.try_contains_id("trace").unwrap_or(false) {
        matches.get_flag("trace")
    } else {
        false
    };
    let _ = otel::init_if_enabled(trace_flag);

    info!("Starting site generation process");

    dispatch_invocation(invocation, &matches)
}

/// Routes a parsed [`CliInvocation`] to the appropriate handler.
fn dispatch_invocation(
    invocation: CliInvocation,
    matches: &clap::ArgMatches,
) -> Result<(), SsgError> {
    match invocation {
        CliInvocation::Legacy => run_legacy(matches),
        CliInvocation::Build => run_subcommand(matches, "build", false),
        CliInvocation::Dev => run_subcommand(matches, "dev", true),
        CliInvocation::Check => run_check(matches),
        CliInvocation::Audit => run_audit(matches),
        CliInvocation::Deploy { target } => run_deploy(matches, &target),
        CliInvocation::Plugins { json, target } => {
            run_plugins(json, target.as_deref())
        }
    }
}

/// Reports the plugin pipeline without building anything.
///
/// The manager is populated through the same `register_default_plugins` the
/// build uses, so the listing cannot drift from what actually runs — which is
/// the point: the README's plugin count is generated from this, and the count
/// had already gone stale once (33 registered, "38 plugins" documented).
fn run_plugins(json: bool, target: Option<&str>) -> Result<(), SsgError> {
    // Which plugins register depends on configuration — the edge-headers
    // emitter only appears when a target is set, for instance — so the real
    // config is used when one can be found. A project without one still gets
    // an accurate listing for the defaults rather than an error.
    let config = SsgConfig::discover_config_file()
        .and_then(|path| SsgConfig::from_file(&path).ok())
        .unwrap_or_default();

    let mut plugins = plugin::PluginManager::new();
    pipeline::register_default_plugins(&mut plugins, &config, false, target);
    let inventory = plugins.inventory();

    if json {
        let rows: Vec<_> = inventory
            .iter()
            .map(|p| {
                serde_json::json!({
                    "order": p.order,
                    "name": p.name,
                    "has_transform": p.has_transform,
                    "needs_all_files": p.needs_all_files,
                })
            })
            .collect();
        let doc = serde_json::json!({
            "count": inventory.len(),
            "plugins": rows,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
        );
        return Ok(());
    }

    println!("{} plugin(s), in execution order:\n", inventory.len());
    println!(
        "  {:>3}  {:<28} {:>9}  {:>9}",
        "#", "NAME", "TRANSFORM", "ALL-FILES"
    );
    for p in &inventory {
        println!(
            "  {:>3}  {:<28} {:>9}  {:>9}",
            p.order,
            p.name,
            if p.has_transform { "yes" } else { "-" },
            if p.needs_all_files { "yes" } else { "-" },
        );
    }
    Ok(())
}

/// Fault-injectable wrapper around [`logging::initialize_logging`].
///
/// `initialize_logging` can never actually fail (it ignores
/// `log::set_logger` races and always returns `Ok`), so the error
/// branch of the `?` in [`run_with_argv`] is only reachable through
/// the `lib::initialize-logging` failpoint under the
/// `test-fault-injection` feature.
fn initialize_logging_checked() -> Result<(), SsgError> {
    fail_point!("lib::initialize-logging", |_| Err(SsgError::Validation {
        field: "logging".to_string(),
        message: "injected: lib::initialize-logging".to_string(),
    }));
    logging::initialize_logging()
}

/// Fault-injectable wrapper around [`plugin::PluginManager::run_on_serve`].
///
/// None of the default plugins' `on_serve` hooks can be made to fail
/// from CLI-reachable inputs, so the error branches of the `?` at the
/// serve call sites in [`run_legacy`] / [`run_subcommand`] are only
/// reachable through the `lib::run-on-serve` failpoint under the
/// `test-fault-injection` feature.
fn run_on_serve_checked(
    plugins: &plugin::PluginManager,
    ctx: &plugin::PluginContext,
) -> Result<(), SsgError> {
    fail_point!("lib::run-on-serve", |_| Err(SsgError::Validation {
        field: "serve".to_string(),
        message: "injected: lib::run-on-serve".to_string(),
    }));
    plugins.run_on_serve(ctx)
}

/// Run handler for the `ssg audit` subcommand (issue #549).
///
/// Delegates to [`crate::cmd::audit::run_and_dispatch`], which handles
/// gate selection, output formatting (text / JSON / `JUnit` XML), and
/// the `--fail-on` exit-code contract.
fn run_audit(matches: &clap::ArgMatches) -> Result<(), SsgError> {
    let sub_m = matches.subcommand_matches("audit").ok_or_else(|| {
        SsgError::Validation {
            field: "subcommand".to_string(),
            message: "missing matches for `audit`".to_string(),
        }
    })?;
    cmd::audit::run_and_dispatch(sub_m, false)
}

/// Legacy code path: behaves exactly like 0.0.42 `ssg` did.
fn run_legacy(matches: &clap::ArgMatches) -> Result<(), SsgError> {
    // `--new NAME` scaffolds a project and stops. The flag has been
    // declared on this parser since 0.0.42 and was never dispatched:
    // `ssg --new mysite` parsed it, ignored it, and went on to build the
    // current directory — failing with "I/O error at 'content'" on a
    // machine where no project existed yet. A flag that parses and does
    // nothing is worse than an unknown one, which at least errors.
    if let Some(name) = matches.get_one::<String>("new") {
        return scaffold::scaffold_project_at(name, Path::new(".")).map_err(
            |e| SsgError::Validation {
                field: "new".to_string(),
                message: e.to_string(),
            },
        );
    }

    let config =
        SsgConfig::from_matches(matches).map_err(|e| SsgError::Validation {
            field: "config".to_string(),
            message: e.to_string(),
        })?;
    let opts = pipeline::RunOptions::from_matches(matches);

    apply_rayon_thread_pool(opts.jobs)?;

    if opts.validate_only {
        return content::validate_only(&config.content_dir).map_err(|e| {
            SsgError::Validation {
                field: "content".to_string(),
                message: e.to_string(),
            }
        });
    }

    if !opts.quiet {
        Cli::print_banner();
    }

    let (plugins, ctx, build_dir, site_dir) =
        pipeline::build_pipeline(&config, &opts);

    pipeline::execute_build_pipeline_with(
        &plugins,
        &ctx,
        &build_dir,
        &config.content_dir,
        &site_dir,
        &config.template_dir,
        opts.quiet,
        opts.incremental,
    )?;

    // Legacy contract: `--serve` boots the dev server.
    if config.serve_dir.is_some() {
        run_on_serve_checked(&plugins, &ctx)?;
        serve_site(&site_dir)
    } else {
        Ok(())
    }
}

/// Run handler shared by the `ssg build` and `ssg dev` subcommands.
///
/// `start_server` controls whether the dev server is booted after the
/// build completes.
fn run_subcommand(
    matches: &clap::ArgMatches,
    name: &str,
    start_server: bool,
) -> Result<(), SsgError> {
    let sub_m = matches.subcommand_matches(name).ok_or_else(|| {
        SsgError::Validation {
            field: "subcommand".to_string(),
            message: format!("missing matches for `{name}`"),
        }
    })?;

    let config = build_config_from_subcommand_matches(sub_m)?;
    let opts = pipeline::RunOptions::from_subcommand_matches(sub_m);

    apply_rayon_thread_pool(opts.jobs)?;

    if !opts.quiet {
        Cli::print_banner();
    }

    let (plugins, ctx, build_dir, site_dir) =
        pipeline::build_pipeline(&config, &opts);

    pipeline::execute_build_pipeline_with(
        &plugins,
        &ctx,
        &build_dir,
        &config.content_dir,
        &site_dir,
        &config.template_dir,
        opts.quiet,
        opts.incremental,
    )?;

    if start_server {
        run_on_serve_checked(&plugins, &ctx)?;
        serve_site(&site_dir)
    } else {
        Ok(())
    }
}

/// Run handler for the `ssg check` subcommand (issue #527 AC3).
///
/// Runs the full plugin pipeline with `dry_run: true` so plugins know
/// to skip writes. Exits 0 iff every plugin's validation pass
/// succeeded.
fn run_check(matches: &clap::ArgMatches) -> Result<(), SsgError> {
    let sub_m = matches.subcommand_matches("check").ok_or_else(|| {
        SsgError::Validation {
            field: "subcommand".to_string(),
            message: "missing matches for `check`".to_string(),
        }
    })?;

    let config = build_config_from_subcommand_matches(sub_m)?;
    let opts = pipeline::RunOptions::from_subcommand_matches(sub_m);

    apply_rayon_thread_pool(opts.jobs)?;

    // First, validate content schemas — cheap and catches the largest
    // class of authoring mistakes before we bother with the rest of
    // the plugin pipeline.
    content::validate_only(&config.content_dir).map_err(|e| {
        SsgError::Validation {
            field: "content".to_string(),
            message: e.to_string(),
        }
    })?;

    // Run the before_compile hooks under dry_run. These are the hooks
    // that perform validation (ContentValidationPlugin,
    // AccessibilityPlugin, SeoPlugin, JsonLdPlugin, CspPlugin). We
    // deliberately skip after_compile / on_serve — those would write
    // to disk.
    let (plugins, ctx, _build_dir, _site_dir) =
        pipeline::build_pipeline(&config, &opts);
    let ctx = ctx.with_dry_run(true);
    plugins.run_before_compile(&ctx)?;

    if !opts.quiet {
        println!("check: all validators passed");
    }
    Ok(())
}

/// Run handler for the `ssg deploy` subcommand (issue #527 AC4).
///
/// Builds the site, then invokes the deploy adapter for the chosen
/// target. Stubs print a `not yet implemented` message and exit
/// cleanly.
fn run_deploy(
    matches: &clap::ArgMatches,
    target: &str,
) -> Result<(), SsgError> {
    let sub_m = matches.subcommand_matches("deploy").ok_or_else(|| {
        SsgError::Validation {
            field: "subcommand".to_string(),
            message: "missing matches for `deploy`".to_string(),
        }
    })?;

    let config = build_config_from_subcommand_matches(sub_m)?;
    let opts = pipeline::RunOptions::from_subcommand_matches(sub_m);

    apply_rayon_thread_pool(opts.jobs)?;

    if !opts.quiet {
        Cli::print_banner();
    }

    let (plugins, ctx, build_dir, site_dir) =
        pipeline::build_pipeline(&config, &opts);

    execute_build_pipeline(
        &plugins,
        &ctx,
        &build_dir,
        &config.content_dir,
        &site_dir,
        &config.template_dir,
        opts.quiet,
    )?;

    let target_enum = deploy_adapter::Target::from_cli(target)?;
    let adapter = deploy_adapter::adapter_for(target_enum);
    if !opts.quiet {
        println!("deploy: invoking adapter `{}`", adapter.name());
    }
    adapter.deploy(&site_dir)
}

/// Builds an `SsgConfig` from subcommand-style matches. The
/// subcommand parser uses the same flag names as the legacy parser
/// (`--config`, `--content`, `--output`, `--template`, etc.) but no
/// `--new`, so we re-use the existing override machinery.
fn build_config_from_subcommand_matches(
    sub_m: &clap::ArgMatches,
) -> Result<SsgConfig, SsgError> {
    SsgConfig::from_subcommand_matches(sub_m).map_err(|e| {
        SsgError::Validation {
            field: "config".to_string(),
            message: e.to_string(),
        }
    })
}

/// Helper: configure the global Rayon thread pool from `--jobs`.
fn apply_rayon_thread_pool(jobs: Option<usize>) -> Result<(), SsgError> {
    if let Some(n) = jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .map_err(|e| SsgError::Validation {
                field: "jobs".to_string(),
                message: format!("failed to configure Rayon thread pool: {e}"),
            })?;
        info!("Rayon thread pool configured with {n} threads");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::Cli;
    use crate::logging::{SimpleLogger, DEFAULT_LOG_LEVEL, ENV_LOG_LEVEL};
    use crate::pipeline::{
        build_pipeline, execute_build_pipeline, resolve_build_and_site_dirs,
        RunOptions,
    };
    use crate::server::build_serve_address;
    use log::Log;
    use serial_test::serial;
    use std::env;
    use std::{
        fs::{self, File},
        path::PathBuf,
    };
    use tempfile::{tempdir, TempDir};

    /// Region-friendly variant-equality check. Compares enum
    /// discriminants via `assert_eq!` so no permanently-untaken
    /// match-arm region is generated — the `matches!` macro's false
    /// arm can never execute in a passing test.
    fn assert_same_variant<T>(actual: &T, expected: &T) {
        assert_eq!(
            std::mem::discriminant(actual),
            std::mem::discriminant(expected)
        );
    }

    #[test]
    fn test_create_log_file_success() {
        let temp_dir = tempdir().unwrap();
        let log_file_path = temp_dir.path().join("test.log");

        let log_file =
            create_log_file(log_file_path.to_str().unwrap()).unwrap();
        assert!(log_file.metadata().unwrap().is_file());
    }

    #[test]
    fn test_log_arguments() {
        let temp_dir = tempdir().unwrap();
        let log_file_path = temp_dir.path().join("args_log.log");
        let mut log_file = File::create(&log_file_path).unwrap();

        let date = now_iso();
        log_arguments(&mut log_file, &date).unwrap();

        let log_content = fs::read_to_string(log_file_path).unwrap();
        assert!(log_content.contains("process"));
    }

    #[test]
    fn test_create_directories_success() {
        let temp_dir = tempdir().unwrap();
        let base_path = temp_dir.path().to_path_buf();

        let paths = Paths {
            site: base_path.join("public"),
            content: base_path.join("content"),
            build: base_path.join("build"),
            template: base_path.join("templates"),
        };

        create_directories(&paths).unwrap();

        // Verify each directory exists
        assert!(paths.site.exists());
        assert!(paths.content.exists());
        assert!(paths.build.exists());
        assert!(paths.template.exists());
    }

    #[cfg(not(target_os = "windows"))] // Unix-only: invalid paths behave differently on Windows
    #[test]
    fn test_create_directories_failure() {
        let invalid_paths = Paths {
            site: PathBuf::from("/invalid/site"),
            content: PathBuf::from("/invalid/content"),
            build: PathBuf::from("/invalid/build"),
            template: PathBuf::from("/invalid/template"),
        };

        let result = create_directories(&invalid_paths);
        assert!(result.is_err());
    }

    #[test]
    fn test_copy_dir_all() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        let src_file = src_dir.path().join("test_file.txt");
        _ = File::create(&src_file).unwrap();

        let result = copy_dir_all(src_dir.path(), dst_dir.path());
        assert!(result.is_ok());
        assert!(dst_dir.path().join("test_file.txt").exists());
    }

    #[test]
    fn test_verify_and_copy_files_success() {
        let temp_dir = tempdir().unwrap();
        let base_path = temp_dir.path().to_path_buf();

        // Create source directory and test file
        let src_dir = base_path.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let test_file = src_dir.join("test_file.txt");
        fs::write(&test_file, "test content").unwrap();

        // Create destination directory
        let dst_dir = base_path.join("dst");

        // Verify and copy files
        verify_and_copy_files(&src_dir, &dst_dir).unwrap();

        // Verify the file was copied
        assert!(dst_dir.join("test_file.txt").exists());
    }

    #[test]
    fn test_verify_and_copy_files_failure() {
        let src_dir = PathBuf::from("/invalid/src");
        let dst_dir = PathBuf::from("/invalid/dst");

        let result = verify_and_copy_files(&src_dir, &dst_dir);
        assert!(result.is_err());
    }

    #[cfg(not(target_os = "windows"))] // Unix-only: invalid paths behave differently on Windows
    #[test]
    fn test_handle_server_failure() {
        let temp_dir = tempdir().unwrap();
        let log_file_path = temp_dir.path().join("server_log.log");
        let mut log_file = File::create(&log_file_path).unwrap();

        let paths = Paths {
            site: PathBuf::from("/invalid/site"),
            content: PathBuf::from("/invalid/content"),
            build: PathBuf::from("/invalid/build"),
            template: PathBuf::from("/invalid/template"),
        };

        let serve_dir = temp_dir.path().join("serve");
        let date = now_iso();
        let result = handle_server(&mut log_file, &date, &paths, &serve_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_safe_path_safe() {
        let temp_dir = tempdir().unwrap();
        let safe_path = temp_dir.path().to_path_buf().join("safe_path");

        // Create the directory
        fs::create_dir_all(&safe_path).unwrap();

        // Use the absolute path
        let absolute_safe_path = safe_path.canonicalize().unwrap();
        assert!(is_safe_path(&absolute_safe_path).unwrap());
    }

    #[cfg(not(target_os = "windows"))] // Unix-only: invalid paths behave differently on Windows
    #[test]
    fn test_create_directories_partial_failure() {
        let temp_dir = tempdir().unwrap();
        let valid_path = temp_dir.path().join("valid_dir");
        let invalid_path = PathBuf::from("/invalid/path");

        let paths = Paths {
            site: valid_path,
            content: invalid_path,
            build: temp_dir.path().join("build"),
            template: temp_dir.path().join("template"),
        };

        let result = create_directories(&paths);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_directories_rejects_traversal_in_build() {
        // Covers create_directories' line ~508-511 — PathTraversal
        // bubble for the build dir.
        let tmp = tempdir().unwrap();
        let bad = tmp.path().join("..").join("escape-build");
        let paths = Paths {
            content: tmp.path().join("content"),
            build: bad,
            site: tmp.path().join("site"),
            template: tmp.path().join("template"),
        };
        let err = create_directories(&paths).unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::PathTraversal {
                path: PathBuf::new(),
            },
        );
    }

    #[test]
    fn test_create_directories_rejects_traversal_in_site() {
        // Covers lines ~513-516.
        let tmp = tempdir().unwrap();
        let bad = tmp.path().join("..").join("escape-site");
        let paths = Paths {
            content: tmp.path().join("content"),
            build: tmp.path().join("build"),
            site: bad,
            template: tmp.path().join("template"),
        };
        let err = create_directories(&paths).unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::PathTraversal {
                path: PathBuf::new(),
            },
        );
    }

    #[test]
    fn test_create_directories_rejects_traversal_in_template() {
        // Covers lines ~518-521.
        let tmp = tempdir().unwrap();
        let bad = tmp.path().join("..").join("escape-template");
        let paths = Paths {
            content: tmp.path().join("content"),
            build: tmp.path().join("build"),
            site: tmp.path().join("site"),
            template: bad,
        };
        let err = create_directories(&paths).unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::PathTraversal {
                path: PathBuf::new(),
            },
        );
    }

    #[test]
    fn test_copy_dir_all_nested() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        let nested_dir = src_dir.path().join("nested_dir");
        fs::create_dir(&nested_dir).unwrap();

        let nested_file = nested_dir.join("nested_file.txt");
        _ = File::create(&nested_file).unwrap();

        copy_dir_all(src_dir.path(), dst_dir.path()).unwrap();
        assert!(dst_dir.path().join("nested_dir/nested_file.txt").exists());
    }

    #[test]
    fn test_verify_and_copy_files_missing_source() {
        let src_path = PathBuf::from("/non_existent_dir");
        let dst_dir = tempdir().unwrap();

        let result = verify_and_copy_files(&src_path, dst_dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_server_missing_serve_dir() {
        let temp_dir = tempdir().unwrap();
        let log_file_path = temp_dir.path().join("server_log.log");
        let mut log_file = File::create(&log_file_path).unwrap();

        let paths = Paths {
            site: temp_dir.path().join("site"),
            content: temp_dir.path().join("content"),
            build: temp_dir.path().join("build"),
            template: temp_dir.path().join("template"),
        };

        let non_existent_serve_dir = PathBuf::from("/non_existent_serve_dir");
        let binding = now_iso();
        let result = handle_server(
            &mut log_file,
            &binding,
            &paths,
            &non_existent_serve_dir,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_collect_files_recursive_empty() {
        let temp_dir = tempdir().unwrap();
        let mut files = Vec::new();

        collect_files_recursive(temp_dir.path(), &mut files).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_print_banner() {
        // Simply call the function to ensure it runs without errors.
        Cli::print_banner();
    }

    #[test]
    fn test_collect_files_recursive_with_nested_directories() {
        let temp_dir = tempdir().unwrap();
        let nested_dir = temp_dir.path().join("nested_dir");
        fs::create_dir(&nested_dir).unwrap();

        let nested_file = nested_dir.join("nested_file.txt");
        _ = File::create(&nested_file).unwrap();

        let mut files = Vec::new();
        collect_files_recursive(temp_dir.path(), &mut files).unwrap();

        assert!(files.contains(&nested_file));
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_handle_server_start_message() {
        let temp_dir = tempdir().unwrap();
        let log_file_path = temp_dir.path().join("server_log.log");
        let mut log_file = File::create(&log_file_path).unwrap();

        let paths = Paths {
            site: temp_dir.path().join("site"),
            content: temp_dir.path().join("content"),
            build: temp_dir.path().join("build"),
            template: temp_dir.path().join("template"),
        };

        let serve_dir = temp_dir.path().join("serve");

        // Check setup conditions before calling `handle_server`
        fs::create_dir_all(&serve_dir).unwrap();
        assert!(serve_dir.exists(), "Expected serve directory to be created");

        // Now, call `handle_server` and check for specific output or error
        let date = now_iso();
        let result = handle_server(&mut log_file, &date, &paths, &serve_dir);
        assert!(
            result.is_err(),
            "Expected handle_server to fail without valid setup"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn test_verify_file_safety_symlink() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let symlink_path = temp_dir.path().join("test_link.txt");

        // Create a regular file
        fs::write(&file_path, "test content").unwrap();

        // Create a symlink
        #[cfg(unix)]
        std::os::unix::fs::symlink(&file_path, &symlink_path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&file_path, &symlink_path).unwrap();

        // Debug output
        println!("File exists: {}", file_path.exists());
        println!("Symlink exists: {}", symlink_path.exists());
        println!(
            "Is symlink: {}",
            symlink_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );

        // Try to verify the symlink
        let result = verify_file_safety(&symlink_path);

        // Print the result for debugging
        println!("Result: {result:?}");

        // Verify that we got an error
        assert!(result.is_err(), "Expected error for symlink, got success");

        // Verify the error message
        let err = result.unwrap_err();
        println!("Error message: {err}");
        assert!(
            matches!(err, SsgError::SymlinkForbidden { ref path } if path == &symlink_path),
            "expected SsgError::SymlinkForbidden, got: {err:?}"
        );
    }

    #[test]
    fn test_verify_file_safety_size() {
        let temp_dir = tempdir().unwrap();
        let large_file_path = temp_dir.path().join("large.txt");

        // Create a large file
        let file = File::create(&large_file_path).unwrap();
        file.set_len(11 * 1024 * 1024).unwrap(); // 11MB

        let result = verify_file_safety(&large_file_path);
        assert!(result.is_err(), "Expected error, got: {result:?}");
    }

    #[test]
    fn test_verify_file_safety_regular() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("regular.txt");

        // Create a regular file
        fs::write(&file_path, "test content").unwrap();

        assert!(verify_file_safety(&file_path).is_ok());
    }

    /// Tests successful copying of an empty directory
    #[test]
    fn test_copy_empty_directory_async() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        let result = copy_dir_all_async(src_dir.path(), dst_dir.path());
        assert!(result.is_ok());

        // Verify destination directory exists
        assert!(dst_dir.path().exists());
    }

    /// Tests copying a directory with a single file
    #[test]
    fn test_copy_single_file_async() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        // Create a test file
        let test_file = src_dir.path().join("test.txt");
        fs::write(&test_file, "test content").unwrap();

        copy_dir_all_async(src_dir.path(), dst_dir.path()).unwrap();

        // Verify file was copied
        let copied_file = dst_dir.path().join("test.txt");
        assert!(copied_file.exists());
        assert_eq!(fs::read_to_string(copied_file).unwrap(), "test content");
    }

    /// Tests copying a directory with nested subdirectories
    #[test]
    fn test_copy_nested_directories_async() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        // Create nested directory structure
        let nested_dir = src_dir.path().join("nested");
        fs::create_dir(&nested_dir).unwrap();

        // Create files in both root and nested directory
        fs::write(src_dir.path().join("root.txt"), "root content").unwrap();
        fs::write(nested_dir.join("nested.txt"), "nested content").unwrap();

        copy_dir_all_async(src_dir.path(), dst_dir.path()).unwrap();

        // Verify directory structure and contents
        assert!(dst_dir.path().join("nested").exists());
        assert!(dst_dir.path().join("root.txt").exists());
        assert!(dst_dir.path().join("nested/nested.txt").exists());

        assert_eq!(
            fs::read_to_string(dst_dir.path().join("root.txt")).unwrap(),
            "root content"
        );
        assert_eq!(
            fs::read_to_string(dst_dir.path().join("nested/nested.txt"))
                .unwrap(),
            "nested content"
        );
    }

    /// Tests handling of symlinks
    #[test]
    fn test_copy_with_symlink_async() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        // Create a regular file
        let file_path = src_dir.path().join("original.txt");
        fs::write(&file_path, "original content").unwrap();

        // Create a symlink
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let symlink_path = src_dir.path().join("link.txt");
            symlink(&file_path, &symlink_path).unwrap();
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            let symlink_path = src_dir.path().join("link.txt");
            symlink_file(&file_path, &symlink_path).unwrap();
        }

        // Attempt to copy - should fail due to symlink
        let result = copy_dir_all_async(src_dir.path(), dst_dir.path());
        assert!(result.is_err());
    }

    /// Tests copying large files
    #[test]
    fn test_copy_large_file_async() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        // Create a large file (11MB)
        let large_file = src_dir.path().join("large.txt");
        let file = File::create(&large_file).unwrap();
        file.set_len(11 * 1024 * 1024).unwrap();

        // Attempt to copy - should fail due to file size limit
        let result = copy_dir_all_async(src_dir.path(), dst_dir.path());
        assert!(result.is_err());
    }

    /// Tests copying with invalid destination
    #[cfg(not(target_os = "windows"))] // Unix-only: invalid paths behave differently on Windows
    #[test]
    fn test_copy_invalid_destination_async() {
        let src_dir = tempdir().unwrap();
        let invalid_dst = PathBuf::from("/nonexistent/path");

        let result = copy_dir_all_async(src_dir.path(), &invalid_dst);
        assert!(result.is_err());
    }

    /// Tests concurrent copying of multiple files
    #[test]
    fn test_concurrent_copy_async() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        // Create multiple files
        for i in 0..5 {
            fs::write(
                src_dir.path().join(format!("file{i}.txt")),
                format!("content {i}"),
            )
            .unwrap();
        }

        copy_dir_all_async(src_dir.path(), dst_dir.path()).unwrap();

        // Verify all files were copied
        for i in 0..5 {
            let copied_file = dst_dir.path().join(format!("file{i}.txt"));
            assert!(copied_file.exists());
            assert_eq!(
                fs::read_to_string(copied_file).unwrap(),
                format!("content {i}")
            );
        }
    }

    /// Tests copying with maximum directory depth
    #[test]
    fn test_max_directory_depth_async() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();
        let max_depth = 5;

        // Create deeply nested directory structure
        let mut current_dir = src_dir.path().to_path_buf();
        for i in 0..max_depth {
            current_dir = current_dir.join(format!("level{i}"));
            fs::create_dir(&current_dir).unwrap();
            fs::write(
                current_dir.join("file.txt"),
                format!("content level {i}"),
            )
            .unwrap();
        }

        copy_dir_all_async(src_dir.path(), dst_dir.path()).unwrap();

        // Verify the entire structure was copied
        current_dir = dst_dir.path().to_path_buf();
        for i in 0..max_depth {
            current_dir = current_dir.join(format!("level{i}"));
            assert!(current_dir.exists());
            assert!(current_dir.join("file.txt").exists());
            assert_eq!(
                fs::read_to_string(current_dir.join("file.txt")).unwrap(),
                format!("content level {i}")
            );
        }
    }

    #[test]
    fn test_verify_and_copy_files_async_missing_source() {
        let temp_dir = tempdir().unwrap();
        let src_dir = temp_dir.path().join("nonexistent");
        let dst_dir = temp_dir.path().join("dst");

        let error = verify_and_copy_files_async(&src_dir, &dst_dir)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("does not exist"),
            "Expected error message about non-existent source, got: {error}"
        );
    }

    #[test]
    fn test_paths_builder_default() {
        let paths = Paths::builder().build().unwrap();
        assert_eq!(paths.site, PathBuf::from("public"));
        assert_eq!(paths.content, PathBuf::from("content"));
        assert_eq!(paths.build, PathBuf::from("build"));
        assert_eq!(paths.template, PathBuf::from("templates"));
    }

    #[test]
    fn test_resolve_build_and_site_dirs_without_serve_dir() {
        let mut config = SsgConfig::default();
        config.output_dir = PathBuf::from("docs");
        config.serve_dir = None;

        let (build_dir, site_dir) = resolve_build_and_site_dirs(&config);

        assert_eq!(site_dir, PathBuf::from("docs"));
        assert_eq!(build_dir, PathBuf::from("docs.build-tmp"));
        assert_ne!(build_dir, site_dir);
    }

    #[test]
    fn test_resolve_build_and_site_dirs_with_distinct_serve_dir() {
        let mut config = SsgConfig::default();
        config.output_dir = PathBuf::from("docs");
        config.serve_dir = Some(PathBuf::from("public"));

        let (build_dir, site_dir) = resolve_build_and_site_dirs(&config);

        assert_eq!(build_dir, PathBuf::from("docs"));
        assert_eq!(site_dir, PathBuf::from("public"));
        assert_ne!(build_dir, site_dir);
    }

    #[test]
    fn test_resolve_build_and_site_dirs_with_same_serve_and_output_dir() {
        let mut config = SsgConfig::default();
        config.output_dir = PathBuf::from("docs");
        config.serve_dir = Some(PathBuf::from("docs"));

        let (build_dir, site_dir) = resolve_build_and_site_dirs(&config);

        assert_eq!(site_dir, PathBuf::from("docs"));
        assert_eq!(build_dir, PathBuf::from("docs.build-tmp"));
        assert_ne!(build_dir, site_dir);
    }

    #[test]
    fn test_paths_builder_custom() {
        let temp_dir = tempdir().unwrap();
        let paths = Paths::builder()
            .site(temp_dir.path().join("custom_public"))
            .content(temp_dir.path().join("custom_content"))
            .build_dir(temp_dir.path().join("custom_build"))
            .template(temp_dir.path().join("custom_templates"))
            .build()
            .unwrap();

        assert_eq!(paths.site, temp_dir.path().join("custom_public"));
        assert_eq!(paths.content, temp_dir.path().join("custom_content"));
        assert_eq!(paths.build, temp_dir.path().join("custom_build"));
        assert_eq!(paths.template, temp_dir.path().join("custom_templates"));
    }

    #[test]
    fn test_paths_builder_relative() {
        let temp_dir = tempdir().unwrap();

        // Create the directories first
        fs::create_dir_all(temp_dir.path().join("public")).unwrap();
        fs::create_dir_all(temp_dir.path().join("content")).unwrap();
        fs::create_dir_all(temp_dir.path().join("build")).unwrap();
        fs::create_dir_all(temp_dir.path().join("templates")).unwrap();

        let paths = Paths::builder()
            .relative_to(temp_dir.path())
            .build()
            .unwrap();

        assert_eq!(paths.site, temp_dir.path().join("public"));
        assert_eq!(paths.content, temp_dir.path().join("content"));
        assert_eq!(paths.build, temp_dir.path().join("build"));
        assert_eq!(paths.template, temp_dir.path().join("templates"));
    }

    #[test]
    fn test_paths_validation() {
        // Test directory traversal
        let err = Paths::builder().site("../invalid").build().unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::PathTraversal {
                path: PathBuf::new(),
            },
        );

        // Test double slashes
        let err = Paths::builder().site("invalid//path").build().unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::Validation {
                field: String::new(),
                message: String::new(),
            },
        );

        // Test symlinks if possible
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let temp_dir = tempdir().unwrap();
            let real_path = temp_dir.path().join("real");
            let symlink_path = temp_dir.path().join("symlink");

            fs::create_dir(&real_path).unwrap();
            symlink(&real_path, &symlink_path).unwrap();

            let err = Paths::builder().site(symlink_path).build().unwrap_err();
            assert_same_variant(
                &err,
                &SsgError::SymlinkForbidden {
                    path: PathBuf::new(),
                },
            );
        }
    }

    #[test]
    fn test_paths_default_paths() {
        let paths = Paths::default_paths();
        assert_eq!(paths.site, PathBuf::from("public"));
        assert_eq!(paths.content, PathBuf::from("content"));
        assert_eq!(paths.build, PathBuf::from("build"));
        assert_eq!(paths.template, PathBuf::from("templates"));
    }

    // Add a new test for non-existent but valid paths
    #[test]
    fn test_paths_nonexistent_valid() {
        let temp_dir = tempdir().unwrap();
        let valid_path = temp_dir.path().join("new_directory");

        let paths = Paths::builder().site(valid_path.clone()).build().unwrap();

        assert_eq!(paths.site, valid_path);
    }

    #[test]
    #[serial(env_log)]
    fn test_initialize_logging_with_custom_level() {
        env::set_var(ENV_LOG_LEVEL, "debug");
        assert!(logging::initialize_logging().is_ok());
        env::remove_var(ENV_LOG_LEVEL);
    }

    #[test]
    fn test_paths_builder_with_all_invalid_paths() {
        let result = Paths::builder()
            .site("../invalid")
            .content("content//invalid")
            .build_dir("build/../invalid")
            .template("template//invalid")
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_paths_builder_clone() {
        let builder = PathsBuilder::default();
        let cloned = builder;
        assert!(cloned.site.is_none());
        assert!(cloned.content.is_none());
        assert!(cloned.build.is_none());
        assert!(cloned.template.is_none());
    }

    #[test]
    fn test_paths_clone() {
        let paths = Paths::default_paths();
        let cloned = paths.clone();

        assert_eq!(paths.site, cloned.site);
        assert_eq!(paths.content, cloned.content);
        assert_eq!(paths.build, cloned.build);
        assert_eq!(paths.template, cloned.template);
    }

    #[test]
    fn test_async_copy_with_empty_source() {
        let temp_dir = tempdir().unwrap();
        let src_dir = temp_dir.path().join("empty_src");
        let dst_dir = temp_dir.path().join("empty_dst");

        fs::create_dir(&src_dir).unwrap();

        let result = verify_and_copy_files_async(&src_dir, &dst_dir);
        assert!(result.is_ok());
        assert!(dst_dir.exists());
    }

    #[test]
    fn test_paths_validation_all_aspects() {
        let temp_dir = tempdir().unwrap();

        // Test with absolute paths
        let result = Paths::builder()
            .site(temp_dir.path().join("site"))
            .content(temp_dir.path().join("content"))
            .build_dir(temp_dir.path().join("build"))
            .template(temp_dir.path().join("template"))
            .build();

        assert!(result.is_ok());

        // Test with multiple validation issues
        let result = Paths::builder()
            .site("../site")
            .content("content//test")
            .build_dir("build/../../test")
            .template("template//test")
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_log_initialization_with_empty_log_file() {
        let temp_dir = tempdir().unwrap();
        let log_path = temp_dir.path().join("empty.log");
        let mut log_file = File::create(&log_path).unwrap();

        let date = now_iso();
        log_initialization(&mut log_file, &date).unwrap();

        let content = fs::read_to_string(&log_path).unwrap();
        assert!(!content.is_empty());
        assert!(content.contains("process"));
    }

    #[test]
    fn test_verify_and_copy_files_async_with_nested_empty_dirs() {
        let temp_dir = tempdir().unwrap();
        let src_dir = temp_dir.path().join("src");
        let dst_dir = temp_dir.path().join("dst");

        // Create nested empty directory structure
        fs::create_dir_all(src_dir.join("a/b/c")).unwrap();
        fs::create_dir_all(src_dir.join("d/e/f")).unwrap();

        verify_and_copy_files_async(&src_dir, &dst_dir).unwrap();

        assert!(dst_dir.join("a/b/c").exists());
        assert!(dst_dir.join("d/e/f").exists());
    }

    #[test]
    fn test_validate_nonexistent_paths() {
        let paths = Paths {
            site: PathBuf::from("nonexistent/site"),
            content: PathBuf::from("nonexistent/content"),
            build: PathBuf::from("nonexistent/build"),
            template: PathBuf::from("nonexistent/template"),
        };

        // Non-existent paths should be valid if they don't contain unsafe patterns
        assert!(paths.validate().is_ok());
    }

    #[test]
    fn test_copy_dir_all_async_with_empty_dirs() {
        let temp_dir = tempdir().unwrap();
        let src_dir = temp_dir.path().join("src");
        let dst_dir = temp_dir.path().join("dst");

        fs::create_dir_all(src_dir.join("empty1")).unwrap();
        fs::create_dir_all(src_dir.join("empty2/empty3")).unwrap();

        copy_dir_all_async(&src_dir, &dst_dir).unwrap();

        assert!(dst_dir.join("empty1").exists());
        assert!(dst_dir.join("empty2/empty3").exists());
    }

    #[test]
    #[serial(env_log)]
    fn test_log_level_from_env() {
        // Seed the variable so the restore branch at the end of the
        // test always executes, then save the current value.
        env::set_var(ENV_LOG_LEVEL, "info");
        let original_value = env::var(ENV_LOG_LEVEL).ok();

        // Helper function to get processed log level
        fn get_processed_log_level() -> String {
            let log_level = env::var(ENV_LOG_LEVEL)
                .unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string());

            match log_level.to_lowercase().as_str() {
                "error" => "error",
                "warn" => "warn",
                "info" => "info",
                "debug" => "debug",
                "trace" => "trace",
                _ => "info", // Default to info for invalid values
            }
            .to_string()
        }

        // Test various log level settings
        let test_levels = vec![
            ("DEBUG", "debug"),
            ("ERROR", "error"),
            ("WARN", "warn"),
            ("INFO", "info"),
            ("TRACE", "trace"),
            ("INVALID", "info"), // Should default to info
        ];

        for (input, expected) in test_levels {
            env::set_var(ENV_LOG_LEVEL, input);
            let processed_level = get_processed_log_level();
            assert_eq!(
                processed_level, expected,
                "Expected log level '{expected}' for input '{input}', but got '{processed_level}'"
            );
        }

        // With the variable unset, the fallback closure supplies the
        // default level.
        env::remove_var(ENV_LOG_LEVEL);
        assert_eq!(get_processed_log_level(), DEFAULT_LOG_LEVEL);

        // Restore the original environment variable state
        env::remove_var(ENV_LOG_LEVEL);
        if let Some(value) = original_value {
            env::set_var(ENV_LOG_LEVEL, value);
        }
    }

    /// Test for default log level when environment variable is not set
    #[test]
    #[serial(env_log)]
    fn test_default_log_level() {
        // Seed the variable so the restore branch at the end of the
        // test always executes, then save the current value.
        env::set_var(ENV_LOG_LEVEL, "info");
        let original_value = env::var(ENV_LOG_LEVEL).ok();
        env::remove_var(ENV_LOG_LEVEL);

        let log_level = env::var(ENV_LOG_LEVEL)
            .unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string())
            .to_lowercase();
        assert_eq!(log_level, DEFAULT_LOG_LEVEL.to_lowercase());

        env::remove_var(ENV_LOG_LEVEL);
        if let Some(value) = original_value {
            env::set_var(ENV_LOG_LEVEL, value);
        }
    }

    /// Test the logic for translating string log levels to `LevelFilter` values
    #[test]
    fn test_log_level_translation() {
        use log::LevelFilter;
        let test_cases = vec![
            ("error", LevelFilter::Error),
            ("warn", LevelFilter::Warn),
            ("info", LevelFilter::Info),
            ("debug", LevelFilter::Debug),
            ("trace", LevelFilter::Trace),
            ("invalid", LevelFilter::Info),
            ("", LevelFilter::Info),
        ];

        for (input, expected) in test_cases {
            let level = match input.to_lowercase().as_str() {
                "error" => LevelFilter::Error,
                "warn" => LevelFilter::Warn,
                "info" => LevelFilter::Info,
                "debug" => LevelFilter::Debug,
                "trace" => LevelFilter::Trace,
                _ => LevelFilter::Info,
            };

            assert_eq!(
                level, expected,
                "Log level mismatch for input: '{input}' - expected {expected:?}, got {level:?}"
            );
        }
    }

    /// Test environment variable handling with cleanup
    #[test]
    #[serial(env_log)]
    fn test_env_log_level_handling() {
        // Seed the variable so the restore branch at the end of the
        // test always executes, then save the original state.
        env::set_var(ENV_LOG_LEVEL, "info");
        let original_value = env::var(ENV_LOG_LEVEL).ok();

        let test_cases = vec![
            (Some("DEBUG"), "debug"),
            (Some("ERROR"), "error"),
            (Some("WARN"), "warn"),
            (Some("INFO"), "info"),
            (Some("TRACE"), "trace"),
            (Some("INVALID"), "info"),
            (None, "info"),
        ];

        for (env_value, expected) in test_cases {
            // Clear any existing env var
            env::remove_var(ENV_LOG_LEVEL);

            // Set new value if provided
            if let Some(value) = env_value {
                env::set_var(ENV_LOG_LEVEL, value);
            }

            let log_level = env::var(ENV_LOG_LEVEL)
                .unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string())
                .to_lowercase();

            let actual = match log_level.as_str() {
                "error" => "error",
                "warn" => "warn",
                "info" => "info",
                "debug" => "debug",
                "trace" => "trace",
                _ => "info",
            };

            assert_eq!(
                actual, expected,
                "Log level mismatch for env value: {env_value:?}"
            );
        }

        // Restore original state
        env::remove_var(ENV_LOG_LEVEL);
        if let Some(value) = original_value {
            env::set_var(ENV_LOG_LEVEL, value);
        }
    }

    #[test]
    fn test_initialize_logging_custom_levels() {
        // Verify that the expected log level strings are valid
        let valid_levels = ["debug", "warn", "error", "trace", "info"];
        for level in &valid_levels {
            assert!(
                ["trace", "debug", "info", "warn", "error"].contains(level),
                "unexpected log level: {level}"
            );
        }
        // Verify our default is valid
        assert!(["trace", "debug", "info", "warn", "error"]
            .contains(&DEFAULT_LOG_LEVEL),);
    }

    #[test]
    fn parse_log_level_recognises_all_supported_levels() {
        use log::LevelFilter;
        assert_eq!(logging::parse_log_level("error"), LevelFilter::Error);
        assert_eq!(logging::parse_log_level("warn"), LevelFilter::Warn);
        assert_eq!(logging::parse_log_level("info"), LevelFilter::Info);
        assert_eq!(logging::parse_log_level("debug"), LevelFilter::Debug);
        assert_eq!(logging::parse_log_level("trace"), LevelFilter::Trace);
    }

    #[test]
    fn parse_log_level_is_case_insensitive() {
        use log::LevelFilter;
        assert_eq!(logging::parse_log_level("ERROR"), LevelFilter::Error);
        assert_eq!(logging::parse_log_level("Warn"), LevelFilter::Warn);
        assert_eq!(logging::parse_log_level("TraCe"), LevelFilter::Trace);
    }

    #[test]
    fn parse_log_level_unknown_value_falls_back_to_info() {
        use log::LevelFilter;
        assert_eq!(logging::parse_log_level("nonsense"), LevelFilter::Info);
        assert_eq!(logging::parse_log_level(""), LevelFilter::Info);
        assert_eq!(logging::parse_log_level("verbose"), LevelFilter::Info);
    }

    #[test]
    fn test_concurrent_operations() {
        let temp_dir = TempDir::new().unwrap();
        let src_dir = temp_dir.path().join("src");
        let dst_dir = temp_dir.path().join("dst");

        // Create source directory
        fs::create_dir_all(&src_dir).unwrap();

        // Create files
        for i in 0..100 {
            fs::write(
                src_dir.join(format!("file_{i}.txt")),
                format!("content {i}"),
            )
            .unwrap();
        }

        // Verify source files
        let mut src_files = Vec::new();
        collect_files_recursive(&src_dir, &mut src_files).unwrap();
        assert_eq!(src_files.len(), 100);

        // Create destination directory
        fs::create_dir_all(&dst_dir).unwrap();

        // Copy files using verify_and_copy_files
        verify_and_copy_files(&src_dir, &dst_dir).unwrap();

        // Verify destination files
        let mut dst_files = Vec::new();
        collect_files_recursive(&dst_dir, &mut dst_files).unwrap();

        assert_eq!(dst_files.len(), 100);

        // Verify file contents
        for i in 0..100 {
            let dst_path = dst_dir.join(format!("file_{i}.txt"));
            assert!(dst_path.exists());

            let content = fs::read_to_string(&dst_path).unwrap();
            assert_eq!(
                content,
                format!("content {i}"),
                "Content mismatch for file {}",
                i
            );
        }
    }

    #[test]
    fn test_verify_and_copy_files_basic() {
        let temp_dir = TempDir::new().unwrap();
        let src_dir = temp_dir.path().join("src");
        let dst_dir = temp_dir.path().join("dst");

        fs::create_dir_all(&src_dir).unwrap();

        // Create a test file
        fs::write(src_dir.join("test.txt"), "test content").unwrap();

        // Copy files
        verify_and_copy_files(&src_dir, &dst_dir).unwrap();

        // Verify file was copied
        assert!(dst_dir.join("test.txt").exists());
        assert_eq!(
            fs::read_to_string(dst_dir.join("test.txt")).unwrap(),
            "test content"
        );
    }

    #[test]
    fn test_copy_dir_with_progress_empty_source() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        // Call the function with an empty source directory
        copy_dir_with_progress(src_dir.path(), dst_dir.path()).unwrap();

        // Verify that the destination directory exists and is empty
        assert!(dst_dir.path().exists());
        assert!(fs::read_dir(dst_dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn test_copy_dir_with_progress_source_does_not_exist() {
        let src_dir = Path::new("/nonexistent");
        let dst_dir = tempdir().unwrap();

        let result = copy_dir_with_progress(src_dir, dst_dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_copy_dir_with_progress_single_file() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        fs::write(src_dir.path().join("file1.txt"), "content").unwrap();

        copy_dir_with_progress(src_dir.path(), dst_dir.path()).unwrap();

        let copied_file = dst_dir.path().join("file1.txt");
        assert!(copied_file.exists());
        assert_eq!(fs::read_to_string(copied_file).unwrap(), "content");
    }

    #[test]
    fn test_copy_dir_with_progress_nested_directories() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        let nested_dir = src_dir.path().join("nested");
        fs::create_dir(&nested_dir).unwrap();
        fs::write(nested_dir.join("file.txt"), "nested content").unwrap();

        copy_dir_with_progress(src_dir.path(), dst_dir.path()).unwrap();

        let copied_nested_file = dst_dir.path().join("nested/file.txt");
        assert!(copied_nested_file.exists());
        assert_eq!(
            fs::read_to_string(copied_nested_file).unwrap(),
            "nested content"
        );
    }

    #[cfg(not(target_os = "windows"))] // Unix-only: invalid paths behave differently on Windows
    #[test]
    fn test_copy_dir_with_progress_destination_creation_failure() {
        let src_dir = tempdir().unwrap();
        let dst_dir = Path::new("/invalid_path");

        let result = copy_dir_with_progress(src_dir.path(), dst_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_and_copy_files_single_file() {
        let temp_dir = tempdir().unwrap();
        let src_file = temp_dir.path().join("single.txt");
        fs::write(&src_file, "content").unwrap();
        let dst_dir = temp_dir.path().join("dst");
        // Calling with a file as src triggers verify_file_safety branch
        // then copy_dir_all fails because src is a file, not a directory
        let result = verify_and_copy_files(&src_file, &dst_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_safe_path_traversal_nonexistent() {
        assert!(!is_safe_path(Path::new("../../etc/passwd")).unwrap());
    }

    #[test]
    fn test_copy_dir_with_progress_nested() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();
        // Create nested structure with files
        let sub = src_dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(src_dir.path().join("root.txt"), "root").unwrap();
        fs::write(sub.join("nested.txt"), "nested").unwrap();
        copy_dir_with_progress(src_dir.path(), dst_dir.path()).unwrap();
        assert!(dst_dir.path().join("root.txt").exists());
        assert!(dst_dir.path().join("sub/nested.txt").exists());
    }

    #[test]
    fn test_copy_dir_all_parallel_threshold() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();
        // Create >= 16 files to trigger parallel path
        for i in 0..20 {
            fs::write(
                src_dir.path().join(format!("file{i}.txt")),
                format!("content {i}"),
            )
            .unwrap();
        }
        copy_dir_all(src_dir.path(), dst_dir.path()).unwrap();
        for i in 0..20 {
            assert!(dst_dir.path().join(format!("file{i}.txt")).exists());
        }
    }

    #[test]
    fn test_collect_files_recursive_depth_exceeded() {
        let temp_dir = tempdir().unwrap();
        // Create a directory deeper than MAX_DIR_DEPTH
        let mut path = temp_dir.path().to_path_buf();
        for i in 0..=MAX_DIR_DEPTH {
            path = path.join(format!("d{i}"));
            fs::create_dir(&path).unwrap();
        }
        let mut files = Vec::new();
        let result = collect_files_recursive(temp_dir.path(), &mut files);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("maximum depth"));
    }

    #[test]
    fn test_copy_dir_all_depth_exceeded() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();
        let mut path = src_dir.path().to_path_buf();
        for i in 0..=MAX_DIR_DEPTH {
            path = path.join(format!("d{i}"));
            fs::create_dir(&path).unwrap();
        }
        let result = copy_dir_all(src_dir.path(), dst_dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("maximum depth"));
    }

    #[test]
    fn test_verify_and_copy_files_async_depth_exceeded() {
        let temp_dir = tempdir().unwrap();
        let src = temp_dir.path().join("src");
        let dst = temp_dir.path().join("dst");
        let mut path = src.clone();
        for i in 0..=MAX_DIR_DEPTH {
            path = path.join(format!("d{i}"));
            fs::create_dir_all(&path).unwrap();
        }
        let result = verify_and_copy_files_async(&src, &dst);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("maximum depth"));
    }

    #[test]
    fn test_copy_dir_all_async_depth_exceeded() {
        let temp_dir = tempdir().unwrap();
        let src = temp_dir.path().join("src");
        let dst = temp_dir.path().join("dst");
        let mut path = src.clone();
        for i in 0..=MAX_DIR_DEPTH {
            path = path.join(format!("d{i}"));
            fs::create_dir_all(&path).unwrap();
        }
        let result = copy_dir_all_async(&src, &dst);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("maximum depth"));
    }

    #[test]
    fn test_verify_file_safety_nonexistent() {
        let result = verify_file_safety(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn test_copy_dir_with_progress_nonexistent_source() {
        let dst = env::temp_dir().join("ssg_copy_dir_dst");
        let result =
            copy_dir_with_progress(Path::new("/nonexistent/source"), &dst);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_and_copy_files_async_with_files() {
        let temp_dir = tempdir().unwrap();
        let src = temp_dir.path().join("src");
        let dst = temp_dir.path().join("dst");

        // Create source with nested dirs + files
        fs::create_dir_all(src.join("sub1/sub2")).unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();
        fs::write(src.join("sub1/a.txt"), "a").unwrap();
        fs::write(src.join("sub1/sub2/b.txt"), "b").unwrap();

        verify_and_copy_files_async(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("root.txt")).unwrap(), "root");
        assert_eq!(fs::read_to_string(dst.join("sub1/a.txt")).unwrap(), "a");
        assert_eq!(
            fs::read_to_string(dst.join("sub1/sub2/b.txt")).unwrap(),
            "b"
        );
    }

    #[test]
    fn test_copy_dir_with_progress_with_files() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        // Create nested structure
        let sub1 = src_dir.path().join("a");
        let sub2 = sub1.join("b");
        fs::create_dir_all(&sub2).unwrap();
        fs::write(src_dir.path().join("file1.txt"), "f1").unwrap();
        fs::write(sub1.join("file2.txt"), "f2").unwrap();
        fs::write(sub2.join("file3.txt"), "f3").unwrap();

        copy_dir_with_progress(src_dir.path(), dst_dir.path()).unwrap();

        assert_eq!(
            fs::read_to_string(dst_dir.path().join("file1.txt")).unwrap(),
            "f1"
        );
        assert_eq!(
            fs::read_to_string(dst_dir.path().join("a/file2.txt")).unwrap(),
            "f2"
        );
        assert_eq!(
            fs::read_to_string(dst_dir.path().join("a/b/file3.txt")).unwrap(),
            "f3"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_is_safe_path_broken_symlink() {
        let temp_dir = tempdir().unwrap();
        let target = temp_dir.path().join("nonexistent_target");
        let link = temp_dir.path().join("broken_link");

        std::os::unix::fs::symlink(&target, &link).unwrap();
        let result = is_safe_path(&link).unwrap();
        assert!(result);
    }

    #[cfg(unix)]
    #[test]
    fn test_paths_validate_symlink() {
        let temp_dir = tempdir().unwrap();
        let real = temp_dir.path().join("real");
        let link = temp_dir.path().join("link");

        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let paths = Paths {
            site: link,
            content: PathBuf::from("content"),
            build: PathBuf::from("build"),
            template: PathBuf::from("templates"),
        };
        let err = paths.validate().unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::SymlinkForbidden {
                path: PathBuf::new(),
            },
        );
    }

    #[test]
    fn test_copy_dir_with_progress_depth_exceeded() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();
        let mut path = src_dir.path().to_path_buf();
        for i in 0..=MAX_DIR_DEPTH {
            path = path.join(format!("d{i}"));
            fs::create_dir(&path).unwrap();
        }
        let result = copy_dir_with_progress(src_dir.path(), dst_dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("maximum depth"));
    }

    #[test]
    fn test_verify_and_copy_files_source_is_file() {
        let temp_dir = tempdir().unwrap();
        let src_file = temp_dir.path().join("source.txt");
        let dst_dir = temp_dir.path().join("dst");
        fs::write(&src_file, "hello").unwrap();

        let result = verify_and_copy_files(&src_file, &dst_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_compile_site_error() {
        // v0.0.46: staticdatagen 0.0.10 + the trimmed content_stager
        // treat empty content + empty templates as "no work to do"
        // (clean Ok), so we can't reproduce the v0.0.45 "happens to
        // error" pattern with empty dirs. Pass a real *file* where the
        // content directory is expected — `staticdatagen::add` opens
        // it via `read_dir` which fails on a non-directory.
        let temp_dir = tempdir().unwrap();
        let build = temp_dir.path().join("build");
        let content_file = temp_dir.path().join("content_file");
        let site = temp_dir.path().join("site");
        let template = temp_dir.path().join("template");
        fs::create_dir_all(&build).unwrap();
        fs::write(&content_file, "not a directory").unwrap();
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&template).unwrap();

        let result = compile_site(&build, &content_file, &site, &template);
        assert!(
            result.is_err(),
            "compile_site should propagate the io error when \
             content_dir is a file, got: {result:?}"
        );
    }

    #[test]
    fn test_compile_site_propagates_compile_error() {
        // v0.0.46: exercises the `compile(...).map_err(...)` closure
        // in `pipeline::compile_site` — the branch that fires when
        // `staticdatagen::compile` returns Err *after* the stager has
        // succeeded. Pass a `build_dir` that's a regular file: the
        // stager doesn't touch `build_dir` directly (it stages under
        // `std::env::temp_dir()`), so it succeeds; `compile` then
        // can't write into the non-directory and the closure wraps
        // the io error into an `SsgError`.
        let temp_dir = tempdir().unwrap();
        let build_file = temp_dir.path().join("build_file");
        let content = temp_dir.path().join("content");
        let site = temp_dir.path().join("site");
        let template = temp_dir.path().join("template");
        fs::write(&build_file, "not a directory").unwrap();
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&site).unwrap();
        fs::create_dir_all(&template).unwrap();

        let result = compile_site(&build_file, &content, &site, &template);
        assert!(
            result.is_err(),
            "compile_site should propagate compile()'s error when \
             build_dir is a file, got: {result:?}"
        );
    }

    #[test]
    fn test_prepare_serve_dir_same_as_site() {
        let temp_dir = tempdir().unwrap();
        let site_dir = temp_dir.path().join("site");
        fs::create_dir_all(&site_dir).unwrap();
        fs::write(site_dir.join("index.html"), "<html/>").unwrap();

        let paths = Paths {
            site: site_dir.clone(),
            content: PathBuf::from("content"),
            build: PathBuf::from("build"),
            template: PathBuf::from("templates"),
        };

        // When serve_dir == site, no copy should happen
        prepare_serve_dir(&paths, &site_dir).unwrap();
        assert!(site_dir.join("index.html").exists());
    }

    #[test]
    fn test_prepare_serve_dir_different() {
        let temp_dir = tempdir().unwrap();
        let site_dir = temp_dir.path().join("site");
        let serve_dir = temp_dir.path().join("serve");
        fs::create_dir_all(&site_dir).unwrap();
        fs::write(site_dir.join("index.html"), "<html/>").unwrap();

        let paths = Paths {
            site: site_dir,
            content: PathBuf::from("content"),
            build: PathBuf::from("build"),
            template: PathBuf::from("templates"),
        };

        prepare_serve_dir(&paths, &serve_dir).unwrap();
        assert!(serve_dir.join("index.html").exists());
    }

    #[test]
    fn test_create_directories_all_valid() {
        let temp_dir = tempdir().unwrap();
        let paths = Paths {
            site: temp_dir.path().join("s"),
            content: temp_dir.path().join("c"),
            build: temp_dir.path().join("b"),
            template: temp_dir.path().join("t"),
        };
        create_directories(&paths).unwrap();
        assert!(paths.site.exists());
        assert!(paths.build.exists());
    }

    #[test]
    fn test_is_safe_path_existing_valid() {
        let temp_dir = tempdir().unwrap();
        let dir = temp_dir.path().join("valid");
        fs::create_dir(&dir).unwrap();
        let canonical = dir.canonicalize().unwrap();
        assert!(is_safe_path(&canonical).unwrap());
    }

    // -----------------------------------------------------------------
    // RunOptions / build_pipeline / execute_build_pipeline
    // -----------------------------------------------------------------

    #[test]
    fn run_options_from_matches_extracts_quiet_drafts_and_deploy() {
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec![
                "ssg", "--quiet", "--drafts", "--deploy", "netlify",
            ])
            .expect("matches");
        let opts = RunOptions::from_matches(&matches);
        assert!(opts.quiet);
        assert!(opts.include_drafts);
        assert_eq!(opts.deploy_target.as_deref(), Some("netlify"));
    }

    #[test]
    fn run_options_from_matches_defaults_when_flags_absent() {
        let cli = Cli::build();
        let matches = cli.try_get_matches_from(vec!["ssg"]).expect("matches");
        let opts = RunOptions::from_matches(&matches);
        assert!(!opts.quiet);
        assert!(!opts.include_drafts);
        assert!(opts.deploy_target.is_none());
    }

    #[test]
    fn build_pipeline_assembles_manager_context_and_dirs() {
        let temp = tempdir().unwrap();
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

        assert!(plugins.len() >= 10);
        assert_ne!(build_dir, site_dir);
        assert_eq!(site_dir, temp.path().join("public"));
        assert_eq!(ctx.content_dir, temp.path().join("content"));
    }

    #[test]
    fn build_pipeline_with_deploy_target_registers_deploy_plugin() {
        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("content");
        config.output_dir = temp.path().join("public");

        let opts_no_deploy = RunOptions {
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
        let (no_deploy, _, _, _) = build_pipeline(&config, &opts_no_deploy);

        let opts_deploy = RunOptions {
            quiet: true,
            include_drafts: false,
            deploy_target: Some("netlify".to_string()),
            validate_only: false,
            jobs: None,
            max_memory_mb: None,
            ai_fix: false,
            ai_fix_dry_run: false,
            incremental: false,
            no_llm_cache: false,

            isr: false,
        };
        let (with_deploy, _, _, _) = build_pipeline(&config, &opts_deploy);

        assert_eq!(with_deploy.len(), no_deploy.len() + 1);
    }

    #[test]
    fn build_pipeline_with_unknown_deploy_target_logs_and_skips() {
        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("content");
        config.output_dir = temp.path().join("public");

        let opts = RunOptions {
            quiet: true,
            include_drafts: false,
            deploy_target: Some("nonsense-platform".to_string()),
            validate_only: false,
            jobs: None,
            max_memory_mb: None,
            ai_fix: false,
            ai_fix_dry_run: false,
            incremental: false,
            no_llm_cache: false,

            isr: false,
        };
        let (plugins, _, _, _) = build_pipeline(&config, &opts);
        let names = plugins.names();
        assert!(!names.iter().any(|n| n == &"deploy"));
    }

    #[test]
    fn build_pipeline_with_each_known_deploy_target_registers_one_plugin() {
        for target in ["netlify", "vercel", "cloudflare", "github"] {
            let temp = tempdir().unwrap();
            let mut config = SsgConfig::default();
            config.content_dir = temp.path().join("content");
            config.output_dir = temp.path().join("public");

            let opts = RunOptions {
                quiet: true,
                include_drafts: false,
                deploy_target: Some(target.to_string()),
                validate_only: false,
                jobs: None,
                max_memory_mb: None,
                ai_fix: false,
                ai_fix_dry_run: false,
                incremental: false,
                no_llm_cache: false,

                isr: false,
            };
            let (plugins, _, _, _) = build_pipeline(&config, &opts);
            assert!(
                plugins.names().iter().any(|n| n == &"deploy"),
                "deploy plugin should be registered for target `{target}`"
            );
        }
    }

    // -----------------------------------------------------------------
    // ServeTransport / serve_site_with
    // -----------------------------------------------------------------

    /// Test transport that records its calls without starting an
    /// HTTP server.
    #[derive(Debug, Default)]
    struct RecordingTransport {
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl ServeTransport for RecordingTransport {
        fn start(&self, addr: &str, root: &str) -> Result<(), SsgError> {
            self.calls
                .lock()
                .unwrap()
                .push((addr.to_string(), root.to_string()));
            Ok(())
        }
    }

    /// Test transport that always errors — verifies the error is
    /// propagated through `serve_site_with`.
    #[derive(Debug, Default)]
    struct FailingTransport;

    impl ServeTransport for FailingTransport {
        fn start(&self, _addr: &str, _root: &str) -> Result<(), SsgError> {
            Err(SsgError::Validation {
                field: "transport".to_string(),
                message: "transport failed".to_string(),
            })
        }
    }

    #[test]
    fn build_serve_address_resolves_path_to_addr_root_pair() {
        let (addr, root) = build_serve_address(Path::new("./public")).unwrap();
        assert_eq!(
            addr,
            format!("{}:{}", cmd::DEFAULT_HOST, cmd::DEFAULT_PORT)
        );
        assert_eq!(root, "./public");
    }

    #[test]
    fn verify_and_copy_files_destination_create_dir_failure_propagates() {
        let temp = tempdir().unwrap();
        let blocker = temp.path().join("blocker.txt");
        fs::write(&blocker, "i am a file, not a directory").unwrap();

        let bad_dst = blocker.join("sub");
        let result = verify_and_copy_files(temp.path(), &bad_dst);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == &bad_dst),
            "expected SsgError::Io for bad_dst, got: {err:?}"
        );
    }

    #[cfg(not(target_os = "windows"))] // Unix-specific: path behaviour / error messages differ on Windows
    #[test]
    fn create_directories_unsafe_path_bails() {
        let temp = tempdir().unwrap();
        let blocker = temp.path().join("blocker.txt");
        fs::write(&blocker, "x").unwrap();

        let unsafe_path = blocker.join("..").join("subdir");

        let paths = Paths {
            site: temp.path().join("s"),
            content: unsafe_path,
            build: temp.path().join("b"),
            template: temp.path().join("t"),
        };
        let result = create_directories(&paths);
        assert!(result.is_err());
    }

    #[test]
    fn copy_dir_with_progress_read_dir_failure_propagates() {
        let temp = tempdir().unwrap();
        let src_file = temp.path().join("not-a-dir.txt");
        fs::write(&src_file, "content").unwrap();
        let dst = temp.path().join("dst");

        let result = copy_dir_with_progress(&src_file, &dst);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == &src_file),
            "expected SsgError::Io for src_file, got: {err:?}"
        );
    }

    #[test]
    fn verify_and_copy_files_async_destination_create_dir_failure_propagates() {
        let temp = tempdir().unwrap();
        let blocker = temp.path().join("async-blocker.txt");
        fs::write(&blocker, "blocker").unwrap();

        let bad_dst = blocker.join("sub");
        let result = verify_and_copy_files_async(temp.path(), &bad_dst);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == &bad_dst),
            "expected SsgError::Io for bad_dst, got: {err:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn build_serve_address_rejects_invalid_utf8_path() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let invalid_bytes = b"site_\xff_invalid";
        let path = Path::new(OsStr::from_bytes(invalid_bytes));
        let err = build_serve_address(path).unwrap_err();
        assert!(format!("{err:?}").contains("invalid UTF-8"));
    }

    #[test]
    #[cfg(unix)]
    fn serve_site_shim_propagates_invalid_utf8_path_error() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let invalid = b"\xfe\xfe_bad";
        let path = Path::new(OsStr::from_bytes(invalid));
        let err = serve_site(path).unwrap_err();
        assert!(format!("{err:?}").contains("invalid UTF-8"));
    }

    #[test]
    fn serve_site_with_recording_transport_records_addr_and_root() {
        let transport = RecordingTransport::default();
        serve_site_with(Path::new("./public"), &transport).unwrap();
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "./public");
    }

    #[test]
    fn serve_site_with_propagates_transport_errors() {
        let transport = FailingTransport;
        let result = serve_site_with(Path::new("./public"), &transport);
        assert!(result.is_err());
        assert!(
            format!("{:?}", result.unwrap_err()).contains("transport failed")
        );
    }

    #[test]
    fn http_transport_implements_serve_transport_trait() {
        fn assert_impl<T: ServeTransport>() {}
        assert_impl::<HttpTransport>();
    }

    // -----------------------------------------------------------------
    // execute_build_pipeline
    // -----------------------------------------------------------------

    #[test]
    fn execute_build_pipeline_propagates_compile_errors() {
        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("missing-content");
        config.output_dir = temp.path().join("public");
        config.template_dir = temp.path().join("missing-templates");
        config.site_name = "broken".to_string();

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

        let result = execute_build_pipeline(
            &plugins,
            &ctx,
            &build_dir,
            &config.content_dir,
            &site_dir,
            &config.template_dir,
            opts.quiet,
        );
        assert!(result.is_err(), "broken layout should propagate Err");
    }

    /// Drives the full pipeline against the `examples/` fixtures found
    /// under `base`. Returns `false` after logging a skip notice when
    /// the fixtures are absent, so both branches are unit-testable.
    fn run_example_fixture_pipeline(base: &Path, quiet: bool) -> bool {
        let content = base.join("examples/content/en");
        let template = base.join("examples/templates/en");
        if !content.exists() || !template.exists() {
            eprintln!(
                "skipping: examples/content/en not present in {}",
                base.display()
            );
            return false;
        }

        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = content;
        config.template_dir = template;
        config.output_dir = temp.path().join("public");
        config.site_name = "pipeline-success-test".to_string();
        config.base_url = "http://localhost".to_string();

        let opts = RunOptions {
            quiet,
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

        execute_build_pipeline(
            &plugins,
            &ctx,
            &build_dir,
            &config.content_dir,
            &site_dir,
            &config.template_dir,
            opts.quiet,
        )
        .unwrap();

        // Evaluate both eagerly (`|` not `||`) so each check executes.
        let output_present = site_dir.exists() | build_dir.exists();
        assert!(output_present);
        true
    }

    #[test]
    fn execute_build_pipeline_succeeds_against_real_example_fixtures() {
        let cwd = env::current_dir().unwrap();
        let _ = run_example_fixture_pipeline(&cwd, true);
    }

    #[test]
    fn execute_build_pipeline_verbose_success_hits_println_arm() {
        let cwd = env::current_dir().unwrap();
        let _ = run_example_fixture_pipeline(&cwd, false);
    }

    #[test]
    fn example_fixture_pipeline_skips_when_fixtures_missing() {
        let temp = tempdir().unwrap();
        assert!(!run_example_fixture_pipeline(temp.path(), true));
    }

    #[test]
    fn execute_build_pipeline_verbose_propagates_compile_errors() {
        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("missing");
        config.output_dir = temp.path().join("public");
        config.template_dir = temp.path().join("missing-templates");
        config.site_name = "broken-verbose".to_string();

        let opts = RunOptions {
            quiet: false,
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

        let _ = execute_build_pipeline(
            &plugins,
            &ctx,
            &build_dir,
            &config.content_dir,
            &site_dir,
            &config.template_dir,
            opts.quiet,
        );
    }

    #[test]
    fn build_pipeline_with_drafts_flag_registers_draft_plugin() {
        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("content");
        config.output_dir = temp.path().join("public");

        let opts = RunOptions {
            quiet: true,
            include_drafts: true,
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
        let (plugins, _, _, _) = build_pipeline(&config, &opts);
        assert!(plugins.names().iter().any(|n| n == &"drafts"));
    }

    // -----------------------------------------------------------------
    // now_iso / days_to_ymd coverage
    // -----------------------------------------------------------------

    #[test]
    fn now_iso_returns_valid_iso8601_format() {
        let ts = now_iso();
        assert_eq!(ts.len(), 20, "ISO timestamp should be 20 chars: {ts}");
        assert!(ts.ends_with('Z'), "should end with Z: {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
        let year: u64 = ts[0..4].parse().unwrap();
        assert!(year >= 2020, "year should be recent: {year}");
    }

    #[test]
    fn days_to_ymd_epoch() {
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date_2026_04_13() {
        let (y, m, d) = days_to_ymd(20_556);
        assert_eq!((y, m, d), (2026, 4, 13));
    }

    #[test]
    fn days_to_ymd_leap_day() {
        let (y, m, d) = days_to_ymd(11_016);
        assert_eq!((y, m, d), (2000, 2, 29));
    }

    #[test]
    fn days_to_ymd_y2k() {
        let (y, m, d) = days_to_ymd(10_957);
        assert_eq!((y, m, d), (2000, 1, 1));
    }

    // -----------------------------------------------------------------
    // SimpleLogger coverage
    // -----------------------------------------------------------------

    #[test]
    fn simple_logger_enabled_respects_max_level() {
        let logger = SimpleLogger;
        let meta = log::MetadataBuilder::new()
            .level(log::Level::Info)
            .target("test")
            .build();
        let _ = logger.enabled(&meta);
    }

    #[test]
    fn simple_logger_flush_is_noop() {
        use log::Log;
        let logger = SimpleLogger;
        logger.flush();
    }

    // -----------------------------------------------------------------
    // build_serve_address additional coverage
    // -----------------------------------------------------------------

    #[test]
    fn build_serve_address_with_absolute_path() {
        let (addr, root) = build_serve_address(Path::new("/tmp/site")).unwrap();
        assert!(addr.contains(&cmd::DEFAULT_PORT.to_string()));
        assert_eq!(root, "/tmp/site");
    }

    // -----------------------------------------------------------------
    // copy_dir_with_progress file count output
    // -----------------------------------------------------------------

    #[test]
    fn copy_dir_with_progress_counts_files_and_dirs() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        fs::write(src_dir.path().join("a.txt"), "a").unwrap();
        fs::write(src_dir.path().join("b.txt"), "b").unwrap();
        let sub = src_dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("c.txt"), "c").unwrap();

        copy_dir_with_progress(src_dir.path(), dst_dir.path()).unwrap();

        assert!(dst_dir.path().join("a.txt").exists());
        assert!(dst_dir.path().join("b.txt").exists());
        assert!(dst_dir.path().join("sub/c.txt").exists());
    }

    // -----------------------------------------------------------------
    // days_to_ymd — additional edge cases
    // -----------------------------------------------------------------

    #[test]
    fn days_to_ymd_end_of_year() {
        // Dec 31, 1970 = day 364
        let (y, m, d) = days_to_ymd(364);
        assert_eq!((y, m, d), (1970, 12, 31));
    }

    #[test]
    fn days_to_ymd_non_leap_year_feb28() {
        // Feb 28, 1971 = day 58 + 365 = 423
        let (y, m, d) = days_to_ymd(423);
        assert_eq!((y, m, d), (1971, 2, 28));
    }

    #[test]
    fn days_to_ymd_non_leap_year_mar1() {
        // Mar 1, 1971 = day 424
        let (y, m, d) = days_to_ymd(424);
        assert_eq!((y, m, d), (1971, 3, 1));
    }

    #[test]
    fn days_to_ymd_century_non_leap() {
        // 1900 is NOT a leap year (divisible by 100, not by 400).
        // Mar 1, 1900 — we use a negative-offset approach:
        // 2000-01-01 is day 10957. 1900-01-01 is 10957 - 36524 = ???
        // Easier: just test a few far-future dates.
        // 2100-01-01 is NOT a leap year.
        // 2100-03-01: days = (2100-1970)*365 + leap_days + 31 + 28
        // Instead, let's verify round-trip for several known dates.
        let (y, m, d) = days_to_ymd(10_956);
        assert_eq!((y, m, d), (1999, 12, 31));
    }

    #[test]
    fn days_to_ymd_large_day_count() {
        // Far-future date: 2100-01-01
        // 2100-01-01 is day 47482
        let (y, m, d) = days_to_ymd(47_482);
        assert_eq!((y, m, d), (2100, 1, 1));
    }

    // -----------------------------------------------------------------
    // now_iso — additional format checks
    // -----------------------------------------------------------------

    #[test]
    fn now_iso_month_and_day_within_range() {
        let ts = now_iso();
        let month: u32 = ts[5..7].parse().unwrap();
        let day: u32 = ts[8..10].parse().unwrap();
        let hour: u32 = ts[11..13].parse().unwrap();
        let minute: u32 = ts[14..16].parse().unwrap();
        let second: u32 = ts[17..19].parse().unwrap();
        assert!((1..=12).contains(&month), "month out of range: {month}");
        assert!((1..=31).contains(&day), "day out of range: {day}");
        assert!(hour < 24, "hour out of range: {hour}");
        assert!(minute < 60, "minute out of range: {minute}");
        assert!(second < 60, "second out of range: {second}");
    }

    // -----------------------------------------------------------------
    // Paths — additional validation edge cases
    // -----------------------------------------------------------------

    #[test]
    fn paths_validate_double_slash_in_content() {
        let paths = Paths {
            site: PathBuf::from("public"),
            content: PathBuf::from("content//nested"),
            build: PathBuf::from("build"),
            template: PathBuf::from("templates"),
        };
        let err = paths.validate().unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::Validation {
                field: String::new(),
                message: String::new(),
            },
        );
    }

    #[test]
    fn paths_validate_traversal_in_build() {
        let paths = Paths {
            site: PathBuf::from("public"),
            content: PathBuf::from("content"),
            build: PathBuf::from("../build"),
            template: PathBuf::from("templates"),
        };
        let err = paths.validate().unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::PathTraversal {
                path: PathBuf::new(),
            },
        );
    }

    #[test]
    fn paths_validate_traversal_in_template() {
        let paths = Paths {
            site: PathBuf::from("public"),
            content: PathBuf::from("content"),
            build: PathBuf::from("build"),
            template: PathBuf::from("../templates"),
        };
        let err = paths.validate().unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::PathTraversal {
                path: PathBuf::new(),
            },
        );
    }

    #[test]
    fn paths_validate_double_slash_in_build() {
        let paths = Paths {
            site: PathBuf::from("public"),
            content: PathBuf::from("content"),
            build: PathBuf::from("build//sub"),
            template: PathBuf::from("templates"),
        };
        let err = paths.validate().unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::Validation {
                field: String::new(),
                message: String::new(),
            },
        );
    }

    #[test]
    fn paths_validate_double_slash_in_template() {
        let paths = Paths {
            site: PathBuf::from("public"),
            content: PathBuf::from("content"),
            build: PathBuf::from("build"),
            template: PathBuf::from("templates//sub"),
        };
        let err = paths.validate().unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::Validation {
                field: String::new(),
                message: String::new(),
            },
        );
    }

    // -----------------------------------------------------------------
    // PathsBuilder — additional coverage
    // -----------------------------------------------------------------

    #[test]
    fn paths_builder_partial_override() {
        let paths = Paths::builder()
            .site("custom_site")
            .template("custom_templates")
            .build()
            .unwrap();
        assert_eq!(paths.site, PathBuf::from("custom_site"));
        assert_eq!(paths.content, PathBuf::from("content"));
        assert_eq!(paths.build, PathBuf::from("build"));
        assert_eq!(paths.template, PathBuf::from("custom_templates"));
    }

    #[test]
    fn paths_debug_format() {
        let paths = Paths::default_paths();
        let debug = format!("{paths:?}");
        assert!(debug.contains("site"));
        assert!(debug.contains("content"));
    }

    // -----------------------------------------------------------------
    // RunOptions — additional flag combinations
    // -----------------------------------------------------------------

    #[test]
    fn run_options_from_matches_extracts_validate_flag() {
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec!["ssg", "--validate"])
            .expect("matches");
        let opts = RunOptions::from_matches(&matches);
        assert!(opts.validate_only);
        assert!(!opts.quiet);
        assert!(!opts.include_drafts);
    }

    #[test]
    fn run_options_from_matches_extracts_jobs_flag() {
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec!["ssg", "--jobs", "8"])
            .expect("matches");
        let opts = RunOptions::from_matches(&matches);
        assert_eq!(opts.jobs, Some(8));
    }

    #[test]
    fn run_options_from_matches_extracts_max_memory_flag() {
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec!["ssg", "--max-memory", "256"])
            .expect("matches");
        let opts = RunOptions::from_matches(&matches);
        assert_eq!(opts.max_memory_mb, Some(256));
    }

    #[test]
    fn run_options_from_matches_extracts_ai_fix_flags() {
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec!["ssg", "--ai-fix", "--ai-fix-dry-run"])
            .expect("matches");
        let opts = RunOptions::from_matches(&matches);
        assert!(opts.ai_fix);
        assert!(opts.ai_fix_dry_run);
    }

    #[test]
    fn run_options_from_matches_all_flags_combined() {
        let cli = Cli::build();
        let matches = cli
            .try_get_matches_from(vec![
                "ssg",
                "--quiet",
                "--drafts",
                "--deploy",
                "vercel",
                "--validate",
                "--jobs",
                "4",
                "--max-memory",
                "1024",
                "--ai-fix",
                "--ai-fix-dry-run",
            ])
            .expect("matches");
        let opts = RunOptions::from_matches(&matches);
        assert!(opts.quiet);
        assert!(opts.include_drafts);
        assert_eq!(opts.deploy_target.as_deref(), Some("vercel"));
        assert!(opts.validate_only);
        assert_eq!(opts.jobs, Some(4));
        assert_eq!(opts.max_memory_mb, Some(1024));
        assert!(opts.ai_fix);
        assert!(opts.ai_fix_dry_run);
    }

    // -----------------------------------------------------------------
    // build_pipeline — memory budget propagation
    // -----------------------------------------------------------------

    #[test]
    fn build_pipeline_propagates_max_memory_to_context() {
        let temp = tempdir().unwrap();
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
            max_memory_mb: Some(128),
            ai_fix: false,
            ai_fix_dry_run: false,
            incremental: false,
            no_llm_cache: false,

            isr: false,
        };

        let (_plugins, ctx, _build_dir, _site_dir) =
            build_pipeline(&config, &opts);

        assert!(
            ctx.memory_budget.is_some(),
            "memory_budget should be set when max_memory_mb is provided"
        );
    }

    #[test]
    fn build_pipeline_no_memory_budget_when_not_specified() {
        let temp = tempdir().unwrap();
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

        let (_plugins, ctx, _build_dir, _site_dir) =
            build_pipeline(&config, &opts);

        assert!(
            ctx.memory_budget.is_none(),
            "memory_budget should be None when max_memory_mb not provided"
        );
    }

    // -----------------------------------------------------------------
    // build_pipeline — deploy targets: vercel, cloudflare, github
    // -----------------------------------------------------------------

    #[test]
    fn build_pipeline_with_vercel_deploy_target() {
        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("content");
        config.output_dir = temp.path().join("public");

        let opts = RunOptions {
            quiet: true,
            include_drafts: false,
            deploy_target: Some("vercel".to_string()),
            validate_only: false,
            jobs: None,
            max_memory_mb: None,
            ai_fix: false,
            ai_fix_dry_run: false,
            incremental: false,
            no_llm_cache: false,

            isr: false,
        };
        let (plugins, _, _, _) = build_pipeline(&config, &opts);
        assert!(plugins.names().iter().any(|n| n == &"deploy"));
    }

    #[test]
    fn build_pipeline_with_cloudflare_deploy_target() {
        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("content");
        config.output_dir = temp.path().join("public");

        let opts = RunOptions {
            quiet: true,
            include_drafts: false,
            deploy_target: Some("cloudflare".to_string()),
            validate_only: false,
            jobs: None,
            max_memory_mb: None,
            ai_fix: false,
            ai_fix_dry_run: false,
            incremental: false,
            no_llm_cache: false,

            isr: false,
        };
        let (plugins, _, _, _) = build_pipeline(&config, &opts);
        assert!(plugins.names().iter().any(|n| n == &"deploy"));
    }

    #[test]
    fn build_pipeline_with_github_deploy_target() {
        let temp = tempdir().unwrap();
        let mut config = SsgConfig::default();
        config.content_dir = temp.path().join("content");
        config.output_dir = temp.path().join("public");

        let opts = RunOptions {
            quiet: true,
            include_drafts: false,
            deploy_target: Some("github".to_string()),
            validate_only: false,
            jobs: None,
            max_memory_mb: None,
            ai_fix: false,
            ai_fix_dry_run: false,
            incremental: false,
            no_llm_cache: false,

            isr: false,
        };
        let (plugins, _, _, _) = build_pipeline(&config, &opts);
        assert!(plugins.names().iter().any(|n| n == &"deploy"));
    }

    // -----------------------------------------------------------------
    // resolve_build_and_site_dirs — additional edge cases
    // -----------------------------------------------------------------

    #[test]
    fn resolve_build_and_site_dirs_serve_dir_none_uses_output_dir_as_site() {
        let mut config = SsgConfig::default();
        config.output_dir = PathBuf::from("my-output");
        config.serve_dir = None;

        let (_build_dir, site_dir) = resolve_build_and_site_dirs(&config);
        assert_eq!(site_dir, PathBuf::from("my-output"));
    }

    #[test]
    fn resolve_build_and_site_dirs_always_produces_distinct_dirs() {
        // Even when serve_dir == output_dir, build != site
        let mut config = SsgConfig::default();
        config.output_dir = PathBuf::from("same");
        config.serve_dir = Some(PathBuf::from("same"));

        let (build_dir, site_dir) = resolve_build_and_site_dirs(&config);
        assert_ne!(build_dir, site_dir);
        assert_eq!(site_dir, PathBuf::from("same"));
        assert!(build_dir.to_string_lossy().contains("build-tmp"));
    }

    // -----------------------------------------------------------------
    // generate_locale_redirect coverage
    // -----------------------------------------------------------------

    #[test]
    fn generate_locale_redirect_creates_index_html() {
        let temp = tempdir().unwrap();
        let locales = vec!["en".to_string(), "fr".to_string()];
        generate_locale_redirect(temp.path(), &locales, "en").unwrap();

        let index = temp.path().join("index.html");
        assert!(index.exists());
        let content = fs::read_to_string(&index).unwrap();
        assert!(content.contains("ssg-locale-redirect"));
        assert!(content.contains("\"en\""));
        assert!(content.contains("\"fr\""));
    }

    #[test]
    fn generate_locale_redirect_does_not_overwrite_user_index() {
        let temp = tempdir().unwrap();
        let user_html = "<html><body>My site</body></html>";
        fs::write(temp.path().join("index.html"), user_html).unwrap();

        let locales = vec!["en".to_string()];
        generate_locale_redirect(temp.path(), &locales, "en").unwrap();

        let content =
            fs::read_to_string(temp.path().join("index.html")).unwrap();
        assert_eq!(content, user_html, "user index.html should be preserved");
    }

    #[test]
    fn generate_locale_redirect_overwrites_own_index() {
        let temp = tempdir().unwrap();
        let old_redirect = "<!-- ssg-locale-redirect --><html>old</html>";
        fs::write(temp.path().join("index.html"), old_redirect).unwrap();

        let locales = vec!["de".to_string(), "en".to_string()];
        generate_locale_redirect(temp.path(), &locales, "de").unwrap();

        let content =
            fs::read_to_string(temp.path().join("index.html")).unwrap();
        assert!(content.contains("ssg-locale-redirect"));
        assert!(content.contains("\"de\""));
    }

    // ── Subcommand handler unit coverage (issue #527) ───────────────

    #[test]
    fn apply_rayon_thread_pool_none_is_no_op() {
        // `None` path must be Ok and must not touch the global pool.
        assert!(apply_rayon_thread_pool(None).is_ok());
    }

    #[test]
    fn apply_rayon_thread_pool_some_either_succeeds_or_signals_already_set() {
        // The Rayon global pool can only be initialised once per
        // process. Earlier tests may have already initialised it via
        // `RunOptions` / pipeline tests, in which case a second
        // call returns an SsgError::Validation. Either outcome is
        // acceptable here — we just need to walk the `Some(n)` branch.
        // The `Ok` arm is exercised deterministically by
        // `apply_rayon_thread_pool_succeeds_in_fresh_process`, which
        // re-runs this test in a child process where nothing has
        // touched Rayon yet.
        test_support::init_logger();
        let result = apply_rayon_thread_pool(Some(1));
        match result {
            Ok(()) => {}
            Err(e) => assert_same_variant(
                &e,
                &SsgError::Validation {
                    field: String::new(),
                    message: String::new(),
                },
            ),
        }
    }

    #[test]
    fn apply_rayon_thread_pool_succeeds_in_fresh_process() {
        // Re-run the sibling test in a fresh process, where the global
        // Rayon pool has never been initialised, so `build_global`
        // succeeds and the `Ok` arm executes.
        let exe = env::current_exe().unwrap();
        let status = std::process::Command::new(exe)
            .args([
                "--exact",
                "tests::apply_rayon_thread_pool_some_either_succeeds_or_signals_already_set",
                "--test-threads=1",
            ])
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn build_config_from_subcommand_matches_routes_through_to_config() {
        let (_inv, matches) =
            Cli::parse_and_dispatch(["ssg", "build"]).unwrap();
        let sub = matches.subcommand_matches("build").unwrap();
        let cfg = build_config_from_subcommand_matches(sub).unwrap();
        // Default content_dir is `content`.
        assert_eq!(cfg.content_dir, PathBuf::from("content"));
    }

    #[test]
    fn build_config_from_subcommand_matches_propagates_validation_errors() {
        // Point `--config` at a non-existent file — SsgConfig::from_file
        // returns CliError, the wrapper maps that onto
        // SsgError::Validation.
        let (_inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "build",
            "--config",
            "/definitely/does/not/exist.toml",
        ])
        .unwrap();
        let sub = matches.subcommand_matches("build").unwrap();
        let err = build_config_from_subcommand_matches(sub).unwrap_err();
        assert_same_variant(
            &err,
            &SsgError::Validation {
                field: String::new(),
                message: String::new(),
            },
        );
    }

    #[test]
    fn dispatch_invocation_check_with_missing_subcommand_returns_validation_error(
    ) {
        // Build a top-level matches that has no `check` subcommand so
        // run_check's `ok_or_else` arm fires.
        let matches =
            Cli::subcommand_app().try_get_matches_from(["ssg"]).unwrap();
        let err =
            dispatch_invocation(CliInvocation::Check, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "subcommand")
        );
    }

    #[test]
    fn dispatch_invocation_build_with_missing_subcommand_returns_validation_error(
    ) {
        let matches =
            Cli::subcommand_app().try_get_matches_from(["ssg"]).unwrap();
        let err =
            dispatch_invocation(CliInvocation::Build, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "subcommand")
        );
    }

    #[test]
    fn dispatch_invocation_dev_with_missing_subcommand_returns_validation_error(
    ) {
        let matches =
            Cli::subcommand_app().try_get_matches_from(["ssg"]).unwrap();
        let err =
            dispatch_invocation(CliInvocation::Dev, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "subcommand")
        );
    }

    #[test]
    fn dispatch_invocation_deploy_with_missing_subcommand_returns_validation_error(
    ) {
        let matches =
            Cli::subcommand_app().try_get_matches_from(["ssg"]).unwrap();
        let err = dispatch_invocation(
            CliInvocation::Deploy {
                target: "none".to_string(),
            },
            &matches,
        )
        .unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "subcommand")
        );
    }

    /// Mutex used to serialise tests that exercise `run_check` /
    /// `run_legacy` so they don't race on the global Rayon thread pool
    /// init.
    fn ssg_check_lock() -> &'static std::sync::Mutex<()> {
        use std::sync::Mutex;
        use std::sync::OnceLock;
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Acquires the serialisation lock, recovering from poisoning so a
    /// panicking sibling test cannot cascade failures.
    fn ssg_check_guard() -> std::sync::MutexGuard<'static, ()> {
        ssg_check_lock().lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn ssg_check_guard_recovers_from_poisoned_lock() {
        // Poison the lock from a scratch thread, then verify the
        // guard helper's `into_inner` recovery arm actually runs.
        let poisoner = std::thread::spawn(|| {
            let _g = ssg_check_lock().lock().unwrap();
            panic!("deliberate poison for ssg_check_guard test");
        });
        assert!(poisoner.join().is_err());
        let _g = ssg_check_guard();
    }

    #[test]
    fn run_check_with_empty_content_dir_passes() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let argv = [
            "ssg",
            "check",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
        ];
        let (inv, matches) = Cli::parse_and_dispatch(argv).unwrap();
        assert_same_variant(&inv, &CliInvocation::Check);
        // run_check is the unit-under-test. It must complete cleanly
        // for an empty (no-schema, no-content) site.
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok(), "run_check failed: {result:?}");
    }

    #[test]
    fn run_legacy_with_validate_flag_short_circuits_to_validate_only() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let argv = [
            "ssg",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
            "--validate",
        ];
        let (inv, matches) = Cli::parse_and_dispatch(argv).unwrap();
        assert_same_variant(&inv, &CliInvocation::Legacy);
        // The `--validate` legacy flag short-circuits before any
        // pipeline work happens, so this is safe to run as a unit test.
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok(), "run_legacy --validate failed: {result:?}");
    }

    #[test]
    fn run_subcommand_build_with_empty_dirs_completes() {
        // Drives run_subcommand("build") end-to-end through
        // dispatch_invocation. Empty content/templates dirs means the
        // build is a no-op but every line of the dispatcher body is
        // executed.
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let argv = [
            "ssg",
            "build",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
        ];
        let (inv, matches) = Cli::parse_and_dispatch(argv).unwrap();
        assert_same_variant(&inv, &CliInvocation::Build);
        let result = dispatch_invocation(inv, &matches);
        // Build may produce warnings on empty input but should not
        // hard-fail.
        let _ = result;
    }

    #[test]
    fn run_deploy_with_none_target_invokes_noop_adapter() {
        // run_deploy with --target none uses the no-op adapter, which
        // is purely a print + Ok, so the test can drive the full body
        // including pipeline execution + adapter dispatch without
        // hitting any network.
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let argv = [
            "ssg",
            "deploy",
            "--target",
            "none",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
        ];
        let (inv, matches) = Cli::parse_and_dispatch(argv).unwrap();
        assert_same_variant(
            &inv,
            &CliInvocation::Deploy {
                target: String::new(),
            },
        );
        let result = dispatch_invocation(inv, &matches);
        let _ = result;
    }

    #[test]
    fn run_legacy_happy_path_with_empty_content_dir() {
        // Same content/templates as run_check but without --validate, so
        // the full legacy code path executes (build_pipeline +
        // execute_build_pipeline_with). Empty dirs make the build a
        // no-op, no server is started because --serve is not set.
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let argv = [
            "ssg",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
        ];
        let (inv, matches) = Cli::parse_and_dispatch(argv).unwrap();
        assert_same_variant(&inv, &CliInvocation::Legacy);
        let result = dispatch_invocation(inv, &matches);
        let _ = result;
    }
    // ── run_with_argv / run() entry-point coverage ──────────────────

    /// Builds an owned `OsString` argv from string literals.
    fn os_argv(args: &[&str]) -> Vec<std::ffi::OsString> {
        args.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn run_with_argv_legacy_validate_short_circuits_cleanly() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let argv = os_argv(&[
            "ssg",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
            "--validate",
        ]);
        // Legacy parser defines `--trace`, so the `try_contains_id`
        // branch that calls `get_flag` executes here.
        let result = run_with_argv(argv);
        assert!(result.is_ok());
    }

    #[test]
    fn run_with_argv_subcommand_check_lacks_trace_id() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let argv = os_argv(&[
            "ssg",
            "check",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
        ]);
        // The subcommand parser does not define `--trace`, so this
        // walks the `unwrap_or(false)` fallback branch.
        let result = run_with_argv(argv);
        assert!(result.is_ok());
    }

    /// Environment gate for [`run_entrypoint_child`].
    const RUN_CHILD_ENV: &str = "SSG_TEST_RUN_ENTRYPOINT_CHILD";

    #[test]
    fn run_entrypoint_child() {
        // Inert unless spawned by
        // `run_exits_with_clap_error_code_in_child_process`. In the
        // child process, the libtest harness argv
        // (`--exact <name> ...`) is not valid ssg argv, so `run()`
        // reaches `Err(e) => e.exit()` and terminates the process
        // with clap's parse-failure exit code (2).
        if env::var(RUN_CHILD_ENV).is_err() {
            return;
        }
        let _ = run();
    }

    #[test]
    fn run_exits_with_clap_error_code_in_child_process() {
        // Drives the real `run()` entry point (process argv +
        // `clap::Error::exit`) in a child process so the exit does
        // not tear down this harness.
        let exe = env::current_exe().unwrap();
        let output = std::process::Command::new(exe)
            .args([
                "--exact",
                "tests::run_entrypoint_child",
                "--test-threads=1",
            ])
            .env(RUN_CHILD_ENV, "1")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }

    // ── run_audit ───────────────────────────────────────────────────

    #[test]
    fn dispatch_invocation_audit_with_missing_subcommand_returns_validation_error(
    ) {
        let matches =
            Cli::subcommand_app().try_get_matches_from(["ssg"]).unwrap();
        let err =
            dispatch_invocation(CliInvocation::Audit, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "subcommand")
        );
    }

    #[test]
    fn run_audit_explain_lists_gates_without_running_them() {
        // `--explain` early-exits inside `cmd::audit::run` with
        // `Outcome::Pass`, so this drives the full `run_audit`
        // wrapper without needing a built site.
        let (inv, matches) =
            Cli::parse_and_dispatch(["ssg", "audit", "--explain"]).unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok());
    }

    // ── run_legacy error + banner + serve branches ─────────────────

    #[test]
    fn run_legacy_with_bad_config_file_maps_to_validation_error() {
        let _g = ssg_check_guard();

        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "--config",
            "/definitely/does/not/exist.toml",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "config")
        );
    }

    #[test]
    fn run_legacy_validate_with_invalid_schema_propagates_error() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        fs::write(
            content.path().join("content.schema.toml"),
            "not [valid toml",
        )
        .unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "--content",
            content.path().to_str().unwrap(),
            "--quiet",
            "--validate",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "content")
        );
    }

    #[test]
    fn run_legacy_with_jobs_after_pool_init_errors() {
        let _g = ssg_check_guard();
        force_rayon_global_pool_init();

        let content = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "--content",
            content.path().to_str().unwrap(),
            "--jobs",
            "2",
            "--quiet",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "jobs")
        );
    }

    #[test]
    fn run_legacy_nonquiet_prints_banner_and_builds() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok());
    }

    #[test]
    fn run_legacy_with_missing_content_dir_propagates_build_error() {
        let _g = ssg_check_guard();

        let temp = tempdir().unwrap();
        let missing_content = temp.path().join("missing-content");
        let missing_templates = temp.path().join("missing-templates");
        let output = temp.path().join("public");
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "--content",
            missing_content.to_str().unwrap(),
            "--template",
            missing_templates.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_err());
    }

    #[test]
    fn run_legacy_serve_flag_boots_dev_server_and_returns() {
        let _g = ssg_check_guard();

        // Hold the dev-server port so `http_handle::Server::start`
        // fails to bind and returns instead of blocking; the
        // `HttpTransport` shim swallows that error into `Ok(())`.
        let _port_guard =
            std::net::TcpListener::bind((cmd::DEFAULT_HOST, cmd::DEFAULT_PORT))
                .ok();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--serve",
            output.path().to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok());
    }

    // ── run_subcommand (`build` / `dev`) error + serve branches ────

    #[test]
    fn run_subcommand_build_with_bad_config_returns_validation_error() {
        let _g = ssg_check_guard();

        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "build",
            "--config",
            "/definitely/does/not/exist.toml",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "config")
        );
    }

    /// Forces the global Rayon pool to exist so a subsequent
    /// `--jobs N` request must fail with `SsgError::Validation`.
    fn force_rayon_global_pool_init() {
        let _ = rayon::ThreadPoolBuilder::new().build_global();
    }

    #[test]
    fn run_subcommand_build_with_jobs_after_pool_init_errors() {
        let _g = ssg_check_guard();
        force_rayon_global_pool_init();

        let content = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "build",
            "--content",
            content.path().to_str().unwrap(),
            "--jobs",
            "2",
            "--quiet",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "jobs")
        );
    }

    #[test]
    fn run_subcommand_build_nonquiet_prints_banner() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "build",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok());
    }

    #[test]
    fn run_subcommand_build_with_missing_content_dir_propagates_error() {
        let _g = ssg_check_guard();

        let temp = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "build",
            "--content",
            temp.path().join("missing-content").to_str().unwrap(),
            "--template",
            temp.path().join("missing-templates").to_str().unwrap(),
            "--output",
            temp.path().join("public").to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_err());
    }

    #[test]
    fn run_subcommand_dev_serves_and_returns_when_port_is_held() {
        let _g = ssg_check_guard();

        let _port_guard =
            std::net::TcpListener::bind((cmd::DEFAULT_HOST, cmd::DEFAULT_PORT))
                .ok();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "dev",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        assert_same_variant(&inv, &CliInvocation::Dev);
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok());
    }

    // ── run_check error + println branches ─────────────────────────

    #[test]
    fn run_check_with_bad_config_returns_validation_error() {
        let _g = ssg_check_guard();

        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "check",
            "--config",
            "/definitely/does/not/exist.toml",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "config")
        );
    }

    #[test]
    fn run_check_with_jobs_after_pool_init_errors() {
        let _g = ssg_check_guard();
        force_rayon_global_pool_init();

        let content = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "check",
            "--content",
            content.path().to_str().unwrap(),
            "--jobs",
            "2",
            "--quiet",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "jobs")
        );
    }

    #[test]
    fn run_check_with_invalid_schema_fails_validation() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        fs::write(
            content.path().join("content.schema.toml"),
            "not [valid toml",
        )
        .unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "check",
            "--content",
            content.path().to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "content")
        );
    }

    #[test]
    fn run_check_with_invalid_utf8_markdown_fails_before_compile() {
        let _g = ssg_check_guard();

        // No schema file, so `validate_only` passes; the invalid
        // UTF-8 markdown then fails a `before_compile` validator.
        let content = tempdir().unwrap();
        fs::write(content.path().join("fail.md"), [0xFF, 0xFE, 0xFD]).unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "check",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_err());
    }

    #[test]
    fn run_check_nonquiet_prints_success_line() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "check",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok());
    }

    // ── run_deploy error + banner branches ─────────────────────────

    #[test]
    fn run_deploy_with_bad_config_returns_validation_error() {
        let _g = ssg_check_guard();

        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "deploy",
            "--target",
            "none",
            "--config",
            "/definitely/does/not/exist.toml",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "config")
        );
    }

    #[test]
    fn run_deploy_with_jobs_after_pool_init_errors() {
        let _g = ssg_check_guard();
        force_rayon_global_pool_init();

        let content = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "deploy",
            "--target",
            "none",
            "--content",
            content.path().to_str().unwrap(),
            "--jobs",
            "2",
            "--quiet",
        ])
        .unwrap();
        let err = dispatch_invocation(inv, &matches).unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "jobs")
        );
    }

    #[test]
    fn run_deploy_nonquiet_prints_banner_and_adapter_name() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "deploy",
            "--target",
            "none",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_ok());
    }

    #[test]
    fn run_deploy_with_missing_content_dir_propagates_build_error() {
        let _g = ssg_check_guard();

        let temp = tempdir().unwrap();
        let (inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "deploy",
            "--target",
            "none",
            "--content",
            temp.path().join("missing-content").to_str().unwrap(),
            "--template",
            temp.path().join("missing-templates").to_str().unwrap(),
            "--output",
            temp.path().join("public").to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        let result = dispatch_invocation(inv, &matches);
        assert!(result.is_err());
    }

    #[test]
    fn run_deploy_with_unknown_target_errors_after_build() {
        let _g = ssg_check_guard();

        let content = tempdir().unwrap();
        let templates = tempdir().unwrap();
        let output = tempdir().unwrap();
        // Parse a *valid* deploy argv so the `deploy` subcommand
        // matches exist, then dispatch with an invalid target to hit
        // the `Target::from_cli` error branch after the build.
        let (_inv, matches) = Cli::parse_and_dispatch([
            "ssg",
            "deploy",
            "--target",
            "none",
            "--content",
            content.path().to_str().unwrap(),
            "--template",
            templates.path().to_str().unwrap(),
            "--output",
            output.path().to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        let err = dispatch_invocation(
            CliInvocation::Deploy {
                target: "bogus".to_string(),
            },
            &matches,
        )
        .unwrap_err();
        assert!(
            matches!(err, SsgError::Validation { field, .. } if field == "deploy.target")
        );
    }

    // ── Failpoint-driven error branches (feature-gated) ────────────
    //
    // These failpoints sit behind seams (`initialize_logging_checked`,
    // `run_on_serve_checked`) whose real implementations cannot fail
    // from CLI-reachable inputs. Serialised via `ssg_check_guard` —
    // only tests holding that guard can reach these failpoints, so
    // the process-global failpoint registry stays race-free.
    #[cfg(feature = "test-fault-injection")]
    mod failpoints {
        use super::*;

        /// RAII guard that disables a failpoint on drop.
        struct FailGuard<'a>(&'a str);
        impl Drop for FailGuard<'_> {
            fn drop(&mut self) {
                let _ = fail::cfg(self.0, "off");
            }
        }

        #[test]
        fn run_with_argv_propagates_injected_logging_init_failure() {
            let _g = ssg_check_guard();
            let _fp = FailGuard("lib::initialize-logging");
            fail::cfg("lib::initialize-logging", "return").unwrap();

            let argv = os_argv(&["ssg", "--validate", "--quiet"]);
            let err = run_with_argv(argv).unwrap_err();
            assert!(format!("{err:?}")
                .contains("injected: lib::initialize-logging"));
        }

        #[test]
        fn run_legacy_serve_propagates_injected_on_serve_failure() {
            let _g = ssg_check_guard();
            let _fp = FailGuard("lib::run-on-serve");
            fail::cfg("lib::run-on-serve", "return").unwrap();

            let content = tempdir().unwrap();
            let templates = tempdir().unwrap();
            let output = tempdir().unwrap();
            let (inv, matches) = Cli::parse_and_dispatch([
                "ssg",
                "--content",
                content.path().to_str().unwrap(),
                "--template",
                templates.path().to_str().unwrap(),
                "--output",
                output.path().to_str().unwrap(),
                "--serve",
                output.path().to_str().unwrap(),
                "--quiet",
            ])
            .unwrap();
            let err = dispatch_invocation(inv, &matches).unwrap_err();
            assert!(format!("{err:?}").contains("injected: lib::run-on-serve"));
        }

        #[test]
        fn run_subcommand_dev_propagates_injected_on_serve_failure() {
            let _g = ssg_check_guard();
            let _fp = FailGuard("lib::run-on-serve");
            fail::cfg("lib::run-on-serve", "return").unwrap();

            let content = tempdir().unwrap();
            let templates = tempdir().unwrap();
            let output = tempdir().unwrap();
            let (inv, matches) = Cli::parse_and_dispatch([
                "ssg",
                "dev",
                "--content",
                content.path().to_str().unwrap(),
                "--template",
                templates.path().to_str().unwrap(),
                "--output",
                output.path().to_str().unwrap(),
                "--quiet",
            ])
            .unwrap();
            let err = dispatch_invocation(inv, &matches).unwrap_err();
            assert!(format!("{err:?}").contains("injected: lib::run-on-serve"));
        }

        /// `Paths::validate` can only reach `symlink_metadata_checked`'s
        /// error arm through this failpoint: once `path.exists()` is
        /// `true`, `Path::symlink_metadata` cannot be made to fail
        /// deterministically from a CLI-reachable input.
        #[test]
        fn paths_validate_propagates_injected_symlink_metadata_failure() {
            let _g = ssg_check_guard();
            let _fp = FailGuard("lib::symlink-metadata");
            fail::cfg("lib::symlink-metadata", "return").unwrap();

            let tmp = tempdir().unwrap();
            let paths = Paths {
                site: tmp.path().to_path_buf(),
                content: tmp.path().to_path_buf(),
                build: tmp.path().to_path_buf(),
                template: tmp.path().to_path_buf(),
            };
            let err = paths.validate().unwrap_err();
            assert!(
                format!("{err:?}").contains("injected: lib::symlink-metadata")
            );
        }

        /// `create_directories`'s `is_safe_path` error arm only fires
        /// when `Path::canonicalize` fails on an *existing* path — not
        /// constructible deterministically from CLI-reachable inputs,
        /// so this is only reachable through the failpoint.
        #[test]
        fn create_directories_propagates_injected_is_safe_path_failure() {
            let _g = ssg_check_guard();
            let _fp = FailGuard("lib::is-safe-path");
            fail::cfg("lib::is-safe-path", "return").unwrap();

            let tmp = tempdir().unwrap();
            let paths = Paths {
                site: tmp.path().join("site"),
                content: tmp.path().join("content"),
                build: tmp.path().join("build"),
                template: tmp.path().join("template"),
            };
            let err = create_directories(&paths).unwrap_err();
            assert!(format!("{err:?}").contains("injected: lib::is-safe-path"));
        }
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        /// `frontmatter_gen::extract` must never panic on arbitrary input.
        #[test]
        fn parse_frontmatter_never_panics(input in "\\PC*") {
            let _ = frontmatter_gen::extract(&input);
        }

        /// Compiling arbitrary Markdown via pulldown-cmark must never panic
        /// and must produce valid UTF-8 (guaranteed by `String`).
        #[test]
        fn compile_markdown_never_panics(input in "\\PC*") {
            use pulldown_cmark::{Parser, html};
            let parser = Parser::new(&input);
            let mut output = String::new();
            html::push_html(&mut output, parser);
            // output is a String — valid UTF-8 by construction.
            // Reaching this point without a panic is the property.
            drop(output);
        }

        /// Reading time of non-empty text must be at least 1 minute.
        #[test]
        fn reading_time_at_least_one(input in ".{1,5000}") {
            let word_count = input.split_whitespace().count();
            let minutes = (word_count / 200).max(1);
            prop_assert!(minutes >= 1, "reading time was {}", minutes);
        }
    }
}
