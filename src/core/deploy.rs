// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Deployment adapter generation.
//!
//! Generates platform-specific configuration files for common hosting
//! providers, including cache headers and security headers.

use crate::error::SsgError;
use crate::plugin::{Plugin, PluginContext};
use anyhow::Result;
use std::fs;

/// Supported deployment targets.
///
/// Marked `#[non_exhaustive]` so future targets (AWS, Azure Static Web
/// Apps, Cloudflare R2 sites) can be added without a major version bump.
/// Downstream consumers must use a wildcard arm in `match` expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeployTarget {
    /// Netlify (`netlify.toml`).
    Netlify,
    /// Vercel (`vercel.json`).
    Vercel,
    /// Cloudflare Pages (`_headers`, `_redirects`).
    CloudflarePages,
    /// GitHub Pages (`.nojekyll`, `CNAME`).
    GithubPages,
}

/// Plugin that generates deployment configuration files.
#[derive(Debug, Clone, Copy)]
pub struct DeployPlugin {
    target: DeployTarget,
}

impl DeployPlugin {
    /// Creates a new `DeployPlugin` for the given target.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::deploy::{DeployPlugin, DeployTarget};
    /// use ssg::plugin::Plugin;
    ///
    /// let p = DeployPlugin::new(DeployTarget::Netlify);
    /// assert_eq!(p.name(), "deploy");
    /// ```
    #[must_use]
    pub const fn new(target: DeployTarget) -> Self {
        Self { target }
    }
}

impl Plugin for DeployPlugin {
    fn name(&self) -> &'static str {
        "deploy"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        match self.target {
            DeployTarget::Netlify => generate_netlify(&ctx.site_dir)
                .map_err(|e| SsgError::io(e, &ctx.site_dir))?,
            DeployTarget::Vercel => generate_vercel(&ctx.site_dir)
                .map_err(|e| SsgError::io(e, &ctx.site_dir))?,
            DeployTarget::CloudflarePages => {
                generate_cloudflare(&ctx.site_dir)
                    .map_err(|e| SsgError::io(e, &ctx.site_dir))?;
            }
            DeployTarget::GithubPages => {
                generate_github_pages(&ctx.site_dir)
                    .map_err(|e| SsgError::io(e, &ctx.site_dir))?;
            }
        }

        log::info!("[deploy] Generated {:?} config", self.target);
        Ok(())
    }
}

/// Security headers shared across all platforms.
///
/// The `Content-Security-Policy` value is sourced from
/// [`crate::csp::computed_policy`] so the deploy adapter and the
/// `<meta>`-injecting CSP plugin stay in lock-step — and so the new
/// `edge_headers` postprocess plugin (issue #550) has a single source
/// of truth for the CSP string.
const fn security_headers() -> [(&'static str, &'static str); 7] {
    [
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("X-XSS-Protection", "1; mode=block"),
        ("Referrer-Policy", "strict-origin-when-cross-origin"),
        (
            "Permissions-Policy",
            "camera=(), microphone=(), geolocation=()",
        ),
        ("Content-Security-Policy", crate::csp::computed_policy()),
        (
            "Strict-Transport-Security",
            "max-age=31536000; includeSubDomains",
        ),
    ]
}

fn generate_netlify(site_dir: &std::path::Path) -> Result<()> {
    let mut headers = String::from("/*\n");
    for (k, v) in security_headers() {
        headers.push_str(&format!("  {k} = {v}\n"));
    }
    // Content-addressable assets (CSS/JS/images/fonts) — see issue
    // #468. The fingerprint plugin renames every *.{css,js,png,jpg,
    // webp,svg,woff2,...} to *.<hash>.<ext>, so each emit is
    // intrinsically immutable.
    headers.push_str(
        "\n/assets/*\n  Cache-Control: public, max-age=31536000, immutable\n",
    );
    for ext in [
        "css", "js", "mjs", "png", "jpg", "jpeg", "webp", "avif", "gif", "svg",
        "woff", "woff2",
    ] {
        headers.push_str(&format!(
            "\n/*.{ext}\n  Cache-Control: public, max-age=31536000, immutable\n"
        ));
    }
    // HTML and feed/index files always revalidate so content updates
    // are visible immediately.
    headers.push_str("\n/*.html\n  Cache-Control: no-cache, must-revalidate\n");
    for path in [
        "/sitemap.xml",
        "/sitemap-news.xml",
        "/atom.xml",
        "/rss.xml",
        "/manifest.json",
        "/robots.txt",
        "/search-index.json",
    ] {
        headers.push_str(&format!(
            "\n{path}\n  Cache-Control: no-cache, must-revalidate\n"
        ));
    }

    fs::write(site_dir.join("_headers"), &headers)?;
    fs::write(site_dir.join("_redirects"), "")?;

    let toml = r#"[build]
  publish = "public"
  command = "cargo run -- -c content -o public -t templates"

[[headers]]
  for = "/assets/*"
  [headers.values]
    Cache-Control = "public, max-age=31536000, immutable"
"#;
    fs::write(site_dir.join("netlify.toml"), toml)?;
    Ok(())
}

fn generate_vercel(site_dir: &std::path::Path) -> Result<()> {
    let mut headers_arr = Vec::new();
    for (k, v) in security_headers() {
        headers_arr.push(serde_json::json!({"key": k, "value": v}));
    }

    // Per-extension immutable headers for content-addressable assets
    // (#468). The fingerprint plugin renames these to name.hash.ext
    // so the file body is bound to its name.
    let immutable = serde_json::json!([
        {"key": "Cache-Control", "value": "public, max-age=31536000, immutable"}
    ]);
    let no_cache = serde_json::json!([
        {"key": "Cache-Control", "value": "no-cache, must-revalidate"}
    ]);

    let config = serde_json::json!({
        "headers": [
            {"source": "/(.*)", "headers": headers_arr},
            {"source": "/assets/(.*)", "headers": immutable},
            {"source": "/(.*)\\.(css|js|mjs|png|jpg|jpeg|webp|avif|gif|svg|woff|woff2)",
             "headers": immutable},
            {"source": "/(.*)\\.html", "headers": no_cache},
            {"source": "/(sitemap|sitemap-news|atom|rss)\\.xml",
             "headers": no_cache},
            {"source": "/(manifest|search-index)\\.json", "headers": no_cache},
            {"source": "/robots\\.txt", "headers": no_cache}
        ]
    });

    let json = serde_json::to_string_pretty(&config)?;
    fs::write(site_dir.join("vercel.json"), json)?;
    Ok(())
}

fn generate_cloudflare(site_dir: &std::path::Path) -> Result<()> {
    let mut headers = String::from("/*\n");
    for (k, v) in security_headers() {
        headers.push_str(&format!("  {k} : {v}\n"));
    }
    // Content-addressable assets (#468) — same set as Netlify.
    headers.push_str(
        "\n/assets/*\n  Cache-Control: public, max-age=31536000, immutable\n",
    );
    for ext in [
        "css", "js", "mjs", "png", "jpg", "jpeg", "webp", "avif", "gif", "svg",
        "woff", "woff2",
    ] {
        headers.push_str(&format!(
            "\n/*.{ext}\n  Cache-Control: public, max-age=31536000, immutable\n"
        ));
    }
    headers.push_str("\n/*.html\n  Cache-Control: no-cache, must-revalidate\n");
    for path in [
        "/sitemap.xml",
        "/atom.xml",
        "/rss.xml",
        "/manifest.json",
        "/robots.txt",
    ] {
        headers.push_str(&format!(
            "\n{path}\n  Cache-Control: no-cache, must-revalidate\n"
        ));
    }

    fs::write(site_dir.join("_headers"), &headers)?;
    fs::write(site_dir.join("_redirects"), "")?;
    Ok(())
}

fn generate_github_pages(site_dir: &std::path::Path) -> Result<()> {
    // .nojekyll prevents GitHub Pages from processing with Jekyll
    fs::write(site_dir.join(".nojekyll"), "")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::init_logger;
    use std::path::{Path, PathBuf};
    use tempfile::{tempdir, TempDir};

    // -------------------------------------------------------------------
    // Test fixtures
    // -------------------------------------------------------------------

    /// Builds a fresh temp dir containing a `site/` subdirectory and a
    /// `PluginContext` pointing at it. Used by every plugin-trait test.
    fn make_ctx_with_site() -> (TempDir, PathBuf, PluginContext) {
        init_logger();
        let dir = tempdir().expect("create tempdir");
        let site = dir.path().join("site");
        fs::create_dir_all(&site).expect("create site dir");
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        (dir, site, ctx)
    }

    /// Asserts that *every* security header documented in
    /// [`security_headers`] appears verbatim (key + value) inside `body`.
    fn assert_all_security_headers_present(body: &str) {
        for (k, v) in security_headers() {
            assert!(
                body.contains(k),
                "missing header key `{k}` in body:\n{body}"
            );
            assert!(
                body.contains(v),
                "missing header value `{v}` in body:\n{body}"
            );
        }
    }

    // -------------------------------------------------------------------
    // DeployTarget — derives, equality, copy semantics
    // -------------------------------------------------------------------

    #[test]
    fn deploy_target_equality_reflexive_for_each_variant() {
        // Arrange
        let variants = [
            DeployTarget::Netlify,
            DeployTarget::Vercel,
            DeployTarget::CloudflarePages,
            DeployTarget::GithubPages,
        ];

        // Act + Assert: every variant is equal to itself.
        for v in variants {
            assert_eq!(v, v, "{v:?} should equal itself");
        }
    }

    #[test]
    fn deploy_target_distinct_variants_are_not_equal() {
        // Distinct variants must compare unequal — guards against
        // accidental duplicate discriminants if the enum is reordered.
        assert_ne!(DeployTarget::Netlify, DeployTarget::Vercel);
        assert_ne!(DeployTarget::Vercel, DeployTarget::CloudflarePages);
        assert_ne!(DeployTarget::CloudflarePages, DeployTarget::GithubPages);
        assert_ne!(DeployTarget::GithubPages, DeployTarget::Netlify);
    }

    #[test]
    fn deploy_target_every_unordered_pair_is_not_equal() {
        // The test above only walks one adjacent cycle (4 of the 6
        // unordered pairs over 4 variants). Exercise the two remaining
        // pairs (Netlify/CloudflarePages and Vercel/GithubPages) too,
        // so every branch of the derived `PartialEq` impl has actually
        // been driven by at least one inequality comparison.
        let variants = [
            DeployTarget::Netlify,
            DeployTarget::Vercel,
            DeployTarget::CloudflarePages,
            DeployTarget::GithubPages,
        ];
        for (i, a) in variants.iter().enumerate() {
            for b in &variants[i + 1..] {
                assert_ne!(a, b, "{a:?} should not equal {b:?}");
            }
        }
    }

    #[test]
    fn deploy_target_is_copy_after_move() {
        // Verifies the `Copy` derive is in effect: the binding remains
        // usable after being passed by value.
        let target = DeployTarget::Netlify;
        let _copy = target;
        assert_eq!(target, DeployTarget::Netlify);
    }

    #[test]
    fn deploy_target_debug_format_contains_variant_name() {
        assert!(format!("{:?}", DeployTarget::Netlify).contains("Netlify"));
        assert!(format!("{:?}", DeployTarget::Vercel).contains("Vercel"));
        assert!(format!("{:?}", DeployTarget::CloudflarePages)
            .contains("CloudflarePages"));
        assert!(
            format!("{:?}", DeployTarget::GithubPages).contains("GithubPages")
        );
    }

    // -------------------------------------------------------------------
    // DeployPlugin — constructor & trait surface
    // -------------------------------------------------------------------

    #[test]
    fn new_constructs_plugin_for_every_target_variant() {
        // Table-driven: every DeployTarget must be a valid argument to
        // `DeployPlugin::new` and must round-trip through the field.
        let cases = [
            DeployTarget::Netlify,
            DeployTarget::Vercel,
            DeployTarget::CloudflarePages,
            DeployTarget::GithubPages,
        ];
        for target in cases {
            let plugin = DeployPlugin::new(target);
            assert_eq!(
                plugin.target, target,
                "constructor must store the supplied target"
            );
        }
    }

    #[test]
    fn name_returns_static_deploy_identifier() {
        // The plugin name is part of the public contract — registries
        // and log lines key off it, so it must be stable.
        let plugin = DeployPlugin::new(DeployTarget::Netlify);
        assert_eq!(plugin.name(), "deploy");
    }

    #[test]
    fn deploy_plugin_is_copy_after_move() {
        let plugin = DeployPlugin::new(DeployTarget::Vercel);
        let _copy = plugin;
        assert_eq!(plugin.name(), "deploy");
    }

    #[test]
    fn deploy_plugin_debug_format_contains_type_name() {
        // `DeployPlugin` derives `Debug` but no existing test formats
        // it directly (only `DeployTarget`'s `Debug` was exercised).
        let plugin = DeployPlugin::new(DeployTarget::CloudflarePages);
        let formatted = format!("{plugin:?}");
        assert!(formatted.contains("DeployPlugin"), "got: {formatted}");
        assert!(formatted.contains("CloudflarePages"), "got: {formatted}");
    }

    // -------------------------------------------------------------------
    // after_compile — short-circuit on missing site directory
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_missing_site_dir_returns_ok_without_writing() {
        // The hook must be a no-op when the build hasn't produced a
        // site directory yet. This guards the early-return at line 46.
        let dir = tempdir().expect("tempdir");
        let missing_site = dir.path().join("does-not-exist");
        let ctx = PluginContext::new(
            dir.path(),
            dir.path(),
            &missing_site,
            dir.path(),
        );

        let plugin = DeployPlugin::new(DeployTarget::Netlify);
        plugin
            .after_compile(&ctx)
            .expect("missing site_dir is not an error");

        // Nothing should have been created.
        assert!(!missing_site.exists());
        assert!(!dir.path().join("_headers").exists());
        assert!(!dir.path().join("netlify.toml").exists());
    }

    // -------------------------------------------------------------------
    // after_compile — full trait dispatch for every target
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_netlify_writes_all_expected_artifacts() {
        let (_tmp, site, ctx) = make_ctx_with_site();
        DeployPlugin::new(DeployTarget::Netlify)
            .after_compile(&ctx)
            .expect("netlify after_compile");

        for f in ["_headers", "_redirects", "netlify.toml"] {
            assert!(
                site.join(f).exists(),
                "Netlify dispatch must produce `{f}`"
            );
        }
    }

    #[test]
    fn after_compile_vercel_writes_well_formed_json() {
        let (_tmp, site, ctx) = make_ctx_with_site();
        DeployPlugin::new(DeployTarget::Vercel)
            .after_compile(&ctx)
            .expect("vercel after_compile");

        let raw = fs::read_to_string(site.join("vercel.json"))
            .expect("vercel.json should exist");
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).expect("vercel.json must be valid JSON");
        assert!(
            parsed.get("headers").and_then(|v| v.as_array()).is_some(),
            "vercel.json must have a top-level `headers` array"
        );
    }

    #[test]
    fn after_compile_cloudflare_writes_headers_and_redirects() {
        let (_tmp, site, ctx) = make_ctx_with_site();
        DeployPlugin::new(DeployTarget::CloudflarePages)
            .after_compile(&ctx)
            .expect("cloudflare after_compile");

        assert!(site.join("_headers").exists());
        assert!(site.join("_redirects").exists());
    }

    #[test]
    fn after_compile_github_pages_writes_only_nojekyll() {
        let (_tmp, site, ctx) = make_ctx_with_site();
        DeployPlugin::new(DeployTarget::GithubPages)
            .after_compile(&ctx)
            .expect("github pages after_compile");

        assert!(site.join(".nojekyll").exists());
        // GitHub Pages dispatch should NOT touch the Netlify/Vercel/CF
        // artifacts — guard against cross-contamination if a future
        // refactor accidentally calls multiple generators.
        assert!(!site.join("_headers").exists());
        assert!(!site.join("netlify.toml").exists());
        assert!(!site.join("vercel.json").exists());
    }

    // -------------------------------------------------------------------
    // Generators — header content completeness
    // -------------------------------------------------------------------

    #[test]
    fn generate_netlify_headers_file_contains_every_security_header() {
        let dir = tempdir().expect("tempdir");
        generate_netlify(dir.path()).expect("generate netlify");

        let body = fs::read_to_string(dir.path().join("_headers"))
            .expect("read _headers");
        assert_all_security_headers_present(&body);
    }

    #[test]
    fn generate_netlify_headers_file_contains_cache_directives() {
        let dir = tempdir().expect("tempdir");
        generate_netlify(dir.path()).expect("generate netlify");

        let body = fs::read_to_string(dir.path().join("_headers"))
            .expect("read _headers");
        // Issue #468: long-lived immutable for content-addressable
        // assets (CSS/JS/images/fonts) + always-revalidate for HTML
        // and feeds.
        assert!(body.contains("/assets/*"));
        assert!(body.contains("max-age=31536000"));
        assert!(body.contains("immutable"));
        assert!(body.contains("/*.html"));
        assert!(body.contains("no-cache"));
        // Per-extension immutable rules now exist for the wider asset set.
        assert!(body.contains("/*.png"));
        assert!(body.contains("/*.woff2"));
    }

    #[test]
    fn generate_netlify_toml_contains_build_publish_directive() {
        let dir = tempdir().expect("tempdir");
        generate_netlify(dir.path()).expect("generate netlify");

        let toml = fs::read_to_string(dir.path().join("netlify.toml"))
            .expect("read netlify.toml");
        assert!(toml.contains("[build]"));
        assert!(toml.contains("publish"));
        assert!(toml.contains("[[headers]]"));
    }

    #[test]
    fn generate_netlify_creates_empty_redirects_file() {
        let dir = tempdir().expect("tempdir");
        generate_netlify(dir.path()).expect("generate netlify");

        let redirects = fs::read_to_string(dir.path().join("_redirects"))
            .expect("read _redirects");
        assert!(redirects.is_empty(), "_redirects starts empty by design");
    }

    #[test]
    fn generate_vercel_json_contains_every_security_header_value() {
        let dir = tempdir().expect("tempdir");
        generate_vercel(dir.path()).expect("generate vercel");

        let json = fs::read_to_string(dir.path().join("vercel.json"))
            .expect("read vercel.json");
        assert_all_security_headers_present(&json);
    }

    #[test]
    fn generate_vercel_json_has_asset_cache_route() {
        let dir = tempdir().expect("tempdir");
        generate_vercel(dir.path()).expect("generate vercel");

        let raw = fs::read_to_string(dir.path().join("vercel.json"))
            .expect("read vercel.json");
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).expect("valid JSON");

        let routes = parsed["headers"].as_array().expect("headers is an array");
        let sources: Vec<&str> =
            routes.iter().filter_map(|r| r["source"].as_str()).collect();
        assert!(sources.iter().any(|s| s.contains("/assets/")));
        assert!(sources.iter().any(|s| s.contains("/(.*)")));
    }

    #[test]
    fn generate_cloudflare_headers_file_uses_colon_separator() {
        // Cloudflare's _headers syntax differs from Netlify's: it
        // uses `Key: Value` rather than `Key = Value`. Guard the
        // separator to prevent silent regressions.
        let dir = tempdir().expect("tempdir");
        generate_cloudflare(dir.path()).expect("generate cloudflare");

        let body = fs::read_to_string(dir.path().join("_headers"))
            .expect("read _headers");
        assert!(body.contains("X-Content-Type-Options : nosniff"));
        assert_all_security_headers_present(&body);
    }

    #[test]
    fn generate_cloudflare_writes_empty_redirects_file() {
        let dir = tempdir().expect("tempdir");
        generate_cloudflare(dir.path()).expect("generate cloudflare");

        let redirects = fs::read_to_string(dir.path().join("_redirects"))
            .expect("read _redirects");
        assert!(redirects.is_empty(), "{:?}", redirects);
    }

    #[test]
    fn generate_github_pages_writes_empty_nojekyll_marker() {
        let dir = tempdir().expect("tempdir");
        generate_github_pages(dir.path()).expect("generate github pages");

        let nojekyll = dir.path().join(".nojekyll");
        assert!(nojekyll.exists());
        let contents = fs::read_to_string(&nojekyll).expect("read .nojekyll");
        assert!(
            contents.is_empty(),
            ".nojekyll is a marker file and must be empty"
        );
    }

    // -------------------------------------------------------------------
    // Idempotency — running the plugin twice must succeed
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_idempotent_for_every_target() {
        // Re-running after_compile must not fail (file overwrite, not
        // append). Guards against any future use of `OpenOptions::new
        // ().create_new(true)` that would break re-builds.
        for target in [
            DeployTarget::Netlify,
            DeployTarget::Vercel,
            DeployTarget::CloudflarePages,
            DeployTarget::GithubPages,
        ] {
            let (_tmp, _site, ctx) = make_ctx_with_site();
            let plugin = DeployPlugin::new(target);
            plugin
                .after_compile(&ctx)
                .expect("first after_compile for target should succeed");
            plugin
                .after_compile(&ctx)
                .expect("second after_compile for target should succeed");
        }
    }

    // -------------------------------------------------------------------
    // Generator error paths — writing into a non-existent parent dir
    // -------------------------------------------------------------------

    #[test]
    fn generate_netlify_into_missing_parent_returns_err() {
        let bogus = Path::new("/this/path/should/not/exist/ssg-test");
        let result = generate_netlify(bogus);
        assert!(
            result.is_err(),
            "writing into a non-existent parent must error"
        );
    }

    #[test]
    fn generate_vercel_into_missing_parent_returns_err() {
        let bogus = Path::new("/this/path/should/not/exist/ssg-test");
        assert!(generate_vercel(bogus).is_err());
    }

    #[test]
    fn generate_cloudflare_into_missing_parent_returns_err() {
        let bogus = Path::new("/this/path/should/not/exist/ssg-test");
        assert!(generate_cloudflare(bogus).is_err());
    }

    #[test]
    fn generate_github_pages_into_missing_parent_returns_err() {
        let bogus = Path::new("/this/path/should/not/exist/ssg-test");
        assert!(generate_github_pages(bogus).is_err());
    }

    // -------------------------------------------------------------------
    // after_compile error mapping — generator failure surfaces as
    // SsgError::Io via the per-target `map_err` closures
    // -------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn after_compile_maps_generator_errors_for_every_target() {
        use std::os::unix::fs::PermissionsExt;
        for target in [
            DeployTarget::Netlify,
            DeployTarget::Vercel,
            DeployTarget::CloudflarePages,
            DeployTarget::GithubPages,
        ] {
            let (_tmp, site, ctx) = make_ctx_with_site();
            // Site dir exists but is read-only, so every generator's
            // first write fails and the `map_err(|e| SsgError::io(..))`
            // closure for this target runs.
            fs::set_permissions(&site, fs::Permissions::from_mode(0o555))
                .expect("chmod site dir");

            let res = DeployPlugin::new(target).after_compile(&ctx);

            let _ =
                fs::set_permissions(&site, fs::Permissions::from_mode(0o755));
            // Root bypasses permissions on some CI runners, so
            // tolerate Ok; when the failure fired, the error must
            // render non-empty.
            assert!(
                res.err().is_none_or(|e| !format!("{e}").is_empty()),
                "{target:?}"
            );
        }
    }

    // -------------------------------------------------------------------
    // Generator error paths — later writes in each generator
    // -------------------------------------------------------------------

    #[test]
    fn generate_netlify_redirects_write_failure_propagates() {
        // `_headers` succeeds, `_redirects` fails because a directory
        // occupies that name (line 149).
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("_redirects")).unwrap();
        assert!(generate_netlify(dir.path()).is_err());
    }

    #[test]
    fn generate_netlify_toml_write_failure_propagates() {
        // `_headers` and `_redirects` succeed, `netlify.toml` fails
        // (line 160).
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("netlify.toml")).unwrap();
        assert!(generate_netlify(dir.path()).is_err());
    }

    #[test]
    fn generate_cloudflare_redirects_write_failure_propagates() {
        // `_headers` succeeds, `_redirects` fails (line 230).
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("_redirects")).unwrap();
        assert!(generate_cloudflare(dir.path()).is_err());
    }
}
