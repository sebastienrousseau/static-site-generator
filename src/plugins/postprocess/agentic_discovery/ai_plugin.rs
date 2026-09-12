// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `.well-known/ai-plugin.json` emitter — `OpenAI` plugin manifest spec.
//!
//! The `OpenAI` plugin manifest is still the de-facto plugin descriptor
//! across agent runtimes in 2026 (`ChatGPT`, Claude, Perplexity, IDE
//! plugins). It contains the metadata an agent needs to fetch a site's
//! `OpenAPI` spec, plus human/model-facing names and descriptions.
//!
//! Reference: <https://platform.openai.com/docs/plugins/getting-started/plugin-manifest>
//!
//! Shape emitted (AC3):
//!
//! ```jsonc
//! {
//!   "schema_version":         "v1",
//!   "name_for_human":         "<site_title>",
//!   "name_for_model":         "<site_name slug>",
//!   "description_for_human":  "<site_description>",
//!   "description_for_model":  "<site_description, model-facing>",
//!   "auth":                   { "type": "none" },
//!   "api":                    { "type": "openapi", "url": ".../openapi.yaml" },
//!   "logo_url":               "<base_url>/favicon.ico",
//!   "contact_email":          "support@<host>",
//!   "legal_info_url":         "<base_url>/legal"
//! }
//! ```

use crate::cmd::SsgConfig;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::PluginContext;
use serde_json::{json, Value};
use std::fs;

/// Render and write `.well-known/ai-plugin.json` under `ctx.site_dir`.
///
/// # Errors
///
/// Returns [`SsgError::Io`] if the `.well-known` directory cannot be
/// created or the manifest cannot be written.
///
/// # Examples
///
/// ```
/// use ssg::cmd::SsgConfig;
/// use ssg::plugin::PluginContext;
/// use ssg::postprocess::agentic_discovery::write_ai_plugin_json;
/// let tmp = tempfile::tempdir().unwrap();
/// let cfg = SsgConfig::builder()
///     .site_name("Example".into())
///     .base_url("https://example.com".into())
///     .build()
///     .unwrap();
/// let ctx = PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
/// write_ai_plugin_json(&ctx, &cfg).unwrap();
/// assert!(tmp.path().join(".well-known/ai-plugin.json").exists());
/// ```
pub fn write_ai_plugin_json(
    ctx: &PluginContext,
    cfg: &SsgConfig,
) -> Result<(), SsgError> {
    let well_known = ctx.site_dir.join(".well-known");
    fs::create_dir_all(&well_known).with_path(&well_known)?;
    let path = well_known.join("ai-plugin.json");
    let manifest = build_manifest(cfg);
    let body =
        serialize_ai_plugin(&manifest).map_err(|e| SsgError::io(e, &path))?;
    fs::write(&path, body).with_path(&path)?;
    Ok(())
}

/// Serialize the manifest with a fault-injection hook so tests can
/// drive the error-mapping branch (pretty-printing a `Value` cannot
/// fail in practice).
fn serialize_ai_plugin(manifest: &Value) -> serde_json::Result<String> {
    fail_point!("postprocess::ai-plugin-serialize", |_| Err(
        <serde_json::Error as serde::ser::Error>::custom(
            "injected: postprocess::ai-plugin-serialize"
        )
    ));
    serde_json::to_string_pretty(manifest)
}

/// Pure-function manifest builder — split from `write_ai_plugin_json`
/// so unit tests can assert the JSON shape without touching the
/// filesystem.
///
/// # Examples
///
/// ```
/// use ssg::cmd::SsgConfig;
/// use ssg::postprocess::agentic_discovery::build_manifest;
/// let cfg = SsgConfig::builder()
///     .site_name("Example".into())
///     .base_url("https://example.com".into())
///     .build()
///     .unwrap();
/// let m = build_manifest(&cfg);
/// assert_eq!(m["schema_version"], "v1");
/// assert_eq!(m["auth"]["type"], "none");
/// ```
#[must_use]
pub fn build_manifest(cfg: &SsgConfig) -> Value {
    let base_url = cfg.base_url.trim_end_matches('/').to_string();

    let human_name = if cfg.site_title.is_empty() {
        cfg.site_name.clone()
    } else {
        cfg.site_title.clone()
    };

    // OpenAI requires `name_for_model` to be lowercase letters,
    // digits and underscores. Slugify the site name aggressively.
    let model_name = slugify_for_model(&cfg.site_name);

    let description = if cfg.site_description.is_empty() {
        // Always emit something — empty `description_for_*` would fail
        // schema validation on the agent runtime side.
        format!("Content from {human_name}")
    } else {
        cfg.site_description.clone()
    };

    // OpenAPI URL: site author may host their own spec; we point at
    // a conventional location. The audit gate will surface a warning
    // if the file doesn't exist, but the manifest itself is valid.
    let openapi_url = if base_url.is_empty() {
        "/openapi.yaml".to_string()
    } else {
        format!("{base_url}/openapi.yaml")
    };

    let logo_url = if base_url.is_empty() {
        "/favicon.ico".to_string()
    } else {
        format!("{base_url}/favicon.ico")
    };

    let legal_url = if base_url.is_empty() {
        "/legal".to_string()
    } else {
        format!("{base_url}/legal")
    };

    let contact_email = derive_contact_email(&base_url);

    json!({
        "schema_version":         "v1",
        "name_for_human":         human_name,
        "name_for_model":         model_name,
        "description_for_human":  description,
        "description_for_model":  description_for_model(&human_name, cfg),
        "auth":                   { "type": "none" },
        "api": {
            "type":               "openapi",
            "url":                openapi_url,
            "is_user_authenticated": false,
        },
        "logo_url":               logo_url,
        "contact_email":          contact_email,
        "legal_info_url":         legal_url,
    })
}

/// Generates a model-facing description that's slightly more
/// directive than the human-facing one (agents perform better when
/// the description tells them *when* to invoke the plugin).
fn description_for_model(human_name: &str, cfg: &SsgConfig) -> String {
    // Site author supplied their own description? Trust it. Appending a
    // stock "Use this plugin to ..." sentence to an already-good
    // description reads like an auto-generated mash-up to LLM agents
    // (and to humans browsing the registry).
    if !cfg.site_description.is_empty() {
        return cfg.site_description.clone();
    }
    // No description configured — synthesise a model-friendly one with
    // an invocation hint.
    format!(
        "Plugin for accessing content from {human_name}. \
         Use this plugin to search and retrieve content from \
         {human_name}."
    )
}

/// Best-effort `contact_email` derivation: prefer `support@<host>` so
/// the manifest at least carries a deliverable address. Falls back to
/// `support@example.invalid` when no host is available.
fn derive_contact_email(base_url: &str) -> String {
    if let Some(host) = host_from_url(base_url) {
        format!("support@{host}")
    } else {
        "support@example.invalid".to_string()
    }
}

fn host_from_url(url: &str) -> Option<String> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    // `split` always yields at least one segment, so this cannot be
    // empty-handed; `unwrap_or_default` keeps the expression total
    // without an unreachable `None` branch.
    let host = without_scheme.split('/').next().unwrap_or_default();
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Coerce a free-form site name into something safe for
/// `name_for_model`. The spec allows `[a-z0-9_]` up to 50 chars.
fn slugify_for_model(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            for lower in c.to_lowercase() {
                out.push(lower);
            }
        } else if c == '_' || c == ' ' || c == '-' {
            out.push('_');
        }
        // Other characters are dropped silently.
    }
    // Collapse repeated underscores and trim.
    let collapsed: String = out
        .chars()
        .fold(String::new(), |mut acc, c| {
            if c == '_' && acc.ends_with('_') {
                // Skip duplicate underscore.
            } else {
                acc.push(c);
            }
            acc
        })
        .trim_matches('_')
        .to_string();
    if collapsed.is_empty() {
        "site".to_string()
    } else if collapsed.len() > 50 {
        collapsed[..50].to_string()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{ImageConfig, SsgConfig};
    use std::path::PathBuf;

    fn cfg() -> SsgConfig {
        SsgConfig {
            listings: Vec::new(),
            site_name: "Example Site".to_string(),
            site_title: "Example".to_string(),
            site_description: "A demo".to_string(),
            base_url: "https://example.com".to_string(),
            language: "en".to_string(),
            content_dir: PathBuf::from("content"),
            output_dir: PathBuf::from("build"),
            template_dir: PathBuf::from("templates"),
            theme: None,
            serve_dir: None,
            i18n: None,
            cdn_prefix: None,
            og_image: None,
            image: ImageConfig::default(),
            edge_headers: crate::cmd::EdgeHeadersConfig::default(),
            agents: None,
            transitions: false,
            security: crate::cmd::SecurityConfig::default(),
            no_taxonomy_pages: false,
        }
    }

    #[test]
    fn manifest_has_all_required_keys() {
        // AC3: the seven keys listed in the issue body must all be
        // present and non-null.
        let m = build_manifest(&cfg());
        for key in [
            "schema_version",
            "name_for_human",
            "name_for_model",
            "description_for_human",
            "description_for_model",
            "auth",
            "api",
        ] {
            assert!(m.get(key).is_some(), "missing key: {key}");
            assert!(!m[key].is_null(), "key {key} is null");
        }
    }

    #[test]
    fn schema_version_is_v1() {
        // OpenAI's plugin manifest spec is v1 — bumping this would be
        // an external break, so pin it.
        let m = build_manifest(&cfg());
        assert_eq!(m["schema_version"], "v1");
    }

    #[test]
    fn auth_is_none() {
        // Static sites have no authentication — emit the explicit
        // none-auth descriptor so the schema is valid.
        let m = build_manifest(&cfg());
        assert_eq!(m["auth"]["type"], "none");
    }

    #[test]
    fn api_uses_openapi_url() {
        let m = build_manifest(&cfg());
        assert_eq!(m["api"]["type"], "openapi");
        assert_eq!(m["api"]["url"], "https://example.com/openapi.yaml");
    }

    #[test]
    fn name_for_model_is_slug_safe() {
        // OpenAI requires [a-z0-9_]; verify a space becomes _ and
        // uppercase letters are downcased.
        let mut c = cfg();
        c.site_name = "Hello World!".to_string();
        let m = build_manifest(&c);
        let model_name = m["name_for_model"].as_str().unwrap();
        assert!(
            model_name.chars().all(|ch| ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || ch == '_'),
            "name_for_model must be [a-z0-9_]+, got {model_name:?}"
        );
    }

    #[test]
    fn name_for_model_collapses_underscores() {
        // Multiple separators in a row should collapse into one to
        // avoid `__` strings.
        assert_eq!(slugify_for_model("Hello   World!!"), "hello_world");
        assert_eq!(slugify_for_model("foo - bar"), "foo_bar");
    }

    #[test]
    fn name_for_model_falls_back_to_site_when_all_dropped() {
        // A name made entirely of dropped chars must not panic and
        // must yield a non-empty fallback.
        assert_eq!(slugify_for_model("!!!"), "site");
        assert_eq!(slugify_for_model(""), "site");
    }

    #[test]
    fn falls_back_to_site_name_when_title_empty() {
        // `name_for_human` prefers `site_title` but must fall back to
        // `site_name` when title is empty.
        let mut c = cfg();
        c.site_title = String::new();
        let m = build_manifest(&c);
        assert_eq!(m["name_for_human"], "Example Site");
    }

    #[test]
    fn description_for_model_returns_site_description_verbatim_when_present() {
        // When the site author supplied a description, we trust it —
        // no stock "Use this plugin to ..." suffix that turns their
        // copy into an obvious auto-generated mash-up.
        let m = build_manifest(&cfg());
        let desc = m["description_for_model"].as_str().unwrap();
        assert_eq!(desc, "A demo");
    }

    #[test]
    fn description_for_model_synthesises_invocation_hint_when_empty() {
        // No description configured? Synthesise one with an explicit
        // invocation hint — that's what agents need to know when to
        // invoke the plugin.
        let mut c = cfg();
        c.site_description = String::new();
        let m = build_manifest(&c);
        let desc = m["description_for_model"].as_str().unwrap();
        assert!(
            desc.contains("Use this plugin"),
            "synthesised description should hint at when to invoke, got {desc:?}"
        );
        // Non-short-circuiting `|` so both operands are evaluated.
        let mentions_site = desc.contains(c.site_title.as_str())
            | desc.contains(c.site_name.as_str());
        assert!(
            mentions_site,
            "synthesised description should mention the site, got {desc:?}"
        );
    }

    #[test]
    fn host_extraction_is_lenient() {
        assert_eq!(
            host_from_url("https://example.com/foo"),
            Some("example.com".to_string())
        );
        assert_eq!(
            host_from_url("http://example.com"),
            Some("example.com".to_string())
        );
        assert_eq!(host_from_url("notaurl"), None);
        assert_eq!(host_from_url(""), None);
    }

    #[test]
    fn contact_email_falls_back_when_no_host() {
        let mut c = cfg();
        c.base_url = String::new();
        let m = build_manifest(&c);
        let email = m["contact_email"].as_str().unwrap();
        assert!(email.contains('@'), "contact_email must contain @");
    }

    #[test]
    fn manifest_is_valid_json() {
        // serde_json::to_string_pretty must produce parseable JSON.
        let m = build_manifest(&cfg());
        let s = serde_json::to_string_pretty(&m).unwrap();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["schema_version"], "v1");
    }

    #[test]
    fn synthesises_empty_description_safely() {
        // An empty site_description must still produce a usable
        // description (agents will reject null/empty values).
        let mut c = cfg();
        c.site_description = String::new();
        let m = build_manifest(&c);
        let h = m["description_for_human"].as_str().unwrap();
        let model = m["description_for_model"].as_str().unwrap();
        assert!(!h.is_empty());
        assert!(!model.is_empty());
    }

    #[test]
    fn host_from_url_empty_host_returns_none() {
        // Scheme present but nothing after it → empty host → None.
        assert_eq!(host_from_url("https://"), None);
        assert_eq!(host_from_url("https:///path"), None);
    }

    #[test]
    fn name_for_model_truncates_to_fifty_chars() {
        let long = "a".repeat(60);
        let slug = slugify_for_model(&long);
        assert_eq!(slug.len(), 50);
        assert!(slug.chars().all(|c| c == 'a'));
    }

    #[test]
    fn write_ai_plugin_json_errors_when_well_known_is_a_file() {
        use crate::plugin::PluginContext;
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".well-known"), "file").unwrap();
        let ctx =
            PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
        let err = write_ai_plugin_json(&ctx, &cfg()).unwrap_err();
        assert!(format!("{err}").contains(".well-known"));
    }

    #[test]
    #[serial_test::parallel]
    fn write_ai_plugin_json_errors_when_target_is_a_directory() {
        use crate::plugin::PluginContext;
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".well-known/ai-plugin.json"))
            .unwrap();
        let ctx =
            PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
        let err = write_ai_plugin_json(&ctx, &cfg()).unwrap_err();
        assert!(format!("{err}").contains("ai-plugin.json"));
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::plugin::PluginContext;
    use serial_test::serial;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    fn cfg() -> SsgConfig {
        SsgConfig {
            listings: Vec::new(),
            site_name: "Example Site".to_string(),
            site_title: "Example".to_string(),
            site_description: "A demo".to_string(),
            base_url: "https://example.com".to_string(),
            language: "en".to_string(),
            content_dir: std::path::PathBuf::from("content"),
            output_dir: std::path::PathBuf::from("build"),
            template_dir: std::path::PathBuf::from("templates"),
            theme: None,
            serve_dir: None,
            i18n: None,
            cdn_prefix: None,
            og_image: None,
            image: crate::cmd::ImageConfig::default(),
            edge_headers: crate::cmd::EdgeHeadersConfig::default(),
            agents: None,
            transitions: false,
            security: crate::cmd::SecurityConfig::default(),
            no_taxonomy_pages: false,
        }
    }

    #[test]
    #[serial]
    fn write_ai_plugin_json_maps_serialize_failure_to_io_error() {
        let _guard = FailGuard("postprocess::ai-plugin-serialize");
        fail::cfg("postprocess::ai-plugin-serialize", "return")
            .expect("activate failpoint");

        let tmp = tempfile::tempdir().unwrap();
        let ctx =
            PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
        let err = write_ai_plugin_json(&ctx, &cfg())
            .expect_err("injected serialize failure must propagate");
        let msg = format!("{err}");
        assert!(msg.contains("ai-plugin.json"), "got: {msg}");
        assert!(
            msg.contains("injected: postprocess::ai-plugin-serialize"),
            "got: {msg}"
        );
    }
}
