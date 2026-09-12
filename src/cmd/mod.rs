// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! # Command Line Interface Module
//!
//! This module provides a secure and robust command-line interface (CLI) for the
//! **Static Site Generator (SSG)**. It handles argument parsing, configuration management,
//! and validation of user inputs to ensure that the static site generator operates
//! reliably and securely.
//!
//! ## Key Features
//! - Safe path handling (including symbolic link checks and canonicalization)
//! - Input validation (URL, language, environment variables)
//! - Secure configuration with size-limited config files
//! - Builder pattern for convenient configuration construction
//! - Error handling via `CliError`
//!
//! ## Example Usage
//! ```rust,no_run
//! use ssg::cmd::{Cli, SsgConfig};
//!
//! fn main() -> anyhow::Result<()> {
//!     let matches = Cli::build().get_matches();
//!
//!     // Attempt to load configuration from command-line arguments
//!     let mut config = SsgConfig::from_matches(&matches)?;
//!
//!     println!("Configuration loaded: {:?}", config);
//!     // Continue with application logic...
//!     Ok(())
//! }
//! ```

pub mod audit;
mod cli;
pub mod completions;
mod config;
mod error;
pub mod man;
mod validation;

pub use cli::{
    Cli, CliInvocation, DEPLOY_TARGETS, LEGACY_DEPRECATION_WARNING, SUBCOMMANDS,
};
pub use config::{
    EdgeHeadersConfig, ImageConfig, SecurityConfig, SriAlgorithm, SsgConfig,
    SsgConfigBuilder,
};
pub use error::{CliError, LanguageCode};
pub use validation::{is_valid_url, validate_url};

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// Default port for the local development server.
pub const DEFAULT_PORT: u16 = 8000;
/// Default host for the local development server.
///
/// Loopback by default. WSL2 users whose Windows host can't reach the
/// distro on `127.0.0.1` (and Codespaces / dev-containers users binding
/// outside their network namespace) should set `SSG_HOST=0.0.0.0` and
/// let [`resolve_host`] pick it up. The same applies to `SSG_PORT`.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Resolve the dev-server host, preferring `$SSG_HOST` over [`DEFAULT_HOST`].
///
/// Returns the value of the `SSG_HOST` environment variable if set and
/// non-empty; otherwise returns the compiled-in default.
///
/// # Examples
///
/// ```rust
/// use ssg::cmd::{resolve_host, DEFAULT_HOST};
///
/// // With no env override the compiled-in default is returned.
/// std::env::remove_var("SSG_HOST");
/// assert_eq!(resolve_host(), DEFAULT_HOST);
/// ```
#[must_use]
pub fn resolve_host() -> String {
    std::env::var("SSG_HOST")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_HOST.to_string())
}

/// Resolve the dev-server port, preferring `$SSG_PORT` over [`DEFAULT_PORT`].
///
/// # Examples
///
/// ```rust
/// use ssg::cmd::{resolve_port, DEFAULT_PORT};
///
/// std::env::remove_var("SSG_PORT");
/// assert_eq!(resolve_port(), DEFAULT_PORT);
/// ```
#[must_use]
pub fn resolve_port() -> u16 {
    std::env::var("SSG_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Reserved names that cannot be used as paths on Windows systems.
pub const RESERVED_NAMES: &[&str] =
    &["con", "aux", "nul", "prn", "com1", "lpt1"];
/// Maximum allowed size in bytes for config files.
pub const MAX_CONFIG_SIZE: usize = 1024 * 1024; // 1MB limit

/// Default site name for the configuration.
///
/// Used for the scaffold directory name (`ssg --new`), never rendered into a
/// page. Contrast [`DEFAULT_SITE_TITLE`], which is.
pub const DEFAULT_SITE_NAME: &str = "MySsgSite";

/// Default site title for the configuration — deliberately empty.
///
/// This value **reaches rendered HTML**: the taxonomy plugin puts it in
/// `site.title`, and `templates/tera/base.html` appends it to every page
/// title as `<title>{page} — {site.title}</title>`.
///
/// It used to be `"My SSG Site"`. A site built without a config file therefore
/// shipped that placeholder as its brand — on sebastienrousseau.com it reached
/// 7,189 generated tag pages, each titled `Tag: <term> — My SSG Site`, live and
/// indexable. Nothing caught it because the defaults were only ever asserted
/// *equal to their own constant*; no test asked whether they escaped into
/// output.
///
/// Empty is the safe default: `base.html` guards the suffix with
/// `{% if site.title %}`, so an unconfigured build now renders `<title>{page}</title>`
/// and brands nothing. Sites that want a suffix set `site_title` explicitly,
/// and `ssg` warns when it falls back to defaults.
///
/// See `tests/no_placeholder_in_output.rs`, which fails if any placeholder
/// constant appears anywhere in a rendered site.
pub const DEFAULT_SITE_TITLE: &str = "";

/// A static default configuration for the SSG site.
pub static DEFAULT_CONFIG: OnceLock<Arc<SsgConfig>> = OnceLock::new();

/// Returns a reference to the lazily-initialised default configuration.
///
/// # Examples
///
/// ```rust
/// use ssg::cmd::{default_config, DEFAULT_SITE_NAME};
///
/// let cfg = default_config();
/// assert_eq!(cfg.site_name, DEFAULT_SITE_NAME);
/// ```
pub fn default_config() -> &'static Arc<SsgConfig> {
    DEFAULT_CONFIG.get_or_init(|| {
        Arc::new(SsgConfig {
            site_name: DEFAULT_SITE_NAME.to_string(),
            listings: Vec::new(),
            content_dir: PathBuf::from("content"),
            output_dir: PathBuf::from("public"),
            template_dir: PathBuf::from("templates"),
            theme: None,
            serve_dir: None,
            base_url: format!("http://{DEFAULT_HOST}:{DEFAULT_PORT}"),
            site_title: DEFAULT_SITE_TITLE.to_string(),
            site_description: "A site built with SSG".to_string(),
            language: "en-GB".to_string(),
            i18n: None,
            cdn_prefix: None,
            og_image: None,
            image: ImageConfig::default(),
            edge_headers: EdgeHeadersConfig::default(),
            agents: None,
            transitions: false,
            // Default: generate taxonomy pages, as before.
            no_taxonomy_pages: false,
            security: SecurityConfig::default(),
        })
    })
}

/// Const validation for compile-time checks.
const _: () = {
    assert!(MAX_CONFIG_SIZE > 0);
    assert!(MAX_CONFIG_SIZE <= 10 * 1024 * 1024); // Max 10MB
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutex-protected env-var setter so concurrent tests don't race.
    /// `cargo test` runs tests in parallel by default; without serialisation
    /// the env-var assertions below would interleave nondeterministically.
    fn with_env<F: FnOnce()>(key: &str, value: Option<&str>, f: F) {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var(key).ok();
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        f();
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn resolve_host_returns_default_when_env_unset() {
        with_env("SSG_HOST", None, || {
            assert_eq!(resolve_host(), DEFAULT_HOST);
        });
    }

    #[test]
    fn resolve_host_returns_env_value_when_set() {
        with_env("SSG_HOST", Some("0.0.0.0"), || {
            assert_eq!(resolve_host(), "0.0.0.0");
        });
    }

    #[test]
    fn resolve_host_returns_default_when_env_empty() {
        // Empty string should fall through to the default — matters for
        // shells that export `SSG_HOST=` to "unset" without `unset`.
        with_env("SSG_HOST", Some(""), || {
            assert_eq!(resolve_host(), DEFAULT_HOST);
        });
    }

    #[test]
    fn resolve_port_returns_default_when_env_unset() {
        with_env("SSG_PORT", None, || {
            assert_eq!(resolve_port(), DEFAULT_PORT);
        });
    }

    #[test]
    fn resolve_port_returns_env_value_when_set() {
        with_env("SSG_PORT", Some("8080"), || {
            assert_eq!(resolve_port(), 8080);
        });
    }

    #[test]
    fn resolve_port_returns_default_when_env_unparseable() {
        with_env("SSG_PORT", Some("not-a-number"), || {
            assert_eq!(resolve_port(), DEFAULT_PORT);
        });
    }

    #[test]
    fn default_config_returns_lazily_initialised_singleton() {
        let a = default_config();
        let b = default_config();
        // Same Arc pointer — confirms OnceLock is being reused.
        assert!(Arc::ptr_eq(a, b));
        assert_eq!(a.site_name, DEFAULT_SITE_NAME);
        assert_eq!(a.site_title, DEFAULT_SITE_TITLE);
        assert_eq!(a.language, "en-GB");
        assert_eq!(a.content_dir, PathBuf::from("content"));
        assert_eq!(a.output_dir, PathBuf::from("public"));
        assert_eq!(a.template_dir, PathBuf::from("templates"));
        assert!(a.serve_dir.is_none());
        assert!(a.i18n.is_none());
    }

    #[test]
    fn default_config_base_url_uses_default_host_and_port() {
        // Plain conditions only: format-arg expressions in an assert
        // message are evaluated lazily and would leave never-executed
        // regions behind.
        let cfg = default_config();
        assert!(cfg.base_url.contains(DEFAULT_HOST));
        assert!(cfg.base_url.contains(&DEFAULT_PORT.to_string()));
    }

    #[test]
    fn with_env_restores_previous_value_and_survives_poisoning() {
        const KEY: &str = "SSG_TEST_WITH_ENV_POISON";

        // Poison the internal ENV_LOCK by panicking inside `f`.
        let poisoned = std::panic::catch_unwind(|| {
            with_env(KEY, Some("first"), || panic!("poison the env lock"));
        });
        assert!(poisoned.is_err(), "the panic must propagate");

        // The panic skipped the restore, so KEY is still "first".
        // This call must (a) recover from the poisoned lock and
        // (b) restore the previous value afterwards.
        with_env(KEY, Some("second"), || {
            assert_eq!(std::env::var(KEY).ok().as_deref(), Some("second"));
        });
        assert_eq!(std::env::var(KEY).ok().as_deref(), Some("first"));

        std::env::remove_var(KEY);
    }

    #[test]
    fn reserved_names_are_lowercase_and_non_empty() {
        assert!(!RESERVED_NAMES.is_empty());
        for name in RESERVED_NAMES {
            assert!(!name.is_empty(), "reserved name should be non-empty");
            assert_eq!(
                *name,
                name.to_lowercase(),
                "reserved name should be lowercase: {name}"
            );
        }
    }

    #[test]
    fn max_config_size_is_one_megabyte() {
        assert_eq!(MAX_CONFIG_SIZE, 1024 * 1024);
    }
}
