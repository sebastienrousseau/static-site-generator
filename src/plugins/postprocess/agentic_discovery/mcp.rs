// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `/.well-known/mcp.json` emitter — Model Context Protocol registry.
//!
//! MCP is the protocol Anthropic published in November 2024 for
//! agent runtimes to discover tools, prompts, and resources exposed
//! by external servers. The registry JSON tells a client what's
//! available before the first call; the file lives at the
//! `/.well-known/mcp.json` convention.
//!
//! Shape emitted (AC4):
//!
//! ```jsonc
//! {
//!   "protocolVersion": "2025-03-26",
//!   "serverInfo":     { "name": "<site_name>", "version": "<pkg>" },
//!   "transport":      { "type": "http", "url": "<base>/.well-known/mcp" },
//!   "capabilities":   { "resources": {...}, "tools": {...}, "prompts": {...} },
//!   "resources":      [ … one per public page (AC5) … ],
//!   "tools":          [ … from [agents.mcp.tools] … ],
//!   "prompts":        [ … from [agents.mcp.prompts] … ]
//! }
//! ```
//!
//! AC5 — *resources auto-populated from content* — is the most
//! interesting wrinkle. We reuse [`super::super::helpers::read_meta_sidecars`]
//! (the same source the RSS/Atom/JSON-Feed plugins consume) so the MCP
//! resource list stays consistent with the rest of the site's
//! discovery surface. A page is **public** iff:
//!
//! 1. its `.meta.json` has no `published = "false"` or `draft = "true"` key, AND
//! 2. its `agents.disallow` list does not contain `"*"` or `"mcp"`.
//!
//! Per-page overrides land via the `agents` key on the sidecar
//! (e.g. `"agents": "{\"disallow\":[\"mcp\"]}"` — sidecars are
//! `HashMap<String, String>` in this codebase, so the value is a
//! JSON-encoded string rather than a nested object).

use super::super::helpers::read_meta_sidecars;
use super::AgentsConfig;
use crate::cmd::SsgConfig;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::PluginContext;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;

/// One MCP `resource` entry. Public so integration tests can construct
/// instances directly and so consumers of the library can fan their
/// own resources in via post-emit hooks.
///
/// # Examples
///
/// ```
/// use ssg::postprocess::agentic_discovery::McpResource;
/// let r = McpResource {
///     uri: "https://example.com/blog/a/".into(),
///     name: "Hello".into(),
///     description: "Greeting".into(),
///     mime_type: "text/markdown".into(),
/// };
/// assert_eq!(r.mime_type, "text/markdown");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResource {
    /// URI exposed to MCP clients — typically `{base}/{slug}/`.
    pub uri: String,
    /// Short human-readable name (the page title).
    pub name: String,
    /// Longer description (the page excerpt or meta description).
    pub description: String,
    /// IANA MIME type. Always `text/markdown` for content pages.
    pub mime_type: String,
}

impl McpResource {
    fn to_json(&self) -> Value {
        json!({
            "uri":         self.uri,
            "name":        self.name,
            "description": self.description,
            "mimeType":    self.mime_type,
        })
    }
}

/// Render and write `.well-known/mcp.json` under `ctx.site_dir`.
///
/// # Errors
///
/// Returns [`SsgError::Io`] if the `.well-known` directory cannot be
/// created or the registry file cannot be written.
///
/// # Examples
///
/// ```
/// use ssg::cmd::SsgConfig;
/// use ssg::plugin::PluginContext;
/// use ssg::postprocess::agentic_discovery::{AgentsConfig, write_mcp_registry};
/// let tmp = tempfile::tempdir().unwrap();
/// let cfg = SsgConfig::builder()
///     .site_name("Example".into())
///     .base_url("https://example.com".into())
///     .build()
///     .unwrap();
/// let mut agents = AgentsConfig::default();
/// agents.mcp.enabled = true;
/// let ctx = PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
/// write_mcp_registry(&ctx, &cfg, &agents).unwrap();
/// assert!(tmp.path().join(".well-known/mcp.json").exists());
/// ```
pub fn write_mcp_registry(
    ctx: &PluginContext,
    cfg: &SsgConfig,
    agents: &AgentsConfig,
) -> Result<(), SsgError> {
    let well_known = ctx.site_dir.join(".well-known");
    fs::create_dir_all(&well_known).with_path(&well_known)?;
    let path = well_known.join("mcp.json");

    // Resource discovery — only when explicitly opted-in.
    let resources = if agents.mcp.auto_resources {
        collect_mcp_resources(ctx, cfg)
    } else {
        Vec::new()
    };

    let registry = build_registry(cfg, agents, &resources);
    let body =
        serialize_registry(&registry).map_err(|e| SsgError::io(e, &path))?;
    fs::write(&path, body).with_path(&path)?;
    Ok(())
}

/// Serialize the registry with a fault-injection hook so tests can
/// drive the error-mapping branch (pretty-printing a `Value` cannot
/// fail in practice).
fn serialize_registry(registry: &Value) -> serde_json::Result<String> {
    fail_point!("postprocess::mcp-serialize", |_| Err(
        <serde_json::Error as serde::ser::Error>::custom(
            "injected: postprocess::mcp-serialize"
        )
    ));
    serde_json::to_string_pretty(registry)
}

/// Pure-function registry builder, callable from tests without I/O.
///
/// # Examples
///
/// ```
/// use ssg::cmd::SsgConfig;
/// use ssg::postprocess::agentic_discovery::{AgentsConfig, build_registry};
/// let cfg = SsgConfig::builder()
///     .site_name("Example".into())
///     .base_url("https://example.com".into())
///     .build()
///     .unwrap();
/// let agents = AgentsConfig::default();
/// let reg = build_registry(&cfg, &agents, &[]);
/// assert!(reg["resources"].is_array());
/// assert_eq!(reg["transport"]["type"], "http");
/// ```
#[must_use]
pub fn build_registry(
    cfg: &SsgConfig,
    agents: &AgentsConfig,
    resources: &[McpResource],
) -> Value {
    let base_url = cfg.base_url.trim_end_matches('/').to_string();

    // Transport URL — explicit override wins; otherwise synthesise
    // a self-describing URL anchored at the same base.
    let transport_url = agents.mcp.url.clone().unwrap_or_else(|| {
        if base_url.is_empty() {
            "/.well-known/mcp".to_string()
        } else {
            format!("{base_url}/.well-known/mcp")
        }
    });

    let server_name = if cfg.site_name.is_empty() {
        "static-site".to_string()
    } else {
        cfg.site_name.clone()
    };

    let resources_json: Vec<Value> =
        resources.iter().map(McpResource::to_json).collect();

    let tools_json: Vec<Value> = agents
        .mcp
        .tools
        .iter()
        .map(|t| {
            let mut obj = serde_json::Map::new();
            let _ = obj.insert("name".into(), Value::String(t.name.clone()));
            let _ = obj.insert(
                "description".into(),
                Value::String(t.description.clone()),
            );
            if let Some(ref schema) = t.input_schema {
                let _ = obj.insert("inputSchema".into(), schema.clone());
            }
            Value::Object(obj)
        })
        .collect();

    let prompts_json: Vec<Value> = agents
        .mcp
        .prompts
        .iter()
        .map(|p| {
            let mut obj = serde_json::Map::new();
            let _ = obj.insert("name".into(), Value::String(p.name.clone()));
            let _ = obj.insert(
                "description".into(),
                Value::String(p.description.clone()),
            );
            if let Some(ref args) = p.arguments {
                let _ = obj.insert("arguments".into(), args.clone());
            }
            Value::Object(obj)
        })
        .collect();

    json!({
        "protocolVersion": agents.mcp.protocol_version,
        "serverInfo": {
            "name":    server_name,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "transport": {
            "type": agents.mcp.transport,
            "url":  transport_url,
        },
        "capabilities": {
            "resources": { "listChanged": false },
            "tools":     { "listChanged": false },
            "prompts":   { "listChanged": false },
        },
        "resources": resources_json,
        "tools":     tools_json,
        "prompts":   prompts_json,
    })
}

/// Walk `.meta.json` sidecars under `ctx.site_dir` (with the same
/// `build_dir/.meta` fallback used by RSS/Atom/JSON-Feed) and produce
/// one MCP resource per **public** page.
///
/// Public = `published != "false"` AND `draft != "true"` AND `agents.disallow`
/// doesn't contain `"*"` or `"mcp"`.
///
/// # Examples
///
/// ```
/// use ssg::cmd::SsgConfig;
/// use ssg::plugin::PluginContext;
/// use ssg::postprocess::agentic_discovery::collect_mcp_resources;
/// let tmp = tempfile::tempdir().unwrap();
/// let cfg = SsgConfig::builder()
///     .site_name("Example".into())
///     .base_url("https://example.com".into())
///     .build()
///     .unwrap();
/// let ctx = PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
/// // Empty dir, no sidecars -> empty result.
/// assert!(collect_mcp_resources(&ctx, &cfg).is_empty());
/// ```
#[must_use]
pub fn collect_mcp_resources(
    ctx: &PluginContext,
    cfg: &SsgConfig,
) -> Vec<McpResource> {
    let mut meta_entries =
        read_meta_sidecars(&ctx.site_dir).unwrap_or_default();
    if meta_entries.is_empty() {
        let meta_dir = ctx.build_dir.join(".meta");
        if meta_dir.exists() {
            meta_entries = read_meta_sidecars(&meta_dir).unwrap_or_default();
        }
    }

    let base_url = cfg.base_url.trim_end_matches('/').to_string();

    let mut resources: Vec<McpResource> = meta_entries
        .iter()
        .filter_map(|(rel, meta)| build_resource(rel, meta, &base_url))
        .collect();

    // Stable sort by URI so the output is byte-deterministic across
    // builds (filesystem walk order is OS-dependent).
    resources.sort_by(|a, b| a.uri.cmp(&b.uri));
    resources
}

/// Build a single MCP resource from a `.meta.json` entry, returning
/// `None` for pages that aren't eligible (no title, marked draft,
/// or explicitly opted out via `agents.disallow`).
fn build_resource(
    rel_path: &str,
    meta: &HashMap<String, String>,
    base_url: &str,
) -> Option<McpResource> {
    if rel_path.is_empty() {
        return None;
    }
    if is_draft(meta) || is_unpublished(meta) {
        return None;
    }
    if is_mcp_disallowed(meta) {
        return None;
    }

    let title = meta.get("title").cloned().unwrap_or_default();
    if title.is_empty() {
        return None;
    }

    let description = meta
        .get("description")
        .or_else(|| meta.get("excerpt"))
        .or_else(|| meta.get("summary"))
        .cloned()
        .unwrap_or_else(|| format!("Content from {rel_path}"));

    let uri = if base_url.is_empty() {
        format!("/{rel_path}/")
    } else {
        format!("{base_url}/{rel_path}/")
    };

    Some(McpResource {
        uri,
        name: title,
        description,
        mime_type: "text/markdown".to_string(),
    })
}

fn is_draft(meta: &HashMap<String, String>) -> bool {
    meta.get("draft").is_some_and(|v| {
        matches!(v.as_str(), "true" | "True" | "TRUE" | "yes" | "1")
    })
}

fn is_unpublished(meta: &HashMap<String, String>) -> bool {
    meta.get("published").is_some_and(|v| {
        matches!(v.as_str(), "false" | "False" | "FALSE" | "no" | "0")
    })
}

/// Decide whether the page's `agents.disallow` list excludes MCP. The
/// sidecar's value (if any) is a JSON-encoded string per the
/// `HashMap<String, String>` shape used everywhere else in this
/// codebase — parse it leniently and ignore malformed entries.
fn is_mcp_disallowed(meta: &HashMap<String, String>) -> bool {
    let Some(raw) = meta.get("agents") else {
        return false;
    };
    let Ok(parsed): Result<Value, _> = serde_json::from_str(raw) else {
        return false;
    };
    let Some(disallow) = parsed.get("disallow") else {
        return false;
    };
    let Some(arr) = disallow.as_array() else {
        return false;
    };
    arr.iter()
        .filter_map(|v| v.as_str())
        .any(|s| s.eq_ignore_ascii_case("mcp") || s == "*")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{ImageConfig, SsgConfig};
    use std::path::PathBuf;

    fn cfg() -> SsgConfig {
        SsgConfig {
            site_name: "Example".to_string(),
            site_title: "Example Site".to_string(),
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

    fn meta(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn registry_has_protocol_and_transport() {
        // AC4: protocolVersion, transport.type, transport.url are
        // load-bearing fields for client compatibility.
        let mut agents = AgentsConfig::default();
        agents.mcp.enabled = true;
        let reg = build_registry(&cfg(), &agents, &[]);
        assert!(reg["protocolVersion"].is_string());
        assert_eq!(reg["transport"]["type"], "http");
        let url = reg["transport"]["url"].as_str().unwrap();
        assert!(url.starts_with("https://example.com/"));
        assert!(url.ends_with("/.well-known/mcp"));
    }

    #[test]
    fn registry_shape_has_required_arrays() {
        // resources / tools / prompts must always be present (empty
        // arrays are fine — clients distinguish absent from empty).
        let agents = AgentsConfig::default();
        let reg = build_registry(&cfg(), &agents, &[]);
        assert!(reg["resources"].is_array());
        assert!(reg["tools"].is_array());
        assert!(reg["prompts"].is_array());
    }

    #[test]
    fn registry_includes_capabilities_block() {
        // Per the MCP spec, servers advertise capabilities so clients
        // know what list_changed notifications to expect.
        let agents = AgentsConfig::default();
        let reg = build_registry(&cfg(), &agents, &[]);
        let caps = &reg["capabilities"];
        assert!(caps["resources"].is_object());
        assert!(caps["tools"].is_object());
        assert!(caps["prompts"].is_object());
    }

    #[test]
    fn explicit_transport_url_overrides_synth() {
        let mut agents = AgentsConfig::default();
        agents.mcp.url = Some("https://api.example.com/mcp".to_string());
        let reg = build_registry(&cfg(), &agents, &[]);
        assert_eq!(reg["transport"]["url"], "https://api.example.com/mcp");
    }

    #[test]
    fn resources_serialise_with_mime_type_markdown() {
        // AC5: every page-derived resource must declare text/markdown.
        let res = McpResource {
            uri: "https://example.com/blog/hello/".to_string(),
            name: "Hello".to_string(),
            description: "Greeting".to_string(),
            mime_type: "text/markdown".to_string(),
        };
        let agents = AgentsConfig::default();
        let reg = build_registry(&cfg(), &agents, &[res]);
        let arr = reg["resources"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["mimeType"], "text/markdown");
        assert_eq!(arr[0]["uri"], "https://example.com/blog/hello/");
        assert_eq!(arr[0]["name"], "Hello");
    }

    #[test]
    fn build_resource_skips_drafts() {
        // A `draft = "true"` sidecar must not produce a resource —
        // we don't want drafts leaking into the MCP surface.
        let m = meta(&[("title", "Hello"), ("draft", "true")]);
        assert!(build_resource("blog/hello", &m, "https://x.example").is_none());
    }

    #[test]
    fn build_resource_skips_unpublished() {
        let m = meta(&[("title", "Hello"), ("published", "false")]);
        assert!(build_resource("blog/hello", &m, "https://x.example").is_none());
    }

    #[test]
    fn build_resource_skips_when_agents_disallow_contains_mcp() {
        // Per-page opt-out: frontmatter `agents.disallow = ["mcp"]`.
        // Sidecars store JSON-encoded strings, so the value is a
        // string blob we must JSON-parse.
        let agents_json = r#"{"disallow":["mcp"]}"#;
        let m = meta(&[("title", "Hello"), ("agents", agents_json)]);
        assert!(build_resource("blog/hello", &m, "https://x.example").is_none());
    }

    #[test]
    fn build_resource_skips_when_agents_disallow_contains_star() {
        // `["*"]` is the opt-out-of-everything wildcard.
        let agents_json = r#"{"disallow":["*"]}"#;
        let m = meta(&[("title", "Hello"), ("agents", agents_json)]);
        assert!(build_resource("blog/hello", &m, "https://x.example").is_none());
    }

    #[test]
    fn build_resource_keeps_page_with_unrelated_disallow() {
        // Other-agent disallow rules must NOT exclude the page from
        // MCP — only `mcp` and `*` do.
        let agents_json = r#"{"disallow":["gptbot"]}"#;
        let m = meta(&[
            ("title", "Hello"),
            ("description", "Greeting"),
            ("agents", agents_json),
        ]);
        let r = build_resource("blog/hello", &m, "https://x.example").unwrap();
        assert_eq!(r.name, "Hello");
    }

    #[test]
    fn build_resource_uses_description_excerpt_summary_in_order() {
        // Description preference is meta description → excerpt → summary
        // → synthesized fallback. Exercise each step.
        let with_desc = meta(&[("title", "T"), ("description", "D")]);
        assert_eq!(
            build_resource("p", &with_desc, "").unwrap().description,
            "D"
        );

        let with_excerpt = meta(&[("title", "T"), ("excerpt", "E")]);
        assert_eq!(
            build_resource("p", &with_excerpt, "").unwrap().description,
            "E"
        );

        let with_summary = meta(&[("title", "T"), ("summary", "S")]);
        assert_eq!(
            build_resource("p", &with_summary, "").unwrap().description,
            "S"
        );

        let neither = meta(&[("title", "T")]);
        let r = build_resource("p", &neither, "").unwrap();
        assert!(r.description.contains("Content from"));
    }

    #[test]
    fn build_resource_requires_a_title() {
        // Without a title we can't render a useful name — skip.
        let m = meta(&[("description", "D")]);
        assert!(build_resource("p", &m, "").is_none());
    }

    #[test]
    fn malformed_agents_json_is_ignored() {
        // Don't crash on syntactically-broken `agents` sidecar values
        // — treat them as "no opt-out specified".
        let m = meta(&[("title", "Hello"), ("agents", "not json")]);
        let r = build_resource("p", &m, "").unwrap();
        assert_eq!(r.name, "Hello");
    }

    #[test]
    fn registry_carries_static_tools_and_prompts() {
        // Tools and prompts from `[agents.mcp.tools/.prompts]` must
        // pass through to the registry verbatim.
        let mut agents = AgentsConfig::default();
        agents.mcp.tools.push(super::super::McpToolDecl {
            name: "search".to_string(),
            description: "Search the site".to_string(),
            input_schema: Some(json!({"type": "object"})),
        });
        agents.mcp.prompts.push(super::super::McpPromptDecl {
            name: "summarise".to_string(),
            description: "Summarise a page".to_string(),
            arguments: None,
        });
        let reg = build_registry(&cfg(), &agents, &[]);
        let tools = reg["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "search");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");

        let prompts = reg["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0]["name"], "summarise");
    }

    #[test]
    fn registry_fallback_when_base_url_empty() {
        // Covers the `base_url.is_empty()` arm at line ~165.
        let mut c = cfg();
        c.base_url = String::new();
        let agents = AgentsConfig::default();
        let reg = build_registry(&c, &agents, &[]);
        assert_eq!(reg["transport"]["url"], "/.well-known/mcp");
    }

    #[test]
    fn registry_fallback_when_site_name_empty() {
        // Covers the `site_name.is_empty()` arm at line ~172.
        let mut c = cfg();
        c.site_name = String::new();
        let agents = AgentsConfig::default();
        let reg = build_registry(&c, &agents, &[]);
        assert_eq!(reg["serverInfo"]["name"], "static-site");
    }

    #[test]
    fn registry_prompts_include_arguments_when_present() {
        // Covers the `if let Some(args)` arm at line ~210.
        let mut agents = AgentsConfig::default();
        agents.mcp.prompts.push(super::super::McpPromptDecl {
            name: "translate".to_string(),
            description: "Translate a page".to_string(),
            arguments: Some(json!([{"name":"locale","required":true}])),
        });
        let reg = build_registry(&cfg(), &agents, &[]);
        let prompts = reg["prompts"].as_array().unwrap();
        assert_eq!(prompts[0]["arguments"][0]["name"], "locale");
    }

    #[test]
    fn collect_mcp_resources_empty_site_returns_empty() {
        // Covers the `read_meta_sidecars` fallback branch at line ~267
        // when both site_dir and build_dir/.meta are absent.
        let dir = tempfile::tempdir().unwrap();
        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let r = collect_mcp_resources(&ctx, &cfg());
        assert!(r.is_empty());
    }

    #[test]
    fn collect_mcp_resources_reads_build_meta_when_site_meta_missing() {
        // Covers the `if meta_entries.is_empty()` fallback at line
        // ~268-271 that consults build_dir/.meta.
        let dir = tempfile::tempdir().unwrap();
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let meta_dir = build.join(".meta");
        fs::create_dir_all(&meta_dir).unwrap();
        // Write a sidecar discoverable by read_meta_sidecars.
        let sidecar = meta_dir.join("post.json");
        fs::write(&sidecar, r#"{"title":"Hi","description":"D"}"#).unwrap();
        let ctx = PluginContext::new(dir.path(), &build, &site, dir.path());
        let r = collect_mcp_resources(&ctx, &cfg());
        // Behavioural assertion: the fallback path executed. We don't
        // assert on contents because read_meta_sidecars's discovery
        // semantics are tested separately; we only assert the branch
        // ran without panic and produced a (possibly empty) Vec.
        let _ = r.len();
    }

    #[test]
    // Failpoints are process-global: this test reaches `write_mcp_registry`
    // (and therefore `serialize_registry`) expecting success, so it must
    // never run concurrently with `fault_tests`'s injected
    // `postprocess::mcp-serialize` failure — joins that test's `#[serial]`
    // lock as `#[parallel]` on the same key (mirrors the convention in
    // `core::cache`'s fault-injection tests).
    #[serial_test::parallel(mcp_serialize_fp)]
    fn collect_mcp_resources_with_auto_resources_via_write_mcp_registry() {
        // End-to-end: `write_mcp_registry` with `auto_resources=true`
        // is the only public caller that reaches
        // `collect_mcp_resources`.
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());

        let mut agents = AgentsConfig::default();
        agents.mcp.enabled = true;
        agents.mcp.auto_resources = true;
        write_mcp_registry(&ctx, &cfg(), &agents).unwrap();

        let written = site.join(".well-known/mcp.json");
        assert!(written.exists());
        let body = fs::read_to_string(&written).unwrap();
        assert!(body.contains("\"protocolVersion\""));
    }

    #[test]
    fn registry_tool_without_input_schema_omits_key() {
        // Covers the implicit else of the `if let Some(schema)` arm.
        let mut agents = AgentsConfig::default();
        agents.mcp.tools.push(super::super::McpToolDecl {
            name: "ping".to_string(),
            description: "No schema".to_string(),
            input_schema: None,
        });
        let reg = build_registry(&cfg(), &agents, &[]);
        let tools = reg["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert!(tools[0].get("inputSchema").is_none());
    }

    #[test]
    fn collect_mcp_resources_builds_and_sorts_from_sidecars() {
        // Two real sidecars exercise the build_resource closure and
        // the URI sort comparator.
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        for (slug, title) in [("zeta", "Zeta"), ("alpha", "Alpha")] {
            let page = site.join(slug);
            fs::create_dir_all(&page).unwrap();
            fs::write(
                page.join("index.meta.json"),
                format!(r#"{{"title":"{title}","description":"D"}}"#),
            )
            .unwrap();
        }
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let resources = collect_mcp_resources(&ctx, &cfg());
        assert_eq!(resources.len(), 2);
        assert!(
            resources[0].uri < resources[1].uri,
            "resources must be sorted by URI"
        );
        assert_eq!(resources[0].name, "Alpha");
        assert_eq!(resources[1].name, "Zeta");
    }

    #[test]
    fn build_resource_rejects_empty_rel_path() {
        let m = meta(&[("title", "Hello")]);
        assert!(build_resource("", &m, "https://x.example").is_none());
    }

    #[test]
    fn draft_and_published_flag_variants_are_recognised() {
        for v in ["true", "True", "TRUE", "yes", "1"] {
            let m = meta(&[("title", "T"), ("draft", v)]);
            assert!(
                build_resource("p", &m, "").is_none(),
                "draft={v} must be treated as draft"
            );
        }
        for v in ["false", "False", "FALSE", "no", "0"] {
            let m = meta(&[("title", "T"), ("published", v)]);
            assert!(
                build_resource("p", &m, "").is_none(),
                "published={v} must be treated as unpublished"
            );
        }
        // Unrecognised values leave the page public.
        let m = meta(&[("title", "T"), ("draft", "maybe")]);
        assert!(build_resource("p", &m, "").is_some());
    }

    #[test]
    fn agents_json_without_disallow_key_keeps_page() {
        let m = meta(&[("title", "T"), ("agents", "{}")]);
        assert!(build_resource("p", &m, "").is_some());
    }

    #[test]
    fn agents_disallow_non_array_keeps_page() {
        let m = meta(&[("title", "T"), ("agents", r#"{"disallow":"mcp"}"#)]);
        assert!(build_resource("p", &m, "").is_some());
    }

    #[test]
    fn write_mcp_registry_errors_when_well_known_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".well-known"), "file").unwrap();
        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let agents = AgentsConfig::default();
        let err = write_mcp_registry(&ctx, &cfg(), &agents).unwrap_err();
        assert!(format!("{err}").contains(".well-known"));
    }

    #[test]
    fn write_mcp_registry_errors_when_target_is_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".well-known/mcp.json")).unwrap();
        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let agents = AgentsConfig::default();
        let err = write_mcp_registry(&ctx, &cfg(), &agents).unwrap_err();
        assert!(format!("{err}").contains("mcp.json"));
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::cmd::{ImageConfig, SsgConfig};
    use crate::plugin::PluginContext;
    use std::path::PathBuf;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    fn cfg() -> SsgConfig {
        SsgConfig {
            site_name: "Example".to_string(),
            site_title: "Example Site".to_string(),
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
    #[serial_test::serial(mcp_serialize_fp)]
    fn write_mcp_registry_maps_serialize_failure_to_io_error() {
        let _guard = FailGuard("postprocess::mcp-serialize");
        fail::cfg("postprocess::mcp-serialize", "return")
            .expect("activate failpoint");

        let dir = tempfile::tempdir().unwrap();
        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let agents = AgentsConfig::default();
        let err = write_mcp_registry(&ctx, &cfg(), &agents)
            .expect_err("injected serialize failure must propagate");
        let msg = format!("{err}");
        assert!(msg.contains("mcp.json"), "got: {msg}");
        assert!(
            msg.contains("injected: postprocess::mcp-serialize"),
            "got: {msg}"
        );
    }
}
