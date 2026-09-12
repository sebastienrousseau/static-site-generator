// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for the agentic-discovery emitters (issue #552).
//!
//! Exercises the three plugins end-to-end against AC1-AC6:
//!
//! - AC1: `agents.txt` emitted with site defaults
//! - AC2: per-agent allow/disallow rules render in the canonical order
//! - AC3: `.well-known/ai-plugin.json` is schema-valid
//! - AC4: `.well-known/mcp.json` has the protocol-required shape
//! - AC5: MCP resources are auto-populated from `.meta.json` sidecars
//! - AC6: disabled emitters produce no files
//!
//! AC7 (E5 audit gate 11) lands in a separate branch — this suite
//! verifies the files the gate will consume, not the gate itself.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::needless_pass_by_value
)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::tempdir;

use ssg::cmd::{ImageConfig, SsgConfig};
use ssg::plugin::{Plugin, PluginContext};
use ssg::postprocess::{
    AgentRule, AgenticDiscoveryPlugin, AgentsConfig, McpConfig, McpPromptDecl,
    McpToolDecl,
};

// ── Test helpers ─────────────────────────────────────────────────────

fn base_cfg(agents: Option<AgentsConfig>) -> SsgConfig {
    SsgConfig {
        listings: Vec::new(),
        site_name: "AgenticSite".to_string(),
        site_title: "Agentic Site".to_string(),
        site_description: "Site for agentic-discovery testing".to_string(),
        base_url: "https://agentic.example".to_string(),
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
        edge_headers: ssg::cmd::EdgeHeadersConfig::default(),
        agents,
        transitions: false,
        security: ssg::cmd::SecurityConfig::default(),
        no_taxonomy_pages: false,
    }
}

fn ctx(site_dir: &Path, cfg: SsgConfig) -> PluginContext {
    PluginContext::with_config(site_dir, site_dir, site_dir, site_dir, cfg)
}

fn write_meta(dir: &Path, slug: &str, kv: &[(&str, &str)]) {
    let page_dir = dir.join(slug);
    fs::create_dir_all(&page_dir).expect("create page dir");
    let map: HashMap<String, String> = kv
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let path = page_dir.join("page.meta.json");
    fs::write(&path, serde_json::to_string(&map).unwrap())
        .expect("write meta sidecar");
}

fn run(site_dir: &Path, cfg: SsgConfig) {
    let c = ctx(site_dir, cfg);
    AgenticDiscoveryPlugin
        .after_compile(&c)
        .expect("plugin should succeed");
}

// ── AC1: agents.txt with site defaults ───────────────────────────────

#[test]
fn ac1_agents_txt_emits_default_rule_and_sitemap() {
    let dir = tempdir().unwrap();
    let mut agents = AgentsConfig::default();
    agents.agents_txt = true;
    run(dir.path(), base_cfg(Some(agents)));

    let body = fs::read_to_string(dir.path().join("agents.txt")).unwrap();
    assert!(body.contains("User-agent: *\n"));
    assert!(body.contains("Allow: /\n"));
    assert!(body.contains("Disallow: /private/\n"));
    assert!(body.contains("Sitemap: https://agentic.example/sitemap.xml\n"));
}

#[test]
fn ac1_agents_txt_default_rule_override_replaces_baked_defaults() {
    // When a site author supplies `[agents.default_rule]`, our
    // baked-in `Allow:/ + Disallow:/private/` must NOT also appear.
    let dir = tempdir().unwrap();
    let agents = AgentsConfig {
        agents_txt: true,
        default_rule: Some(AgentRule {
            allow: vec!["/public/*".to_string()],
            disallow: vec!["/admin/*".to_string()],
        }),
        ..AgentsConfig::default()
    };
    run(dir.path(), base_cfg(Some(agents)));

    let body = fs::read_to_string(dir.path().join("agents.txt")).unwrap();
    assert!(body.contains("Allow: /public/*"));
    assert!(body.contains("Disallow: /admin/*"));
    assert!(
        !body.contains("Disallow: /private/"),
        "baked-in default leaked through despite explicit override:\n{body}"
    );
}

// ── AC2: per-agent rules ─────────────────────────────────────────────

#[test]
fn ac2_per_agent_block_appears_with_canonical_casing() {
    let dir = tempdir().unwrap();
    let mut agents = AgentsConfig::default();
    agents.agents_txt = true;
    let _ = agents.rules.insert(
        "gptbot".to_string(),
        AgentRule {
            allow: vec!["/blog/*".to_string()],
            disallow: vec!["/".to_string()],
        },
    );
    run(dir.path(), base_cfg(Some(agents)));

    let body = fs::read_to_string(dir.path().join("agents.txt")).unwrap();
    assert!(
        body.contains("User-agent: GPTBot\n"),
        "expected canonical GPTBot stanza, got:\n{body}"
    );
    // Allow must precede Disallow within the same stanza.
    let allow_pos = body.find("Allow: /blog/*").unwrap();
    let disallow_pos = body.find("Disallow: /\n").unwrap();
    // The first Disallow: /\n is the per-agent one (default rule
    // uses `Disallow: /private/`, not `Disallow: /`).
    assert!(
        allow_pos < disallow_pos,
        "Allow should precede Disallow within a stanza\n{body}"
    );
}

// ── AC3: ai-plugin.json schema validity ──────────────────────────────

#[test]
fn ac3_ai_plugin_json_has_required_keys_and_parses() {
    let dir = tempdir().unwrap();
    let mut agents = AgentsConfig::default();
    agents.ai_plugin = true;
    run(dir.path(), base_cfg(Some(agents)));

    let path = dir.path().join(".well-known/ai-plugin.json");
    assert!(path.exists(), "{} must exist", path.display());

    let body = fs::read_to_string(&path).unwrap();
    let v: Value = serde_json::from_str(&body)
        .expect("ai-plugin.json must be valid JSON (AC3)");

    for key in [
        "schema_version",
        "name_for_human",
        "name_for_model",
        "description_for_human",
        "description_for_model",
        "auth",
        "api",
    ] {
        assert!(v.get(key).is_some(), "missing required key: {key}");
        assert!(!v[key].is_null(), "key {key} is null");
    }
    assert_eq!(v["auth"]["type"], "none");
    assert_eq!(v["api"]["type"], "openapi");
    assert!(v["api"]["url"].as_str().unwrap().contains(".yaml"));
}

#[test]
fn ac3_ai_plugin_name_for_model_is_slug_safe() {
    // The OpenAI spec restricts name_for_model to `[a-z0-9_]`. Pass
    // a name with spaces, punctuation and case mixing — the slug
    // must still satisfy the restriction.
    let dir = tempdir().unwrap();
    let mut cfg = base_cfg(Some(AgentsConfig {
        ai_plugin: true,
        ..AgentsConfig::default()
    }));
    cfg.site_name = "Hello World! 2026 Edition".to_string();
    run(dir.path(), cfg);

    let body =
        fs::read_to_string(dir.path().join(".well-known/ai-plugin.json"))
            .unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    let model_name = v["name_for_model"].as_str().unwrap();
    assert!(
        model_name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "name_for_model must satisfy [a-z0-9_]+, got {model_name:?}"
    );
}

// ── AC4: MCP registry shape ──────────────────────────────────────────

#[test]
fn ac4_mcp_registry_emitted_with_required_shape() {
    let dir = tempdir().unwrap();
    let mut agents = AgentsConfig::default();
    agents.mcp = McpConfig {
        enabled: true,
        ..McpConfig::default()
    };
    run(dir.path(), base_cfg(Some(agents)));

    let path = dir.path().join(".well-known/mcp.json");
    assert!(path.exists(), "{} must exist", path.display());

    let body = fs::read_to_string(&path).unwrap();
    let v: Value =
        serde_json::from_str(&body).expect("mcp.json must be valid JSON");

    assert!(v.get("protocolVersion").is_some());
    assert_eq!(v["transport"]["type"], "http");
    let url = v["transport"]["url"].as_str().unwrap();
    assert!(url.contains("agentic.example"));

    for arr_key in ["resources", "tools", "prompts"] {
        assert!(
            v[arr_key].is_array(),
            "expected {arr_key} to be a JSON array, got {:?}",
            v[arr_key]
        );
    }
}

#[test]
fn ac4_mcp_registry_carries_static_tools_and_prompts() {
    let dir = tempdir().unwrap();
    let mut agents = AgentsConfig::default();
    agents.mcp = McpConfig {
        enabled: true,
        tools: vec![McpToolDecl {
            name: "search".to_string(),
            description: "Search the site".to_string(),
            input_schema: Some(serde_json::json!({"type": "object"})),
        }],
        prompts: vec![McpPromptDecl {
            name: "summarise".to_string(),
            description: "Summarise a page".to_string(),
            arguments: None,
        }],
        ..McpConfig::default()
    };
    run(dir.path(), base_cfg(Some(agents)));

    let body =
        fs::read_to_string(dir.path().join(".well-known/mcp.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    let tools = v["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "search");
    assert_eq!(tools[0]["inputSchema"]["type"], "object");

    let prompts = v["prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0]["name"], "summarise");
}

// ── AC5: MCP resources from content ──────────────────────────────────

#[test]
fn ac5_mcp_resources_auto_populated_from_meta_sidecars() {
    let dir = tempdir().unwrap();

    // Three pages — two public, one draft.
    write_meta(
        dir.path(),
        "blog/hello",
        &[("title", "Hello, World"), ("description", "First post")],
    );
    write_meta(
        dir.path(),
        "about",
        &[("title", "About"), ("description", "Who we are")],
    );
    write_meta(
        dir.path(),
        "drafts/wip",
        &[("title", "Work in progress"), ("draft", "true")],
    );

    let agents = AgentsConfig {
        mcp: McpConfig {
            enabled: true,
            auto_resources: true,
            ..McpConfig::default()
        },
        ..AgentsConfig::default()
    };
    run(dir.path(), base_cfg(Some(agents)));

    let body =
        fs::read_to_string(dir.path().join(".well-known/mcp.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    let resources = v["resources"].as_array().unwrap();

    // Two public pages → two resources. Draft is filtered out.
    assert_eq!(
        resources.len(),
        2,
        "expected 2 resources, got: {resources:#?}"
    );

    let names: Vec<&str> = resources
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert!(names.contains(&"Hello, World"));
    assert!(names.contains(&"About"));
    assert!(!names.contains(&"Work in progress"));

    // Every resource has the four required fields.
    for r in resources {
        assert!(r["uri"].is_string());
        assert!(r["name"].is_string());
        assert!(r["description"].is_string());
        assert_eq!(r["mimeType"], "text/markdown");
    }
}

#[test]
fn ac5_mcp_resources_respect_per_page_disallow() {
    // A page may opt out of MCP via its `agents` sidecar key — the
    // value is a JSON-encoded blob (sidecars are HashMap<String,String>).
    let dir = tempdir().unwrap();
    write_meta(
        dir.path(),
        "public/page",
        &[("title", "Public"), ("description", "Public")],
    );
    write_meta(
        dir.path(),
        "private/page",
        &[
            ("title", "Private"),
            ("description", "Private"),
            ("agents", r#"{"disallow":["mcp"]}"#),
        ],
    );

    let agents = AgentsConfig {
        mcp: McpConfig {
            enabled: true,
            auto_resources: true,
            ..McpConfig::default()
        },
        ..AgentsConfig::default()
    };
    run(dir.path(), base_cfg(Some(agents)));

    let body =
        fs::read_to_string(dir.path().join(".well-known/mcp.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    let resources = v["resources"].as_array().unwrap();
    let names: Vec<&str> = resources
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert!(names.contains(&"Public"));
    assert!(
        !names.contains(&"Private"),
        "per-page disallow:[mcp] should have excluded Private, got {names:?}"
    );
}

#[test]
fn ac5_auto_resources_off_emits_empty_resources_array() {
    // Without `auto_resources = true`, no sidecar walk happens —
    // resources stays empty even when pages exist. Authors who want
    // hand-curated resources can extend the list elsewhere.
    let dir = tempdir().unwrap();
    write_meta(dir.path(), "blog/hi", &[("title", "Hi")]);
    let agents = AgentsConfig {
        mcp: McpConfig {
            enabled: true,
            auto_resources: false,
            ..McpConfig::default()
        },
        ..AgentsConfig::default()
    };
    run(dir.path(), base_cfg(Some(agents)));
    let body =
        fs::read_to_string(dir.path().join(".well-known/mcp.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert!(v["resources"].as_array().unwrap().is_empty());
}

// ── AC6: disabled emitters produce no files ──────────────────────────

#[test]
fn ac6_no_agents_section_produces_no_files() {
    let dir = tempdir().unwrap();
    // No agents config at all on the SsgConfig.
    run(dir.path(), base_cfg(None));
    assert!(!dir.path().join("agents.txt").exists());
    assert!(!dir.path().join(".well-known/ai-plugin.json").exists());
    assert!(!dir.path().join(".well-known/mcp.json").exists());
}

#[test]
fn ac6_all_three_flags_false_produces_no_files() {
    // The section is present but every flag is off.
    let dir = tempdir().unwrap();
    let agents = AgentsConfig::default();
    assert!(!agents.any_enabled());
    run(dir.path(), base_cfg(Some(agents)));
    assert!(!dir.path().join("agents.txt").exists());
    assert!(!dir.path().join(".well-known/ai-plugin.json").exists());
    assert!(!dir.path().join(".well-known/mcp.json").exists());
}

#[test]
fn ac6_only_ai_plugin_enabled_skips_agents_txt_and_mcp() {
    // Each emitter is independently toggleable.
    let dir = tempdir().unwrap();
    let agents = AgentsConfig {
        ai_plugin: true,
        ..AgentsConfig::default()
    };
    run(dir.path(), base_cfg(Some(agents)));
    assert!(
        !dir.path().join("agents.txt").exists(),
        "agents.txt must NOT be emitted when only ai_plugin is true"
    );
    assert!(dir.path().join(".well-known/ai-plugin.json").exists());
    assert!(!dir.path().join(".well-known/mcp.json").exists());
}

// ── Cross-cutting: TOML round-trip on SsgConfig::from_str ────────────

#[test]
fn ssg_toml_with_agents_section_round_trips() {
    // The canonical config in the issue body must parse cleanly when
    // pasted into a full ssg.toml. This guards against future
    // refactors that drop the `agents` field off SsgConfig.
    let toml_str = r#"
        site_name        = "Test"
        content_dir      = "content"
        output_dir       = "public"
        template_dir     = "templates"
        base_url         = "https://example.com"
        site_title       = "Test"
        site_description = "Test"
        language         = "en"

        [agents]
        agents_txt = true
        ai_plugin  = true

        [agents.mcp]
        enabled = true
        transport = "http"
        auto_resources = true

        [agents.rules.gptbot]
        allow    = ["/blog/*"]
        disallow = ["/"]
    "#;
    let cfg: SsgConfig = toml_str.parse().expect("ssg.toml must parse");
    let agents = cfg.agents.expect("agents section must round-trip");
    assert!(agents.agents_txt);
    assert!(agents.ai_plugin);
    assert!(agents.mcp.enabled);
    assert!(agents.mcp.auto_resources);
    assert_eq!(agents.rules.get("gptbot").unwrap().allow, vec!["/blog/*"]);
}
