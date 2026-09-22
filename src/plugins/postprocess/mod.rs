// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Post-processing plugins that fix staticdatagen output.
//!
//! These plugins run in the `after_compile` phase to sanitise XML feeds,
//! sitemaps, manifests, and HTML output produced by the upstream
//! `staticdatagen` crate.
//!
//! - `SitemapFixPlugin` -- Fixes 1, 2, 10: duplicate XML declarations,
//!   double-slash URLs, and per-page lastmod dates.
//! - `NewsSitemapFixPlugin` -- Fix 3: populates news-sitemap entries
//!   from front-matter metadata.
//! - `RssAggregatePlugin` -- Fix 4: aggregates per-page RSS items into
//!   the root feed.
//! - `ManifestFixPlugin` -- Fix 8: word-boundary-safe description
//!   truncation.
//! - `HtmlFixPlugin` -- Fix 9: repairs broken `.class=` image syntax
//!   and Fix 7: upgrades JSON-LD `@context` from `http` to `https`.
//! - `AtomFeedPlugin` -- Generates an Atom 1.0 `atom.xml` feed from
//!   `.meta.json` sidecars.
//! - `JsonFeedPlugin` -- Generates a JSON Feed 1.1 `feed.json` from
//!   `.meta.json` sidecars (alongside the RSS/Atom feeds).
//! - `EdgeHeadersPlugin` -- Emits PQC-aware platform header configs
//!   for Cloudflare Workers, Netlify, and Vercel (issue #550).

pub(crate) mod helpers;

pub mod agentic_discovery;
mod atom;
pub mod edge_headers;
mod html_fix;
mod json_feed;
mod manifest;
mod news_sitemap;
mod rss;
mod sitemap;

pub use agentic_discovery::{
    build_manifest, build_registry, collect_mcp_resources, render_agents_txt,
    write_agents_txt, write_ai_plugin_json, write_mcp_registry, AgentRule,
    AgenticDiscoveryPlugin, AgentsConfig, McpConfig, McpPromptDecl,
    McpResource, McpToolDecl,
};
pub use atom::AtomFeedPlugin;
pub use edge_headers::EdgeHeadersPlugin;
pub use html_fix::HtmlFixPlugin;
pub use json_feed::JsonFeedPlugin;
pub use manifest::ManifestFixPlugin;
pub use news_sitemap::NewsSitemapFixPlugin;
pub use rss::RssAggregatePlugin;
pub use sitemap::SitemapFixPlugin;
