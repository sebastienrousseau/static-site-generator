// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![allow(
    clippy::unwrap_used,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned,
    clippy::expect_used,
    missing_docs,
    unused_results,
    clippy::too_many_lines,
    clippy::cognitive_complexity
)]

//! # Master benchmark harness — every public function (issue #533 follow-up).
//!
//! One criterion `bench_function` per public function across the ssg
//! workspace (root `ssg`, `ssg-core`, `ssg-search`, `ssg-rpc`). Skips:
//!
//! * `ssg-wasm` — wasm-bindgen entry points; can't run native.
//! * `ssg-rpc-macro` — proc-macro crate.
//! * Anything gated on `wasm32` cfg.
//!
//! Smoke-validate with:
//!
//! ```sh
//! cargo bench --bench all_pub_api -- --test
//! ```
//!
//! Each bench follows the `b.iter(|| black_box(f(black_box(input))))`
//! shape so LLVM cannot fold the call away. Inputs are tiny, local
//! fixtures — never pulled from disk except when the function under
//! test demands it, in which case a `tempfile::tempdir()` is built
//! once outside `b.iter` and shared.
//!
//! Groups (one `criterion_group!` each — see the bottom of the file):
//!
//! * `group_audit_gates`        — every gate in `src/audit/gates/`
//! * `group_audit_output`       — JSON, JUnit, text formatters
//! * `group_audit_runner`       — `AuditRunner` lifecycle
//! * `group_cmd`                — CLI helpers in `src/cmd/`
//! * `group_core`               — `src/core/` pure helpers
//! * `group_plugins_seo`        — `src/plugins/seo/`
//! * `group_plugins_jsonld_iso20022` — ISO 20022 validators + builders
//! * `group_plugins_postprocess_agentic` — agents.txt / mcp / ai-plugin
//! * `group_plugins_postprocess_edge_headers` — Cloudflare / Netlify / Vercel
//! * `group_plugins_view_transitions`
//! * `group_plugins_misc`       — everything else under `src/plugins/`
//! * `group_plugins_agent_surfaces` — `agent_api` / `oembed` / vector
//!   search / taxonomy (v0.0.47 agent-facing surfaces)
//! * `group_util`               — `src/util/head_dom.rs`, `html_rewriter.rs`
//! * `group_server`             — `src/server/`
//! * `group_ssg_core`           — `crates/ssg-core`
//! * `group_ssg_search`         — `crates/ssg-search`
//! * `group_ssg_rpc`            — `crates/ssg-rpc`
//! * `group_lib_root`           — top-level `ssg::*` (Paths, now_iso, …)

use std::collections::BTreeMap;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::TempDir;

// ====================================================================
// Shared fixtures
// ====================================================================

/// Minimal HTML document used by `Site`-walking gates and by every
/// HTML-rewriting public function. Carries enough head metadata for
/// SEO / CSP / OG / Twitter / hreflang / JSON-LD gates to do real work.
const SAMPLE_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Sample Page</title>
    <meta name="description" content="A sample page used for benchmarks.">
    <meta property="og:title" content="Sample">
    <meta property="og:type" content="website">
    <meta property="og:image" content="/og.png">
    <meta name="twitter:card" content="summary">
    <link rel="canonical" href="https://example.com/">
    <link rel="alternate" hreflang="en" href="https://example.com/">
    <meta http-equiv="Content-Security-Policy" content="default-src 'self'">
    <script type="application/ld+json">
      {"@context":"https://schema.org","@type":"WebPage","name":"Sample"}
    </script>
  </head>
  <body>
    <main>
      <h1>Sample heading</h1>
      <p>Some <strong>HTML</strong> content with &amp; entities &lt;like&gt; this.</p>
      <a href="/about/">about</a>
      <img src="/img.jpg" alt="placeholder" width="10" height="10">
    </main>
  </body>
</html>"#;

/// Markdown body fed into compile_markdown / compile_page / etc.
const SAMPLE_MARKDOWN: &str = "# Hello\n\nA paragraph with *emphasis*.\n\n\
                               | A | B |\n|---|---|\n| 1 | 2 |\n\n\
                               - [x] done\n- [ ] todo\n";

/// Builds an on-disk site directory containing one canonical HTML file
/// plus companion `img.jpg` / `a.webp` so the audit gates have real
/// targets to inspect. Returns the `TempDir` (keep alive for the
/// lifetime of the borrow) and the populated `Site`.
fn build_site_fixture() -> (TempDir, ssg::audit::Site) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("index.html"), SAMPLE_HTML).unwrap();
    std::fs::write(root.join("img.jpg"), vec![0u8; 64]).unwrap();
    std::fs::write(root.join("a.webp"), vec![0u8; 64]).unwrap();
    let site = ssg::audit::Site::load(root).expect("Site::load");
    (tmp, site)
}

/// Build a `PluginContext` rooted in a tempdir for plugin benches that
/// need one. The build/site/template/content dirs all alias the tempdir
/// itself, which is enough for any plugin to short-circuit on the
/// "dir empty" path while still running its public constructors.
fn build_plugin_ctx() -> (TempDir, ssg::plugin::PluginContext) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let p = tmp.path();
    let ctx = ssg::plugin::PluginContext::new(p, p, p, p);
    (tmp, ctx)
}

// ====================================================================
// group_audit_gates — one bench per gate's `.run(&site, &opts)`
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_audit_gates(c: &mut Criterion) {
    use ssg::audit::gates::{
        ai_discovery::AiDiscoveryGate, broken_links::BrokenLinksGate,
        csp_sri::CspSriGate, feeds::FeedsGate, hreflang::HreflangGate,
        html5::Html5Gate, images::ImagesGate, jsonld::JsonLdGate,
        lang_consistency::LangConsistencyGate, markdownlint::MarkdownlintGate,
        metadata::MetadataGate, performance::PerformanceGate,
        pqc_tls::PqcTlsGate, search_index::SearchIndexGate, wcag::WcagGate,
    };
    use ssg::audit::{AuditGate, AuditOptions};

    let (_tmp, site) = build_site_fixture();
    let opts = AuditOptions::default();

    macro_rules! gate {
        ($name:literal, $g:expr) => {
            c.bench_function($name, |b| {
                let g = $g;
                b.iter(|| black_box(g.run(black_box(&site), black_box(&opts))));
            });
        };
    }

    gate!("gates::ai_discovery", AiDiscoveryGate);
    gate!("gates::broken_links", BrokenLinksGate);
    gate!("gates::csp_sri", CspSriGate);
    gate!("gates::feeds", FeedsGate);
    gate!("gates::hreflang", HreflangGate);
    gate!("gates::html5", Html5Gate);
    gate!("gates::images", ImagesGate);
    gate!("gates::jsonld", JsonLdGate);
    gate!("gates::lang_consistency", LangConsistencyGate);
    gate!("gates::markdownlint", MarkdownlintGate);
    gate!("gates::metadata", MetadataGate);
    gate!("gates::performance", PerformanceGate);
    gate!("gates::pqc_tls", PqcTlsGate);
    gate!("gates::search_index", SearchIndexGate);
    gate!("gates::wcag", WcagGate);

    // util::find_tag_end / hreflang_attr — pure-string helpers.
    use ssg::audit::gates::util::{find_tag_end, hreflang_attr};
    c.bench_function("gates::util::find_tag_end", |b| {
        b.iter(|| black_box(find_tag_end(black_box(SAMPLE_HTML), 0)));
    });
    c.bench_function("gates::util::hreflang_attr", |b| {
        b.iter(|| {
            black_box(hreflang_attr(
                black_box(
                    "<link rel=\"alternate\" hreflang=\"en\" href=\"x\">",
                ),
                black_box("hreflang"),
            ))
        });
    });

    // The `all()` constructor — builds the full Vec<Box<dyn AuditGate>>.
    c.bench_function("gates::all", |b| {
        b.iter(|| black_box(ssg::audit::gates::all()));
    });
}

// ====================================================================
// group_audit_output — json / junit / text formatters
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_audit_output(c: &mut Criterion) {
    use ssg::audit::output;
    use ssg::audit::{
        AuditConfig, AuditReport, AuditRunner, Severity, SeverityCounts,
    };

    let (_tmp, site) = build_site_fixture();
    let runner = AuditRunner::new(AuditConfig::new());
    let report: AuditReport = runner.run(&site);

    c.bench_function("audit::output::json::format", |b| {
        b.iter(|| {
            let _ = black_box(output::json::format(black_box(&report)));
        });
    });
    c.bench_function("audit::output::junit::format", |b| {
        b.iter(|| black_box(output::junit::format(black_box(&report))));
    });
    c.bench_function("audit::output::text::format", |b| {
        b.iter(|| {
            let mut s = String::new();
            output::text::format(black_box(&report), &mut s);
            black_box(s)
        });
    });

    // Sanity-bench the `Severity` / `SeverityCounts` public methods so
    // their inlining boundaries are observable here too.
    c.bench_function("audit::Severity::as_str", |b| {
        b.iter(|| black_box(Severity::Warn.as_str()));
    });
    c.bench_function("audit::Severity::parse", |b| {
        b.iter(|| black_box(Severity::parse(black_box("warn"))));
    });
    c.bench_function("audit::SeverityCounts::add_and_total", |b| {
        b.iter(|| {
            let mut counts = SeverityCounts::default();
            counts.add(Severity::Info);
            counts.add(Severity::Warn);
            counts.add(Severity::Error);
            black_box(counts.total())
        });
    });
}

// ====================================================================
// group_audit_runner — AuditRunner / AuditConfig / AuditReport
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_audit_runner(c: &mut Criterion) {
    use ssg::audit::{
        AuditBudgets, AuditConfig, AuditDisabledSection, AuditOptions,
        AuditReport, AuditRunner, AuditTomlConfig, Finding, Severity, Site,
    };

    let (_tmp, site) = build_site_fixture();

    c.bench_function("audit::AuditConfig::new", |b| {
        b.iter(|| black_box(AuditConfig::new()));
    });
    c.bench_function("audit::AuditConfig::default", |b| {
        b.iter(|| black_box(AuditConfig::default()));
    });
    c.bench_function("audit::AuditOptions::default", |b| {
        b.iter(|| black_box(AuditOptions::default()));
    });
    c.bench_function("audit::AuditBudgets::default", |b| {
        b.iter(|| black_box(AuditBudgets::default()));
    });
    c.bench_function("audit::AuditDisabledSection::default", |b| {
        b.iter(|| black_box(AuditDisabledSection::default()));
    });
    c.bench_function("audit::AuditTomlConfig::default", |b| {
        b.iter(|| black_box(AuditTomlConfig::default()));
    });
    c.bench_function("audit::AuditTomlConfig::into_audit_config", |b| {
        b.iter(|| black_box(AuditTomlConfig::default().into_audit_config()));
    });
    c.bench_function("audit::AuditRunner::new", |b| {
        b.iter(|| black_box(AuditRunner::new(AuditConfig::new())));
    });
    let runner = AuditRunner::new(AuditConfig::new());
    c.bench_function("audit::AuditRunner::gate_names", |b| {
        b.iter(|| black_box(runner.gate_names()));
    });
    c.bench_function("audit::AuditRunner::fail_on", |b| {
        b.iter(|| black_box(runner.fail_on()));
    });
    c.bench_function("audit::AuditRunner::run", |b| {
        b.iter(|| black_box(runner.run(black_box(&site))));
    });
    let report: AuditReport = runner.run(&site);
    c.bench_function("audit::AuditReport::max_severity", |b| {
        b.iter(|| black_box(report.max_severity()));
    });
    c.bench_function("audit::AuditReport::should_fail", |b| {
        b.iter(|| black_box(report.should_fail(Severity::Error)));
    });
    c.bench_function("audit::AuditReport::len", |b| {
        b.iter(|| black_box(report.len()));
    });
    c.bench_function("audit::AuditReport::is_empty", |b| {
        b.iter(|| black_box(report.is_empty()));
    });
    // SKIPPED: audit::AuditReport::print_text  — writes to stdout
    // SKIPPED: audit::AuditReport::print_json  — writes to stdout
    // SKIPPED: audit::AuditReport::print_junit — writes to stdout

    // Site + Finding builders.
    let (tmp, _site2) = build_site_fixture();
    let root = tmp.path().to_path_buf();
    c.bench_function("audit::Site::load", |b| {
        b.iter(|| black_box(Site::load(black_box(&root))));
    });
    let site2 = Site::load(&root).unwrap();
    c.bench_function("audit::Site::rel", |b| {
        b.iter(|| black_box(site2.rel(black_box(&root.join("index.html")))));
    });
    c.bench_function("audit::Site::read", |b| {
        b.iter(|| {
            let _ = black_box(site2.read(black_box(&root.join("index.html"))));
        });
    });

    c.bench_function("audit::Finding::new+with_code+with_path", |b| {
        b.iter(|| {
            black_box(
                Finding::new("g", Severity::Warn, "msg")
                    .with_code("CODE1")
                    .with_path("a/b.html"),
            )
        });
    });
}

// ====================================================================
// group_cmd — CLI / config / validation helpers
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_cmd(c: &mut Criterion) {
    use ssg::cmd::{
        default_config, resolve_host, resolve_port, Cli, SsgConfig,
    };
    use ssg::process;

    c.bench_function("cmd::resolve_host", |b| {
        b.iter(|| black_box(resolve_host()));
    });
    c.bench_function("cmd::resolve_port", |b| {
        b.iter(|| black_box(resolve_port()));
    });
    c.bench_function("cmd::default_config", |b| {
        b.iter(|| black_box(default_config()));
    });
    c.bench_function("cmd::Cli::build", |b| {
        b.iter(|| black_box(Cli::build()));
    });
    c.bench_function("cmd::Cli::subcommand_app", |b| {
        b.iter(|| black_box(Cli::subcommand_app()));
    });
    // SKIPPED: cmd::Cli::print_banner — writes to stdout
    // SKIPPED: cmd::Cli::parse_and_dispatch — covered indirectly via Cli::build

    c.bench_function("cmd::SsgConfig::builder+build", |b| {
        b.iter(|| {
            black_box(
                SsgConfig::builder()
                    .site_name("bench".into())
                    .base_url("https://example.test/".into())
                    .content_dir(PathBuf::from("content"))
                    .output_dir(PathBuf::from("public"))
                    .template_dir(PathBuf::from("templates"))
                    .serve_dir(None)
                    .site_title("t".into())
                    .site_description("d".into())
                    .language("en-GB".into())
                    .i18n(None)
                    .cdn_prefix(None)
                    .transitions(false)
                    .build(),
            )
        });
    });
    let cfg = SsgConfig::default();
    c.bench_function("cmd::SsgConfig::validate", |b| {
        b.iter(|| {
            let _ = black_box(cfg.validate());
        });
    });
    c.bench_function("cmd::SsgConfig::default", |b| {
        b.iter(|| black_box(SsgConfig::default()));
    });

    // EdgeHeadersConfig::is_enabled is a public method.
    let edge = ssg::cmd::EdgeHeadersConfig::default();
    c.bench_function("cmd::EdgeHeadersConfig::is_enabled", |b| {
        b.iter(|| black_box(edge.is_enabled()));
    });

    // SriAlgorithm (v0.0.47, `[security]` config) — prefix token and
    // full SRI `integrity=` digest for all three variants.
    use ssg::cmd::SriAlgorithm;
    c.bench_function("cmd::SriAlgorithm::prefix (all variants)", |b| {
        b.iter(|| {
            black_box(SriAlgorithm::Sha256.prefix());
            black_box(SriAlgorithm::Sha384.prefix());
            black_box(SriAlgorithm::Sha512.prefix())
        });
    });
    let sri_payload: &[u8] = b"console.log('bench');";
    c.bench_function("cmd::SriAlgorithm::integrity (sha256)", |b| {
        b.iter(|| {
            black_box(SriAlgorithm::Sha256.integrity(black_box(sri_payload)))
        });
    });
    c.bench_function("cmd::SriAlgorithm::integrity (sha384)", |b| {
        b.iter(|| {
            black_box(SriAlgorithm::Sha384.integrity(black_box(sri_payload)))
        });
    });
    c.bench_function("cmd::SriAlgorithm::integrity (sha512)", |b| {
        b.iter(|| {
            black_box(SriAlgorithm::Sha512.integrity(black_box(sri_payload)))
        });
    });

    // SecurityConfig lands via the `[security]` block of `ssg.toml`,
    // deserialized through SsgConfig's public `FromStr` impl.
    const SECURITY_TOML: &str = r#"
site_name = "bench"
content_dir = "content"
output_dir = "public"
template_dir = "templates"
base_url = "https://example.com"
site_title = "t"
site_description = "d"
language = "en-GB"

[security]
sri_algorithm = "sha512"
"#;
    c.bench_function("cmd::SsgConfig::from_str ([security] block)", |b| {
        b.iter(|| {
            let cfg = black_box(SECURITY_TOML).parse::<SsgConfig>();
            black_box(cfg.map(|c| c.security.sri_algorithm))
        });
    });

    // cmd::validation public free functions (re-exported from cmd::).
    c.bench_function("cmd::validation::is_valid_url", |b| {
        b.iter(|| {
            black_box(ssg::cmd::is_valid_url(black_box("https://example.com/")))
        });
    });
    c.bench_function("cmd::validation::validate_url", |b| {
        b.iter(|| {
            let _ = black_box(ssg::cmd::validate_url(black_box(
                "https://example.com/",
            )));
        });
    });

    // cmd::audit::build_subcommand — clap::Command builder.
    c.bench_function("cmd::audit::build_subcommand", |b| {
        b.iter(|| black_box(ssg::cmd::audit::build_subcommand()));
    });
    // SKIPPED: cmd::audit::run — touches the filesystem and exits.
    // SKIPPED: cmd::audit::run_and_dispatch — same as above.

    // process::args needs a real ArgMatches; we get one from `Cli::build`.
    let matches = Cli::build().get_matches_from(["ssg"]);
    c.bench_function("core::process::args", |b| {
        b.iter(|| {
            let _ = black_box(process::args(black_box(&matches)));
        });
    });
    // SKIPPED: core::process::get_argument — needs structured matches input
    // SKIPPED: core::process::ensure_directory — creates real dirs; bench
    //          path is covered via fs_ops where the tempdir is reused.
}

// ====================================================================
// group_core — pure helpers in src/core/
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_core(c: &mut Criterion) {
    use ssg::cache::BuildCache;
    use ssg::depgraph::DepGraph;
    use ssg::deploy_adapter::{adapter_for, Target};
    use ssg::frontmatter::emit_sidecars;
    use ssg::fs_ops::{
        copy_dir_all, is_safe_path, verify_and_copy_files, verify_file_safety,
    };
    use ssg::logging::create_log_file;
    use ssg::scaffold::scaffold_project_at;
    use ssg::schema::{generate_schema, write_schema};
    use ssg::stream;
    use ssg::walk::{
        walk_files, walk_files_bounded_count, walk_files_bounded_depth,
        walk_files_multi,
    };

    // BuildCache lifecycle.
    let tmp = tempfile::tempdir().unwrap();
    let cache_path = tmp.path().join(".ssg-cache.json");
    c.bench_function("cache::BuildCache::new", |b| {
        b.iter(|| black_box(BuildCache::new(black_box(&cache_path))));
    });
    let cache = BuildCache::new(&cache_path);
    c.bench_function("cache::BuildCache::len", |b| {
        b.iter(|| black_box(cache.len()));
    });
    c.bench_function("cache::BuildCache::is_empty", |b| {
        b.iter(|| black_box(cache.is_empty()));
    });
    c.bench_function("cache::BuildCache::default_path", |b| {
        b.iter(|| black_box(BuildCache::default_path()));
    });
    c.bench_function("cache::BuildCache::load (missing)", |b| {
        b.iter(|| {
            let _ = black_box(BuildCache::load(black_box(&cache_path)));
        });
    });
    // SKIPPED: cache::BuildCache::save — would write to disk every iter
    // SKIPPED: cache::BuildCache::changed_files / update — touches FS
    //          per-iter; covered by benches/core/cache.rs already.

    // DepGraph public surface.
    let dg_root = tempfile::tempdir().unwrap();
    let mut dg = DepGraph::new();
    c.bench_function("depgraph::DepGraph::new", |b| {
        b.iter(|| black_box(DepGraph::new()));
    });
    c.bench_function("depgraph::DepGraph::load (empty)", |b| {
        b.iter(|| black_box(DepGraph::load(black_box(dg_root.path()))));
    });
    let consumer = PathBuf::from("a.html");
    let dep = PathBuf::from("a.md");
    c.bench_function("depgraph::DepGraph::add_dep", |b| {
        b.iter(|| {
            dg.add_dep(black_box(&consumer), black_box(&dep));
        });
    });
    c.bench_function("depgraph::DepGraph::add_output", |b| {
        b.iter(|| {
            dg.add_output(black_box(&dep), black_box(&consumer));
        });
    });
    c.bench_function("depgraph::DepGraph::record_hash", |b| {
        b.iter(|| {
            dg.record_hash(black_box(&dep), black_box(b"hello"));
        });
    });
    c.bench_function("depgraph::DepGraph::sha256_hex", |b| {
        b.iter(|| black_box(DepGraph::sha256_hex(black_box(b"hello"))));
    });
    c.bench_function("depgraph::DepGraph::deps_for", |b| {
        b.iter(|| black_box(dg.deps_for(black_box(&consumer))));
    });
    c.bench_function("depgraph::DepGraph::outputs_for", |b| {
        b.iter(|| black_box(dg.outputs_for(black_box(&dep))));
    });
    c.bench_function("depgraph::DepGraph::tracked_sources", |b| {
        b.iter(|| black_box(dg.tracked_sources()));
    });
    c.bench_function("depgraph::DepGraph::page_count", |b| {
        b.iter(|| black_box(dg.page_count()));
    });
    // SKIPPED: depgraph::DepGraph::is_empty — `is_empty` is on
    //          `depgraph::Diff`, not the graph itself.
    let changed = vec![dep.clone()];
    c.bench_function("depgraph::DepGraph::invalidated", |b| {
        b.iter(|| black_box(dg.invalidated(black_box(&changed))));
    });
    c.bench_function("depgraph::DepGraph::invalidated_outputs", |b| {
        b.iter(|| black_box(dg.invalidated_outputs(black_box(&changed))));
    });
    c.bench_function("depgraph::DepGraph::diff", |b| {
        b.iter(|| {
            let m: std::collections::HashMap<PathBuf, String> =
                std::collections::HashMap::new();
            black_box(dg.diff(black_box(&m)))
        });
    });
    // SKIPPED: depgraph::DepGraph::save — touches disk
    // SKIPPED: depgraph::DepGraph::record_hash_from_disk — reads file
    // SKIPPED: depgraph::DepGraph::forget / clear — mutating-only, skewy

    // Deploy adapter selectors.
    c.bench_function("deploy_adapter::Target::from_cli", |b| {
        b.iter(|| black_box(Target::from_cli(black_box("none"))));
    });
    c.bench_function("deploy_adapter::Target::as_str", |b| {
        b.iter(|| black_box(Target::None.as_str()));
    });
    c.bench_function("deploy_adapter::adapter_for", |b| {
        b.iter(|| black_box(adapter_for(Target::None)));
    });

    // schema.
    c.bench_function("schema::generate_schema", |b| {
        b.iter(|| black_box(generate_schema()));
    });
    let schema_out = tmp.path().join("schema.json");
    c.bench_function("schema::write_schema", |b| {
        b.iter(|| {
            let _ = black_box(write_schema(black_box(&schema_out)));
        });
    });

    // walk.
    let walk_dir = tempfile::tempdir().unwrap();
    std::fs::write(walk_dir.path().join("a.html"), "").unwrap();
    std::fs::write(walk_dir.path().join("b.html"), "").unwrap();
    c.bench_function("walk::walk_files", |b| {
        b.iter(|| {
            let _ = black_box(walk_files(
                black_box(walk_dir.path()),
                black_box("html"),
            ));
        });
    });
    c.bench_function("walk::walk_files_multi", |b| {
        b.iter(|| {
            let _ = black_box(walk_files_multi(
                black_box(walk_dir.path()),
                black_box(&["html", "md"]),
            ));
        });
    });
    c.bench_function("walk::walk_files_bounded_depth", |b| {
        b.iter(|| {
            let _ = black_box(walk_files_bounded_depth(
                black_box(walk_dir.path()),
                black_box("html"),
                black_box(2),
            ));
        });
    });
    c.bench_function("walk::walk_files_bounded_count", |b| {
        b.iter(|| {
            let _ = black_box(walk_files_bounded_count(
                black_box(walk_dir.path()),
                black_box("html"),
                black_box(10),
            ));
        });
    });

    // fs_ops — pure-string predicates + small fixture copies.
    let safe_path = PathBuf::from("safe");
    c.bench_function("fs_ops::is_safe_path", |b| {
        b.iter(|| {
            let _ = black_box(is_safe_path(black_box(&safe_path)));
        });
    });
    let real_path = walk_dir.path().join("a.html");
    c.bench_function("fs_ops::verify_file_safety", |b| {
        b.iter(|| {
            let _ = black_box(verify_file_safety(black_box(&real_path)));
        });
    });
    let copy_src = tempfile::tempdir().unwrap();
    std::fs::write(copy_src.path().join("a.txt"), "hi").unwrap();
    c.bench_function("fs_ops::verify_and_copy_files (one file)", |b| {
        b.iter(|| {
            let dst = tempfile::tempdir().unwrap();
            let _ = black_box(verify_and_copy_files(
                black_box(copy_src.path()),
                black_box(dst.path()),
            ));
        });
    });
    c.bench_function("fs_ops::copy_dir_all (small)", |b| {
        b.iter(|| {
            let dst = tempfile::tempdir().unwrap();
            let _ = black_box(copy_dir_all(
                black_box(copy_src.path()),
                black_box(dst.path()),
            ));
        });
    });
    // SKIPPED: fs_ops::verify_and_copy_files_async — same shape, just async.
    // SKIPPED: fs_ops::copy_dir_with_progress — emits progress to stdout.
    // SKIPPED: fs_ops::collect_files_recursive — covered by walk_files.
    // SKIPPED: fs_ops::copy_dir_all_async — same shape as copy_dir_all.

    // frontmatter::emit_sidecars — small fixture.
    let fm_src = tempfile::tempdir().unwrap();
    std::fs::write(fm_src.path().join("a.md"), "---\ntitle: A\n---\nbody")
        .unwrap();
    let fm_dst = tempfile::tempdir().unwrap();
    c.bench_function("frontmatter::emit_sidecars", |b| {
        b.iter(|| {
            let _ = black_box(emit_sidecars(
                black_box(fm_src.path()),
                black_box(fm_dst.path()),
            ));
        });
    });
    // SKIPPED: frontmatter::read_sidecar — needs the matching .meta.json
    //          produced by emit_sidecars; covered indirectly above.
    // SKIPPED: frontmatter::read_sidecar_for_html — same.

    // logging::create_log_file (also writes to disk — small fixture).
    let log_path = tmp.path().join("bench.log");
    let log_str = log_path.to_string_lossy().to_string();
    c.bench_function("logging::create_log_file", |b| {
        b.iter(|| {
            let _ = black_box(create_log_file(black_box(&log_str)));
        });
    });
    // SKIPPED: logging::log_initialization / log_arguments — write to
    //          stdout/file and configure the global logger.

    // streaming + stream public helpers.
    c.bench_function("core::streaming::MemoryBudget::from_mb", |b| {
        b.iter(|| {
            black_box(ssg::streaming::MemoryBudget::from_mb(black_box(64)))
        });
    });
    c.bench_function("core::streaming::MemoryBudget::default_budget", |b| {
        b.iter(|| black_box(ssg::streaming::MemoryBudget::default_budget()));
    });
    // SKIPPED: core::streaming::batched_content_files / compile_batch /
    //          should_stream — drive the full pipeline; benched in
    //          benches/bench_site_generation.rs.

    let stream_src = tempfile::tempdir().unwrap();
    let stream_in = stream_src.path().join("in.txt");
    std::fs::write(&stream_in, b"hello").unwrap();
    c.bench_function("stream::stream_copy", |b| {
        b.iter(|| {
            let dst = stream_src.path().join("out.txt");
            let _ = black_box(stream::stream_copy(
                black_box(&stream_in),
                black_box(&dst),
            ));
        });
    });
    c.bench_function("stream::stream_hash", |b| {
        b.iter(|| {
            let _ = black_box(stream::stream_hash(black_box(&stream_in)));
        });
    });
    c.bench_function("stream::stream_lines", |b| {
        b.iter(|| {
            let _ = black_box(stream::stream_lines(
                black_box(&stream_in),
                |_n, _line| Ok(()),
            ));
        });
    });
    // SKIPPED: stream::benchmark_throughput — feature-gated on `benchmark`.
    let pb_src = tempfile::tempdir().unwrap();
    std::fs::write(pb_src.path().join("a.txt"), "x").unwrap();
    c.bench_function("stream::process_batch", |b| {
        b.iter(|| {
            let dst = tempfile::tempdir().unwrap();
            let _ = black_box(stream::process_batch(
                black_box(pb_src.path()),
                black_box(dst.path()),
                |_s, _d| Ok(0u64),
            ));
        });
    });

    // scaffold::scaffold_project_at — writes a small dir tree.
    c.bench_function("scaffold::scaffold_project_at", |b| {
        b.iter(|| {
            let dst = tempfile::tempdir().unwrap();
            let _ = black_box(scaffold_project_at(
                black_box("bench-site"),
                black_box(dst.path()),
            ));
        });
    });
    // SKIPPED: scaffold::scaffold_project — uses cwd; would litter the workdir.

    // collections::get_collection / get_entry — need a populated dir.
    let coll_root = tempfile::tempdir().unwrap();
    let coll_dir = coll_root.path().join("posts");
    std::fs::create_dir_all(&coll_dir).unwrap();
    std::fs::write(coll_dir.join("a.md"), "---\ntitle: A\n---\nbody").unwrap();
    c.bench_function("collections::get_collection", |b| {
        b.iter(|| {
            let _ = black_box(ssg::collections::get_collection::<
                serde_json::Value,
            >(black_box(&coll_dir)));
        });
    });
    c.bench_function("collections::get_entry", |b| {
        b.iter(|| {
            let _ =
                black_box(ssg::collections::get_entry::<serde_json::Value>(
                    black_box(&coll_dir),
                    black_box("a"),
                ));
        });
    });

    // content::validate_*  — pure-string when the schema is empty.
    c.bench_function("content::parse_schemas", |b| {
        b.iter(|| {
            let _ = black_box(ssg::content::parse_schemas(black_box("")));
        });
    });
    let empty_schemas: Vec<ssg::content::ContentSchema> = Vec::new();
    let sample_schema = ssg::content::ContentSchema {
        name: "post".into(),
        fields: Vec::new(),
    };
    let fm_path = coll_root.path().join("a.md");
    c.bench_function("content::validate_frontmatter", |b| {
        b.iter(|| {
            let map: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let _ = black_box(ssg::content::validate_frontmatter(
                black_box(&map),
                black_box(&sample_schema),
                black_box(&fm_path),
                black_box(0),
            ));
        });
    });
    c.bench_function("content::load_schemas (missing)", |b| {
        b.iter(|| {
            let _ = black_box(ssg::content::load_schemas(black_box(
                Path::new("nope.toml"),
            )));
        });
    });
    c.bench_function("content::validate_only (empty dir)", |b| {
        b.iter(|| {
            let _ = black_box(ssg::content::validate_only(black_box(
                coll_root.path(),
            )));
        });
    });
    c.bench_function("content::validate_content_dir", |b| {
        b.iter(|| {
            let _ = black_box(ssg::content::validate_content_dir(
                black_box(coll_root.path()),
                black_box(&empty_schemas),
            ));
        });
    });
    c.bench_function("content::validate_with_schema", |b| {
        b.iter(|| {
            let _ = black_box(ssg::content::validate_with_schema(
                black_box(coll_root.path()),
                black_box(Path::new("does-not-exist.toml")),
            ));
        });
    });

    // otel::init_if_enabled with `false` is a no-op (won't install global
    // subscriber).
    c.bench_function("otel::init_if_enabled (off)", |b| {
        b.iter(|| black_box(ssg::otel::init_if_enabled(black_box(false))));
    });

    // pipeline public surfaces — most need a real config + flag matches.
    c.bench_function("pipeline::clear_error_message", |b| {
        b.iter(|| black_box(ssg::pipeline::clear_error_message()));
    });
    let pipeline_cfg = ssg::cmd::SsgConfig::default();
    c.bench_function("pipeline::resolve_build_and_site_dirs", |b| {
        b.iter(|| {
            black_box(ssg::pipeline::resolve_build_and_site_dirs(black_box(
                &pipeline_cfg,
            )))
        });
    });
    let pipeline_root = tempfile::tempdir().unwrap();
    c.bench_function("pipeline::depgraph_cache_root", |b| {
        b.iter(|| {
            black_box(ssg::pipeline::depgraph_cache_root(black_box(
                pipeline_root.path(),
            )))
        });
    });
    // SKIPPED: pipeline::build_pipeline / register_isr_plugins /
    //          execute_build_pipeline / execute_build_pipeline_with /
    //          compile_site / register_default_plugins / ErrorMessage::* /
    //          RunOptions::* — drive the full build pipeline; benched
    //          end-to-end in benches/bench_site_generation.rs.
    // SKIPPED: pipeline::compile_site_with_base_url (v0.0.47) — same
    //          shape as compile_site (which it now backs); there is no
    //          compile_site precedent in this harness to mirror, and
    //          both drive the full staticdatagen build. Covered
    //          end-to-end in benches/bench_site_generation.rs.

    // TemplateEngine — feature-gated on `templates`.
    #[cfg(feature = "templates")]
    {
        use ssg::template_engine::{TemplateConfig, TemplateEngine};
        c.bench_function("template_engine::TemplateConfig::default", |b| {
            b.iter(|| black_box(TemplateConfig::default()));
        });
        let tmpl_dir = tempfile::tempdir().unwrap();
        let tconf = TemplateConfig {
            template_dir: tmpl_dir.path().to_path_buf(),
            globals: std::collections::HashMap::new(),
            autoescape: true,
        };
        c.bench_function("template_engine::TemplateEngine::init", |b| {
            b.iter(|| {
                let _ = black_box(TemplateEngine::init(tconf.clone()));
            });
        });
        c.bench_function("template_engine::site_globals_from_config", |b| {
            b.iter(|| {
                black_box(TemplateEngine::site_globals_from_config(black_box(
                    &pipeline_cfg,
                )))
            });
        });
        c.bench_function("template_engine::load_data_files (missing)", |b| {
            b.iter(|| {
                let _ = black_box(TemplateEngine::load_data_files(black_box(
                    Path::new("does-not-exist"),
                )));
            });
        });
        // SKIPPED: template_engine::TemplateEngine::render_page — needs a
        //          real template file on disk; covered by integration tests.
    }

    // dates (v0.0.47) — flexible date parsing + the four formatters.
    use ssg::dates::{days_in_month, is_leap_year, parse_flexible_date};
    c.bench_function("dates::parse_flexible_date (rfc2822)", |b| {
        b.iter(|| {
            black_box(parse_flexible_date(black_box(
                "Wed, 01 Jul 2026 07:07:07 +0000",
            )))
        });
    });
    c.bench_function("dates::parse_flexible_date (long form)", |b| {
        b.iter(|| black_box(parse_flexible_date(black_box("July 1, 2026"))));
    });
    c.bench_function("dates::parse_flexible_date (iso8601)", |b| {
        b.iter(|| {
            black_box(parse_flexible_date(black_box("2026-07-01T07:07:07Z")))
        });
    });
    let flex = parse_flexible_date("2026-07-01T07:07:07Z").unwrap();
    c.bench_function("dates::FlexibleDate::to_rfc2822", |b| {
        b.iter(|| black_box(flex.to_rfc2822()));
    });
    c.bench_function("dates::FlexibleDate::to_rfc3339", |b| {
        b.iter(|| black_box(flex.to_rfc3339()));
    });
    c.bench_function("dates::FlexibleDate::to_w3c_date", |b| {
        b.iter(|| black_box(flex.to_w3c_date()));
    });
    c.bench_function("dates::FlexibleDate::to_iso_date", |b| {
        b.iter(|| black_box(flex.to_iso_date()));
    });
    c.bench_function("dates::is_leap_year", |b| {
        b.iter(|| black_box(is_leap_year(black_box(2026))));
    });
    c.bench_function("dates::days_in_month", |b| {
        b.iter(|| black_box(days_in_month(black_box(2026), black_box(2))));
    });
    let date_err = parse_flexible_date("not a date").unwrap_err();
    c.bench_function("dates::DateParseError::attempted_formats+input", |b| {
        b.iter(|| {
            black_box(date_err.attempted_formats());
            black_box(date_err.input())
        });
    });

    // urls (v0.0.47) — permalink / output-path derivation helpers.
    use ssg::urls::{
        derive_output_rel_path, derive_page_url, derive_permalink,
    };
    c.bench_function("urls::derive_page_url", |b| {
        b.iter(|| {
            black_box(derive_page_url(
                black_box("https://example.com"),
                black_box("posts/foo/index.html"),
            ))
        });
    });
    c.bench_function("urls::derive_output_rel_path", |b| {
        b.iter(|| black_box(derive_output_rel_path(black_box("posts/foo.md"))));
    });
    c.bench_function("urls::derive_permalink", |b| {
        b.iter(|| {
            black_box(derive_permalink(
                black_box("https://example.com"),
                black_box("posts/foo.md"),
            ))
        });
    });

    // io_pool (v0.0.47) — write+flush cycle against a shared pool. The
    // pool and tempdir are built once outside the timed closure; each
    // iteration enqueues 32 × 1 KiB jobs and barriers on flush().
    use ssg::io_pool::IoPool;
    let io_dir = tempfile::tempdir().unwrap();
    let io_pool = IoPool::new();
    let io_payload = vec![0u8; 1024];
    c.bench_function("io_pool::IoPool::write+flush (32x1KiB)", |b| {
        b.iter(|| {
            for i in 0..32 {
                io_pool
                    .write(
                        io_dir.path().join(format!("f{i}.bin")),
                        io_payload.clone(),
                    )
                    .unwrap();
            }
            black_box(io_pool.flush())
        });
    });
    c.bench_function("io_pool::IoPool::completed_writes", |b| {
        b.iter(|| black_box(io_pool.completed_writes()));
    });
    c.bench_function("io_pool::IoPool::threads", |b| {
        b.iter(|| black_box(io_pool.threads()));
    });
    // SKIPPED: io_pool::IoPool::new / with_threads / default — spawn
    //          and join OS writer threads per iteration (Drop joins),
    //          which measures thread lifecycle, not pool behaviour;
    //          the constructor path is exercised by the shared pool
    //          above.

    // content_stager (v0.0.47 additions) — permalink-injecting stage
    // pass over a 3-file content tree + the pure injection helper.
    use ssg::content_stager::{
        inject_permalink_if_missing, stage_content_with_site_defaults,
    };
    let cs_tmp = tempfile::tempdir().unwrap();
    let cs_src = cs_tmp.path().join("content");
    std::fs::create_dir_all(&cs_src).unwrap();
    std::fs::write(cs_src.join("index.md"), "---\ntitle: Home\n---\nbody")
        .unwrap();
    std::fs::write(cs_src.join("a.md"), "---\ntitle: A\n---\nbody").unwrap();
    std::fs::write(cs_src.join("b.md"), "---\ntitle: B\n---\nbody").unwrap();
    let cs_build = cs_tmp.path().join("build");
    let cs_keys: &[String] = &[];
    c.bench_function(
        "content_stager::stage_content_with_site_defaults (3 files)",
        |b| {
            b.iter(|| {
                black_box(stage_content_with_site_defaults(
                    black_box(&cs_src),
                    black_box(&cs_build),
                    black_box(cs_keys),
                    black_box(Some("https://example.com")),
                    black_box(&[]),
                ))
            });
        },
    );
    c.bench_function("content_stager::inject_permalink_if_missing", |b| {
        b.iter(|| {
            black_box(inject_permalink_if_missing(
                black_box("---\ntitle: A\n---\nbody"),
                black_box("https://example.com/a/"),
            ))
        });
    });
    // SKIPPED: content_stager::stage_content_with_template_defaults —
    //          thin delegate to stage_content_with_site_defaults with
    //          base_url: None; identical code path benched above.
    // SKIPPED: content_stager::collect_template_vars /
    //          inject_missing_keys — pre-v0.0.47 surface, covered by
    //          the staging pass above and unit tests.
}

// ====================================================================
// group_plugins_seo — every public fn in src/plugins/seo/
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_plugins_seo(c: &mut Criterion) {
    use ssg::seo::helpers::{extract_title, has_meta_tag};
    use ssg::seo::jsonld::{JsonLdConfig, JsonLdPlugin};
    use ssg::seo::{validate_jsonld, CanonicalPlugin, RobotsPlugin, SeoPlugin};

    c.bench_function("seo::helpers::extract_title", |b| {
        b.iter(|| black_box(extract_title(black_box(SAMPLE_HTML))));
    });
    c.bench_function("seo::helpers::has_meta_tag", |b| {
        b.iter(|| {
            black_box(has_meta_tag(
                black_box(SAMPLE_HTML),
                black_box("description"),
            ))
        });
    });
    c.bench_function("seo::jsonld::validate_jsonld", |b| {
        b.iter(|| black_box(validate_jsonld(black_box(SAMPLE_HTML))));
    });

    c.bench_function("seo::canonical::CanonicalPlugin::new", |b| {
        b.iter(|| black_box(CanonicalPlugin::new("https://example.com")));
    });
    c.bench_function("seo::robots::RobotsPlugin::new", |b| {
        b.iter(|| black_box(RobotsPlugin::new("https://example.com")));
    });
    c.bench_function("seo::seo_plugin::SeoPlugin (unit)", |b| {
        b.iter(|| black_box(SeoPlugin));
    });
    let jsonld_cfg = JsonLdConfig {
        base_url: "https://example.com".into(),
        org_name: "ACME".into(),
        breadcrumbs: true,
    };
    c.bench_function("seo::jsonld::JsonLdPlugin::new", |b| {
        b.iter(|| black_box(JsonLdPlugin::new(jsonld_cfg.clone())));
    });
    c.bench_function("seo::jsonld::JsonLdPlugin::from_site", |b| {
        b.iter(|| {
            black_box(JsonLdPlugin::from_site(
                black_box("https://example.com"),
                black_box("ACME"),
            ))
        });
    });
}

// ====================================================================
// group_plugins_jsonld_iso20022
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_plugins_jsonld_iso20022(c: &mut Criterion) {
    use ssg::seo::jsonld::iso20022::{
        from_frontmatter, log_first_use_pointer, validate_bic, validate_iban,
        validate_schema_org, warn_invalid_fields, BankAccount,
        FinancialProduct, FinancialTransaction, Iso20022Entity, MonetaryAmount,
        PaymentInstrument, RegulatedFinancialInstitution,
    };

    c.bench_function("iso20022::log_first_use_pointer", |b| {
        b.iter(|| {
            log_first_use_pointer();
            black_box(())
        });
    });
    c.bench_function("iso20022::validate_iban", |b| {
        b.iter(|| {
            black_box(validate_iban(black_box("GB82WEST12345698765432")))
        });
    });
    c.bench_function("iso20022::validate_bic", |b| {
        b.iter(|| black_box(validate_bic(black_box("DEUTDEFF"))));
    });

    let ma = MonetaryAmount {
        currency: "EUR".into(),
        amount: 42.0,
    };
    c.bench_function("iso20022::MonetaryAmount::to_jsonld", |b| {
        b.iter(|| black_box(ma.to_jsonld()));
    });

    let ba = BankAccount {
        name: Some("Alice".into()),
        iban: Some("GB82WEST12345698765432".into()),
        bic: Some("DEUTDEFF".into()),
    };
    c.bench_function("iso20022::BankAccount::to_jsonld", |b| {
        b.iter(|| black_box(ba.to_jsonld()));
    });

    let pi = PaymentInstrument {
        name: Some("Visa".into()),
        instrument_type: "card".into(),
        brand: Some("Visa".into()),
    };
    c.bench_function("iso20022::PaymentInstrument::to_jsonld", |b| {
        b.iter(|| black_box(pi.to_jsonld()));
    });

    let ft = FinancialTransaction {
        instructed_amount: Some(ma.clone()),
        debtor_account: Some(ba.clone()),
        creditor_account: Some(ba.clone()),
        execution_date: Some("2026-06-26T00:00:00Z".into()),
        end_to_end_id: Some("E2E-1".into()),
    };
    c.bench_function("iso20022::FinancialTransaction::to_jsonld", |b| {
        b.iter(|| black_box(ft.to_jsonld()));
    });

    let rfi = RegulatedFinancialInstitution {
        name: "ACME Bank".into(),
        lei: Some("529900T8BM49AURSDO55".into()),
        licence_id: Some("FCA-12345".into()),
        regulator: Some("FCA".into()),
        url: Some("https://example.com".into()),
    };
    c.bench_function(
        "iso20022::RegulatedFinancialInstitution::to_jsonld",
        |b| {
            b.iter(|| black_box(rfi.to_jsonld()));
        },
    );

    let fp = FinancialProduct {
        name: "Mortgage 30Y".into(),
        product_type: "loan".into(),
        issuer: Some("ACME Bank".into()),
        annual_percentage_rate: Some(4.25),
        isin: Some("US0378331005".into()),
    };
    c.bench_function("iso20022::FinancialProduct::to_jsonld", |b| {
        b.iter(|| black_box(fp.to_jsonld()));
    });

    let ent = Iso20022Entity::BankAccount(ba);
    c.bench_function("iso20022::Iso20022Entity::to_jsonld", |b| {
        b.iter(|| black_box(ent.to_jsonld()));
    });
    c.bench_function("iso20022::Iso20022Entity::type_name", |b| {
        b.iter(|| black_box(ent.type_name()));
    });

    let fm_value = serde_json::json!({
        "type": "BankAccount",
        "iban": "GB82WEST12345698765432",
    });
    c.bench_function("iso20022::from_frontmatter", |b| {
        b.iter(|| {
            let _ = black_box(from_frontmatter(black_box(&fm_value)));
        });
    });
    c.bench_function("iso20022::warn_invalid_fields", |b| {
        b.iter(|| {
            black_box(warn_invalid_fields(black_box(&ent), black_box("p")))
        });
    });

    let so_value = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "WebPage",
        "name": "Sample",
    });
    c.bench_function("iso20022::validate_schema_org", |b| {
        b.iter(|| black_box(validate_schema_org(black_box(&so_value))));
    });
}

// ====================================================================
// group_plugins_postprocess_agentic
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_plugins_postprocess_agentic(c: &mut Criterion) {
    use ssg::postprocess::agentic_discovery::{
        build_manifest as ai_build_manifest, build_registry, render_agents_txt,
        AgenticDiscoveryPlugin, AgentsConfig, McpResource,
    };

    let cfg = ssg::cmd::SsgConfig::default();
    let agents = AgentsConfig {
        agents_txt: true,
        ai_plugin: true,
        ..AgentsConfig::default()
    };

    c.bench_function("agentic::AgentsConfig::any_enabled", |b| {
        b.iter(|| black_box(agents.any_enabled()));
    });
    c.bench_function("agentic::AgenticDiscoveryPlugin (unit)", |b| {
        b.iter(|| black_box(AgenticDiscoveryPlugin));
    });
    c.bench_function("agentic::ai_plugin::build_manifest", |b| {
        b.iter(|| black_box(ai_build_manifest(black_box(&cfg))));
    });
    c.bench_function("agentic::agents_txt::render_agents_txt", |b| {
        b.iter(|| {
            black_box(render_agents_txt(
                black_box(&agents),
                black_box("https://example.com"),
            ))
        });
    });
    let resources: Vec<McpResource> = Vec::new();
    c.bench_function("agentic::mcp::build_registry", |b| {
        b.iter(|| {
            black_box(build_registry(
                black_box(&cfg),
                black_box(&agents),
                black_box(&resources),
            ))
        });
    });
    // SKIPPED: agentic::ai_plugin::write_ai_plugin_json — disk I/O
    // SKIPPED: agentic::agents_txt::write_agents_txt — disk I/O
    // SKIPPED: agentic::mcp::write_mcp_registry — disk I/O
    // SKIPPED: agentic::mcp::collect_mcp_resources — needs a built site
}

// ====================================================================
// group_plugins_postprocess_edge_headers
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_plugins_postprocess_edge_headers(c: &mut Criterion) {
    use ssg::postprocess::edge_headers::{
        baseline_headers, merged_headers, EdgeHeadersPlugin,
    };

    c.bench_function("edge_headers::baseline_headers", |b| {
        b.iter(|| black_box(baseline_headers()));
    });
    let overrides: BTreeMap<String, String> = BTreeMap::new();
    c.bench_function("edge_headers::merged_headers (empty overrides)", |b| {
        b.iter(|| black_box(merged_headers(black_box(&overrides))));
    });
    c.bench_function("edge_headers::EdgeHeadersPlugin::new", |b| {
        b.iter(|| black_box(EdgeHeadersPlugin::new()));
    });
    // The cloudflare/netlify/vercel emitters are private (`pub(crate)`);
    // the public surface is the plugin itself + the two helpers above.
}

// ====================================================================
// group_plugins_view_transitions
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_plugins_view_transitions(c: &mut Criterion) {
    use ssg::view_transitions::ViewTransitionsPlugin;

    let cfg = ssg::cmd::SsgConfig::default();
    c.bench_function("view_transitions::ViewTransitionsPlugin::new", |b| {
        b.iter(|| black_box(ViewTransitionsPlugin::new()));
    });
    c.bench_function("view_transitions::ViewTransitionsPlugin::enabled", |b| {
        b.iter(|| black_box(ViewTransitionsPlugin::enabled(black_box(&cfg))));
    });
}

// ====================================================================
// group_plugins_misc — everything else under src/plugins/
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_plugins_misc(c: &mut Criterion) {
    // csp
    use ssg::csp::{computed_policy, inject_csp_meta, CspPlugin};
    c.bench_function("csp::computed_policy", |b| {
        b.iter(|| black_box(computed_policy()));
    });
    c.bench_function("csp::CspPlugin::new", |b| {
        b.iter(|| black_box(CspPlugin::new()));
    });
    c.bench_function("csp::inject_csp_meta", |b| {
        b.iter(|| {
            black_box(inject_csp_meta(
                black_box(SAMPLE_HTML),
                black_box("default-src 'self'"),
            ))
        });
    });

    // csp v0.0.47 additions — per-page hash-strict policy pipeline.
    // SAMPLE_HTML carries an inline JSON-LD <script>, so every helper
    // below does real hashing work.
    use ssg::csp::{
        page_inline_hashes, page_policy, render_policy_template,
        DEFAULT_CSP_POLICY_TEMPLATE,
    };
    c.bench_function("csp::page_inline_hashes", |b| {
        b.iter(|| black_box(page_inline_hashes(black_box(SAMPLE_HTML))));
    });
    let csp_hashes = page_inline_hashes(SAMPLE_HTML);
    c.bench_function("csp::PageCspHashes::is_empty", |b| {
        b.iter(|| black_box(csp_hashes.is_empty()));
    });
    c.bench_function("csp::render_policy_template", |b| {
        b.iter(|| {
            black_box(render_policy_template(
                black_box(DEFAULT_CSP_POLICY_TEMPLATE),
                black_box(&csp_hashes.scripts),
                black_box(&csp_hashes.styles),
            ))
        });
    });
    c.bench_function("csp::page_policy", |b| {
        b.iter(|| black_box(page_policy(black_box(SAMPLE_HTML))));
    });

    // drafts
    use ssg::drafts::DraftPlugin;
    c.bench_function("drafts::DraftPlugin::new", |b| {
        b.iter(|| black_box(DraftPlugin::new(black_box(true))));
    });

    // highlight
    use ssg::highlight::HighlightPlugin;
    c.bench_function("highlight::HighlightPlugin::new", |b| {
        b.iter(|| black_box(HighlightPlugin::new("github-dark")));
    });
    c.bench_function("highlight::HighlightPlugin::with_theme", |b| {
        b.iter(|| black_box(HighlightPlugin::with_theme("github-dark")));
    });

    // islands
    use ssg::islands::IslandPlugin;
    c.bench_function("islands::IslandPlugin::new", |b| {
        b.iter(|| black_box(IslandPlugin::new()));
    });

    // isr_manifest plugin (the ssg::plugin one, not ssg-core)
    use ssg::isr_manifest::{build_manifest, IsrManifestPlugin};
    c.bench_function("isr_manifest::IsrManifestPlugin::new", |b| {
        b.iter(|| black_box(IsrManifestPlugin::new()));
    });
    let (_tmp_isr, _ctx_isr) = build_plugin_ctx();
    let isr_dir = tempfile::tempdir().unwrap();
    c.bench_function("isr_manifest::build_manifest", |b| {
        b.iter(|| {
            let _ = black_box(build_manifest(
                black_box(isr_dir.path()),
                black_box(isr_dir.path()),
                black_box(isr_dir.path()),
            ));
        });
    });

    // markdown_ext::expand_gfm
    use ssg::markdown_ext::expand_gfm;
    c.bench_function("markdown_ext::expand_gfm", |b| {
        b.iter(|| {
            black_box(expand_gfm(black_box(SAMPLE_MARKDOWN), black_box(None)))
        });
    });

    // og_image
    use ssg::og_image::{generate_og_svg, OgImagePlugin};
    c.bench_function("og_image::OgImagePlugin::new", |b| {
        b.iter(|| black_box(OgImagePlugin::new("https://example.com")));
    });
    c.bench_function("og_image::OgImagePlugin::with_colors", |b| {
        b.iter(|| {
            black_box(OgImagePlugin::with_colors(
                "https://example.com",
                "#000",
                "#fff",
            ))
        });
    });
    c.bench_function("og_image::generate_og_svg", |b| {
        b.iter(|| {
            black_box(generate_og_svg(
                black_box("Title"),
                black_box("Site"),
                black_box("#000"),
                black_box("#fff"),
            ))
        });
    });

    // pagination
    use ssg::pagination::PaginationPlugin;
    c.bench_function("pagination::PaginationPlugin::with_per_page", |b| {
        b.iter(|| black_box(PaginationPlugin::with_per_page(10)));
    });

    // plugin: PluginContext + PluginCache + PluginManager.
    use ssg::plugin::{PluginCache, PluginContext, PluginManager};
    let tmp_pc = tempfile::tempdir().unwrap();
    let p = tmp_pc.path();
    c.bench_function("plugin::PluginContext::new", |b| {
        b.iter(|| black_box(PluginContext::new(p, p, p, p)));
    });
    c.bench_function("plugin::PluginContext::with_config", |b| {
        b.iter(|| {
            black_box(PluginContext::with_config(
                p,
                p,
                p,
                p,
                ssg::cmd::SsgConfig::default(),
            ))
        });
    });
    let ctx_pc = PluginContext::new(p, p, p, p);
    c.bench_function("plugin::PluginContext::with_dry_run", |b| {
        b.iter(|| black_box(ctx_pc.clone().with_dry_run(black_box(true))));
    });
    c.bench_function("plugin::PluginContext::get_html_files", |b| {
        b.iter(|| black_box(ctx_pc.get_html_files()));
    });
    c.bench_function("plugin::PluginContext::cache_html_files", |b| {
        b.iter(|| {
            let mut cx = ctx_pc.clone();
            cx.cache_html_files();
            black_box(cx)
        });
    });

    c.bench_function("plugin::PluginCache::new", |b| {
        b.iter(|| black_box(PluginCache::new()));
    });
    c.bench_function("plugin::PluginCache::load (missing)", |b| {
        b.iter(|| black_box(PluginCache::load(black_box(p))));
    });
    let pc = PluginCache::new();
    c.bench_function("plugin::PluginCache::has_changed", |b| {
        b.iter(|| {
            black_box(pc.has_changed(black_box(&p.join("missing.html"))))
        });
    });
    let real_file = p.join("a.html");
    std::fs::write(&real_file, b"<p>hi</p>").unwrap();
    c.bench_function("plugin::PluginCache::update", |b| {
        b.iter(|| {
            let mut pc = PluginCache::new();
            pc.update(black_box(&real_file));
            black_box(pc)
        });
    });
    let pc_pop = {
        let mut pc = PluginCache::new();
        pc.update(&real_file);
        pc
    };
    c.bench_function("plugin::PluginCache::save", |b| {
        b.iter(|| {
            let _ = black_box(pc_pop.save(black_box(p)));
        });
    });

    let mut pm = PluginManager::new();
    c.bench_function("plugin::PluginManager::new", |b| {
        b.iter(|| black_box(PluginManager::new()));
    });
    pm.register(IslandPlugin::new());
    c.bench_function("plugin::PluginManager::len", |b| {
        b.iter(|| black_box(pm.len()));
    });
    c.bench_function("plugin::PluginManager::is_empty", |b| {
        b.iter(|| black_box(pm.is_empty()));
    });
    c.bench_function("plugin::PluginManager::names", |b| {
        b.iter(|| black_box(pm.names()));
    });
    let ctx_for_pm = PluginContext::new(p, p, p, p);
    c.bench_function("plugin::PluginManager::run_before_compile", |b| {
        b.iter(|| {
            let _ = black_box(pm.run_before_compile(black_box(&ctx_for_pm)));
        });
    });
    c.bench_function("plugin::PluginManager::run_after_compile", |b| {
        b.iter(|| {
            let _ = black_box(pm.run_after_compile(black_box(&ctx_for_pm)));
        });
    });
    c.bench_function("plugin::PluginManager::run_fused_transforms", |b| {
        b.iter(|| {
            let _ = black_box(pm.run_fused_transforms(black_box(&ctx_for_pm)));
        });
    });
    c.bench_function("plugin::PluginManager::run_on_serve", |b| {
        b.iter(|| {
            let _ = black_box(pm.run_on_serve(black_box(&ctx_for_pm)));
        });
    });

    // plugins helper module — minify wrappers.
    use ssg::plugins::minify_html;
    c.bench_function("plugins::minify_html", |b| {
        b.iter(|| black_box(minify_html(black_box(SAMPLE_HTML))));
    });
    use ssg::plugins::{minify_css, minify_js};
    c.bench_function("plugins::minify_css", |b| {
        b.iter(|| black_box(minify_css(black_box("body { color: red; }"))));
    });
    c.bench_function("plugins::minify_js", |b| {
        b.iter(|| black_box(minify_js(black_box("var x = 1 + 2;"))));
    });

    // rpc_schema plugin.
    use ssg::rpc_schema::RpcSchemaPlugin;
    c.bench_function("rpc_schema::RpcSchemaPlugin::new", |b| {
        b.iter(|| black_box(RpcSchemaPlugin::new()));
    });

    // sbom plugin (the ssg::sbom path)
    use ssg::sbom::SbomPlugin;
    c.bench_function("sbom::SbomPlugin::sbom_path", |b| {
        b.iter(|| black_box(SbomPlugin::sbom_path()));
    });

    // shortcodes
    use ssg::shortcodes::expand_shortcodes;
    c.bench_function("shortcodes::expand_shortcodes", |b| {
        b.iter(|| {
            black_box(expand_shortcodes(black_box(
                "{{< note >}}body{{< /note >}}",
            )))
        });
    });

    // search — labels + plugin
    use ssg::search::{LocalizedSearchPlugin, SearchIndex, SearchLabels};
    c.bench_function("search::SearchLabels::english", |b| {
        b.iter(|| black_box(SearchLabels::english()));
    });
    c.bench_function("search::SearchLabels::french", |b| {
        b.iter(|| black_box(SearchLabels::french()));
    });
    c.bench_function("search::SearchLabels::for_locale", |b| {
        b.iter(|| black_box(SearchLabels::for_locale(black_box("de"))));
    });
    c.bench_function("search::LocalizedSearchPlugin::new", |b| {
        b.iter(|| {
            black_box(LocalizedSearchPlugin::new(SearchLabels::english()))
        });
    });
    let tmp_search = tempfile::tempdir().unwrap();
    std::fs::write(tmp_search.path().join("a.html"), SAMPLE_HTML).unwrap();
    c.bench_function("search::SearchIndex::build", |b| {
        b.iter(|| {
            let _ = black_box(SearchIndex::build(black_box(tmp_search.path())));
        });
    });
    let idx = SearchIndex::build(tmp_search.path()).unwrap();
    c.bench_function("search::SearchIndex::len", |b| {
        b.iter(|| black_box(idx.len()));
    });
    c.bench_function("search::SearchIndex::is_empty", |b| {
        b.iter(|| black_box(idx.is_empty()));
    });
    c.bench_function("search::SearchIndex::write", |b| {
        b.iter(|| {
            let _ = black_box(idx.write(black_box(tmp_search.path())));
        });
    });

    // template_plugin. The `ssg::template_plugin` re-export is gated
    // behind the `templates` feature, so the whole section must be
    // cfg-gated or `--no-default-features` fails to type-check this
    // bench (v0.0.47 plan, W1-E gating fix). cargo-hack's
    // feature-powerset job checks with `--no-dev-deps` (lib/bins
    // only), so this gate is what keeps `cargo check --all-targets
    // --no-default-features` green locally.
    #[cfg(feature = "templates")]
    {
        use ssg::template_engine::TemplateConfig;
        use ssg::template_plugin::TemplatePlugin;
        let tdir = tempfile::tempdir().unwrap();
        c.bench_function(
            "template_plugin::TemplatePlugin::from_template_dir",
            |b| {
                b.iter(|| {
                    black_box(TemplatePlugin::from_template_dir(black_box(
                        tdir.path(),
                    )))
                });
            },
        );
        c.bench_function("template_plugin::TemplatePlugin::new", |b| {
            b.iter(|| {
                black_box(TemplatePlugin::new(TemplateConfig::default()))
            });
        });
    }

    // i18n — public free functions + plugin ctor.
    use ssg::i18n::{
        generate_lang_switcher_html, negotiate_locale, parse_accept_language,
        I18nPlugin, UrlPrefixStrategy,
    };
    c.bench_function("i18n::parse_accept_language", |b| {
        b.iter(|| {
            black_box(parse_accept_language(black_box(
                "fr-CH, fr;q=0.9, en;q=0.8, *;q=0.1",
            )))
        });
    });
    let pref = vec!["fr-CH".to_string(), "fr".to_string()];
    let avail = vec!["fr".to_string(), "en".to_string()];
    c.bench_function("i18n::negotiate_locale", |b| {
        b.iter(|| {
            black_box(negotiate_locale(
                black_box(&pref),
                black_box(&avail),
                black_box("en"),
            ))
        });
    });
    let locales: Vec<String> = vec!["en".into(), "fr".into(), "de".into()];
    c.bench_function("i18n::generate_lang_switcher_html", |b| {
        b.iter(|| {
            black_box(generate_lang_switcher_html(
                black_box(&locales),
                black_box("en"),
                black_box("about/index.html"),
                black_box("https://example.com"),
                black_box(&UrlPrefixStrategy::SubPath),
            ))
        });
    });
    let i18n_cfg = ssg::i18n::I18nConfig::default();
    c.bench_function("i18n::I18nPlugin::new", |b| {
        b.iter(|| black_box(I18nPlugin::new(i18n_cfg.clone())));
    });

    // llm + llm_cache: pure constructors + the readability analyses.
    use ssg::llm::{
        LlmConfig, LlmPlugin, ReadabilityAudit, ReadabilityFormula,
    };
    c.bench_function("llm::LlmPlugin::new", |b| {
        b.iter(|| black_box(LlmPlugin::new(LlmConfig::default())));
    });
    c.bench_function("llm::ReadabilityFormula::from_lang", |b| {
        b.iter(|| black_box(ReadabilityFormula::from_lang(black_box("en"))));
    });
    let sample_text = "The quick brown fox jumps over the lazy dog. \
                       Each sentence is a small unit of meaning. The end.";
    c.bench_function("llm::ReadabilityAudit::analyze", |b| {
        b.iter(|| black_box(ReadabilityAudit::analyze(black_box(sample_text))));
    });
    c.bench_function("llm::ReadabilityAudit::analyze_with_lang", |b| {
        b.iter(|| {
            black_box(ReadabilityAudit::analyze_with_lang(
                black_box(sample_text),
                black_box("en"),
            ))
        });
    });
    // SKIPPED: llm::LlmPlugin::audit_all / audit_and_fix / audit_and_fix_with_report
    //          — drive a real Ollama backend or full file walks.
    // SKIPPED: llm::query_ollama / OllamaClient::query — network I/O.

    use ssg::llm_cache::LlmCache;
    let cache_root = tempfile::tempdir().unwrap();
    c.bench_function("llm_cache::LlmCache::new", |b| {
        b.iter(|| black_box(LlmCache::new(cache_root.path().to_path_buf())));
    });
    c.bench_function("llm_cache::LlmCache::with_ttl", |b| {
        b.iter(|| {
            black_box(LlmCache::with_ttl(
                cache_root.path().to_path_buf(),
                Duration::from_secs(3600),
            ))
        });
    });
    c.bench_function("llm_cache::LlmCache::default_cache_dir", |b| {
        b.iter(|| black_box(LlmCache::default_cache_dir()));
    });
    let llm_cache = LlmCache::new(cache_root.path().to_path_buf());
    c.bench_function("llm_cache::LlmCache::compute_key", |b| {
        b.iter(|| {
            black_box(LlmCache::compute_key(
                black_box("http://localhost:11434"),
                black_box("llama3"),
                black_box("hello"),
                black_box(30),
            ))
        });
    });
    let key = LlmCache::compute_key("http://localhost", "llama3", "hi", 30);
    c.bench_function("llm_cache::LlmCache::get (miss)", |b| {
        b.iter(|| black_box(llm_cache.get(black_box(&key))));
    });
    c.bench_function("llm_cache::LlmCache::stats", |b| {
        b.iter(|| black_box(llm_cache.stats()));
    });
    c.bench_function("llm_cache::LlmCache::root", |b| {
        b.iter(|| black_box(llm_cache.root()));
    });
    // SKIPPED: llm_cache::LlmCache::set / evict — disk-mutating; covered
    //          by unit tests in the same module.

    // image_plugin::encode_avif — gated on image-optimization.
    #[cfg(feature = "image-optimization")]
    {
        use image::{ImageBuffer, Rgba};
        use ssg::image_plugin::encode_avif;
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(16, 16, Rgba([255, 0, 0, 255]));
        let dyn_img = image::DynamicImage::ImageRgba8(img);
        c.bench_function("image_plugin::encode_avif (16x16)", |b| {
            b.iter(|| {
                let _ =
                    black_box(encode_avif(black_box(&dyn_img), black_box(50)));
            });
        });
    }

    // Postprocess plugin units (unit structs — just bench construction).
    c.bench_function("postprocess::atom::AtomFeedPlugin (unit)", |b| {
        b.iter(|| black_box(ssg::postprocess::AtomFeedPlugin));
    });
    c.bench_function("postprocess::html_fix::HtmlFixPlugin (unit)", |b| {
        b.iter(|| black_box(ssg::postprocess::HtmlFixPlugin));
    });
    c.bench_function("postprocess::json_feed::JsonFeedPlugin (unit)", |b| {
        b.iter(|| black_box(ssg::postprocess::JsonFeedPlugin));
    });
    c.bench_function("postprocess::manifest::ManifestFixPlugin (unit)", |b| {
        b.iter(|| black_box(ssg::postprocess::ManifestFixPlugin));
    });
    c.bench_function(
        "postprocess::news_sitemap::NewsSitemapFixPlugin (unit)",
        |b| {
            b.iter(|| black_box(ssg::postprocess::NewsSitemapFixPlugin));
        },
    );
    c.bench_function("postprocess::rss::RssAggregatePlugin (unit)", |b| {
        b.iter(|| black_box(ssg::postprocess::RssAggregatePlugin));
    });
    c.bench_function("postprocess::sbom::SbomPlugin (unit)", |b| {
        b.iter(|| black_box(ssg::postprocess::SbomPlugin));
    });
    c.bench_function("postprocess::sitemap::SitemapFixPlugin (unit)", |b| {
        b.iter(|| black_box(ssg::postprocess::SitemapFixPlugin));
    });

    // SKIPPED: plugins::ProgressBar / progress::* — terminal UI helpers
    //          covered by their own unit tests.
}

// ====================================================================
// group_plugins_agent_surfaces — agent_api / oembed / vector search /
// taxonomy (v0.0.47 agent-facing surfaces)
// ====================================================================

/// Builds a small compiled-site fixture: three HTML pages with
/// `.meta.json` sidecars both next to the HTML (the layout
/// `agent_api::collect_posts` falls back to) and under
/// `<site>/.meta/` (the sidecar tree `agent_api` and taxonomy prefer).
/// Returns the `TempDir` plus a `PluginContext` carrying a config
/// with a real `base_url`.
fn build_agent_site_fixture() -> (TempDir, ssg::plugin::PluginContext) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let content = root.join("content");
    let build = root.join("build");
    let site = root.join("site");
    let templates = root.join("templates");
    for d in [&content, &build, &site, &templates] {
        std::fs::create_dir_all(d).unwrap();
    }
    let meta_dir = site.join(".meta");
    std::fs::create_dir_all(&meta_dir).unwrap();

    for (stem, title) in
        [("alpha", "Alpha"), ("beta", "Beta"), ("gamma", "Gamma")]
    {
        std::fs::write(site.join(format!("{stem}.html")), SAMPLE_HTML).unwrap();
        let sidecar = format!(
            r#"{{"title":"{title}","description":"About {title}.",
"date":"2026-07-01","tags":["rust","ssg"],"categories":["bench"],
"author":"jane@example.com (Jane Doe)"}}"#
        );
        std::fs::write(
            site.join(format!("{stem}.meta.json")),
            sidecar.as_bytes(),
        )
        .unwrap();
        std::fs::write(
            meta_dir.join(format!("{stem}.meta.json")),
            sidecar.as_bytes(),
        )
        .unwrap();
    }

    let cfg = ssg::cmd::SsgConfig::builder()
        .site_name("bench".into())
        .base_url("https://example.com".into())
        .build()
        .expect("valid config");
    let ctx = ssg::plugin::PluginContext::with_config(
        &content, &build, &site, &templates, cfg,
    );
    (tmp, ctx)
}

#[allow(unreachable_pub)]
pub fn bench_plugins_agent_surfaces(c: &mut Criterion) {
    use ssg::agent_api::{
        collect_posts, jsonld_word_count, parse_author, AgentApiPlugin,
    };
    use ssg::oembed::{build_oembed, OembedPlugin};
    use ssg::plugin::Plugin as _;
    use ssg::search_index::VectorSearchPlugin;
    use ssg::taxonomy::{TaxonomyPlugin, TaxonomyTerm};

    let (_tmp, ctx) = build_agent_site_fixture();

    // agent_api — constructors + sidecar collection + after_compile
    // (writes the four /api/agents/*.json documents each iteration).
    c.bench_function("agent_api::AgentApiPlugin::new", |b| {
        b.iter(|| black_box(AgentApiPlugin::new()));
    });
    c.bench_function("agent_api::AgentApiPlugin::disabled", |b| {
        b.iter(|| black_box(AgentApiPlugin::disabled()));
    });
    let agent_plugin = AgentApiPlugin::new();
    c.bench_function("agent_api::AgentApiPlugin::after_compile (3p)", |b| {
        b.iter(|| {
            let _ = black_box(agent_plugin.after_compile(black_box(&ctx)));
        });
    });
    let agent_disabled = AgentApiPlugin::disabled();
    c.bench_function(
        "agent_api::AgentApiPlugin::after_compile (disabled)",
        |b| {
            b.iter(|| {
                let _ =
                    black_box(agent_disabled.after_compile(black_box(&ctx)));
            });
        },
    );
    c.bench_function("agent_api::collect_posts", |b| {
        b.iter(|| black_box(collect_posts(black_box(&ctx))));
    });
    let jsonld_html = r#"<script type="application/ld+json">
      {"@type":"BlogPosting","wordCount":321}
    </script>"#;
    c.bench_function("agent_api::jsonld_word_count", |b| {
        b.iter(|| black_box(jsonld_word_count(black_box(jsonld_html))));
    });
    c.bench_function("agent_api::parse_author", |b| {
        b.iter(|| {
            black_box(parse_author(black_box("jane@example.com (Jane Doe)")))
        });
    });

    // oembed — after_compile writes the *.oembed.json siblings, and
    // transform_html injects the discovery <link> (the sibling exists
    // after the first after_compile pass below).
    let oembed = OembedPlugin;
    c.bench_function("oembed::OembedPlugin::after_compile (3p)", |b| {
        b.iter(|| {
            let _ = black_box(oembed.after_compile(black_box(&ctx)));
        });
    });
    oembed.after_compile(&ctx).unwrap();
    let oembed_page = ctx.site_dir.join("alpha.html");
    c.bench_function("oembed::OembedPlugin::transform_html", |b| {
        b.iter(|| {
            let _ = black_box(oembed.transform_html(
                black_box(SAMPLE_HTML),
                black_box(&oembed_page),
                black_box(&ctx),
            ));
        });
    });
    c.bench_function("oembed::build_oembed", |b| {
        b.iter(|| {
            black_box(build_oembed(
                black_box("Alpha"),
                black_box(Some("jane@example.com (Jane Doe)")),
                black_box(Some("bench")),
                black_box(Some("https://example.com")),
            ))
        });
    });

    // search_index::VectorSearchPlugin — builds the SearchIndex over
    // the three fixture pages, embeds them, and writes the four
    // <site>/search/ artifacts per iteration.
    let vector = VectorSearchPlugin;
    c.bench_function(
        "search_index::VectorSearchPlugin::after_compile (3p)",
        |b| {
            b.iter(|| {
                let _ = black_box(vector.after_compile(black_box(&ctx)));
            });
        },
    );

    // taxonomy — per-term landing pages (#586 port 5). Runs last so
    // the generated /tags/... pages don't inflate the vector-search
    // bench above. TaxonomyTerm is the new public value type.
    c.bench_function("taxonomy::TaxonomyTerm (construct)", |b| {
        b.iter(|| {
            black_box(TaxonomyTerm {
                name: "rust".into(),
                slug: "rust".into(),
                pages: vec![(
                    "Alpha".into(),
                    "https://example.com/alpha.html".into(),
                )],
            })
        });
    });
    let taxonomy = TaxonomyPlugin;
    c.bench_function("taxonomy::TaxonomyPlugin::after_compile (3p)", |b| {
        b.iter(|| {
            let _ = black_box(taxonomy.after_compile(black_box(&ctx)));
        });
    });
}

// ====================================================================
// group_util — head_dom + html_rewriter
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_util(c: &mut Criterion) {
    use ssg::util::head_dom::{
        extract_head_meta, inject_before_head_close, remove_canonical_links,
        replace_canonical_link,
    };
    use ssg::util::html_rewriter::{
        collapse_whitespace, decode_html_entities, extract_text_with_filter,
    };

    c.bench_function("util::head_dom::inject_before_head_close", |b| {
        b.iter(|| {
            black_box(inject_before_head_close(
                black_box(SAMPLE_HTML),
                black_box("<meta name=\"injected\" content=\"1\">"),
            ))
        });
    });
    c.bench_function("util::head_dom::extract_head_meta", |b| {
        b.iter(|| black_box(extract_head_meta(black_box(SAMPLE_HTML))));
    });
    c.bench_function("util::head_dom::remove_canonical_links", |b| {
        b.iter(|| black_box(remove_canonical_links(black_box(SAMPLE_HTML))));
    });
    c.bench_function("util::head_dom::replace_canonical_link", |b| {
        b.iter(|| {
            black_box(replace_canonical_link(
                black_box(SAMPLE_HTML),
                black_box("<link rel=\"canonical\" href=\"https://x/\">"),
            ))
        });
    });

    c.bench_function("util::html_rewriter::decode_html_entities", |b| {
        b.iter(|| {
            black_box(decode_html_entities(black_box(
                "Tom &amp; Jerry &lt;3 entities &#39;quoted&#x27;",
            )))
        });
    });
    c.bench_function("util::html_rewriter::collapse_whitespace", |b| {
        b.iter(|| {
            black_box(collapse_whitespace(black_box(
                "  many   spaces\nand\ttabs  ",
            )))
        });
    });
    c.bench_function("util::html_rewriter::extract_text_with_filter", |b| {
        b.iter(|| {
            let _ = black_box(extract_text_with_filter(
                black_box(SAMPLE_HTML),
                black_box("main"),
            ));
        });
    });
    // SKIPPED: util::html_rewriter::rewrite_html — exposes lol_html
    //          handler types; benched indirectly via extract_text_with_filter
    //          which uses the same code path.
}

// ====================================================================
// group_server — server / hmr / livereload / watch / event_watch
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_server(c: &mut Criterion) {
    // event_watch: pure helpers + the value types.
    use notify::EventKind;
    use ssg::event_watch::{
        debounce_paths, event_should_propagate, ChangeBatch, RecvOutcome,
    };

    let kind_create = EventKind::Create(notify::event::CreateKind::File);
    c.bench_function("event_watch::event_should_propagate", |b| {
        b.iter(|| black_box(event_should_propagate(black_box(&kind_create))));
    });
    let events = vec![
        (PathBuf::from("a"), Instant::now()),
        (PathBuf::from("b"), Instant::now()),
    ];
    c.bench_function("event_watch::debounce_paths", |b| {
        b.iter(|| {
            black_box(debounce_paths(
                black_box(&events),
                black_box(Duration::from_millis(50)),
            ))
        });
    });
    let cb = ChangeBatch {
        paths: vec![PathBuf::from("a")],
    };
    c.bench_function("event_watch::ChangeBatch::is_empty", |b| {
        b.iter(|| black_box(cb.is_empty()));
    });
    c.bench_function("event_watch::ChangeBatch::len", |b| {
        b.iter(|| black_box(cb.len()));
    });
    let ro = RecvOutcome::Batch(cb.clone());
    c.bench_function("event_watch::RecvOutcome::is_closed", |b| {
        b.iter(|| black_box(ro.is_closed()));
    });
    c.bench_function("event_watch::RecvOutcome::batch", |b| {
        b.iter(|| black_box(ro.clone().batch()));
    });
    // SKIPPED: event_watch::EventWatcher::new / with_debounce / recv /
    //          recv_timeout / debounce — spawns a background thread per
    //          iter; verified indirectly by tests/server/*.

    // hmr — pure-string message constructors.
    use ssg::hmr::{HmrBroadcaster, HmrMessage, HmrType};
    let html_paths = vec!["a.html".to_string()];
    let css_paths = vec!["a.css".to_string()];
    c.bench_function("hmr::HmrMessage::css", |b| {
        b.iter(|| black_box(HmrMessage::css(css_paths.clone())));
    });
    c.bench_function("hmr::HmrMessage::html", |b| {
        b.iter(|| black_box(HmrMessage::html(html_paths.clone())));
    });
    c.bench_function("hmr::HmrMessage::reload", |b| {
        b.iter(|| black_box(HmrMessage::reload()));
    });
    let msg = HmrMessage::reload();
    c.bench_function("hmr::HmrType::wire", |b| {
        b.iter(|| black_box(HmrType::Reload.wire()));
    });
    let css_msg = HmrMessage::css(vec!["a.css".into()]);
    c.bench_function("hmr::HmrMessage::with_sha", |b| {
        b.iter(|| black_box(css_msg.clone().with_sha("abc")));
    });
    c.bench_function("hmr::HmrMessage::to_json", |b| {
        b.iter(|| black_box(msg.to_json()));
    });
    let bcast = HmrBroadcaster::new();
    c.bench_function("hmr::HmrBroadcaster::new", |b| {
        b.iter(|| black_box(HmrBroadcaster::new()));
    });
    c.bench_function("hmr::HmrBroadcaster::subscriber_count", |b| {
        b.iter(|| black_box(bcast.subscriber_count()));
    });
    // SKIPPED: hmr::HmrBroadcaster::subscribe / broadcast — needs a real
    //          HmrSink (a WebSocket peer); covered by tests/server/hmr*.

    // livereload
    use ssg::livereload::{css_reload_message, LiveReloadPlugin};
    c.bench_function("livereload::LiveReloadPlugin::new", |b| {
        b.iter(|| black_box(LiveReloadPlugin::new()));
    });
    c.bench_function("livereload::LiveReloadPlugin::with_port", |b| {
        b.iter(|| black_box(LiveReloadPlugin::with_port(8080)));
    });
    let lr = LiveReloadPlugin::new();
    c.bench_function("livereload::LiveReloadPlugin::port", |b| {
        b.iter(|| black_box(lr.port()));
    });
    c.bench_function("livereload::css_reload_message", |b| {
        b.iter(|| black_box(css_reload_message(black_box("a.css"))));
    });

    // watch
    use ssg::watch::{classify_change, FileWatcher, WatchConfig};
    let watch_path = PathBuf::from("a.html");
    c.bench_function("watch::classify_change", |b| {
        b.iter(|| black_box(classify_change(black_box(&watch_path))));
    });
    let watch_dir = tempfile::tempdir().unwrap();
    let watch_cfg = WatchConfig::new(
        watch_dir.path().to_path_buf(),
        Duration::from_millis(500),
    );
    c.bench_function("watch::WatchConfig::new", |b| {
        b.iter(|| {
            black_box(WatchConfig::new(
                watch_dir.path().to_path_buf(),
                Duration::from_millis(500),
            ))
        });
    });
    c.bench_function("watch::WatchConfig::directory", |b| {
        b.iter(|| black_box(watch_cfg.directory()));
    });
    c.bench_function("watch::WatchConfig::poll_interval", |b| {
        b.iter(|| black_box(watch_cfg.poll_interval()));
    });
    c.bench_function("watch::FileWatcher::new", |b| {
        b.iter(|| {
            let _ = black_box(FileWatcher::new(watch_cfg.clone()));
        });
    });
    let mut fw = FileWatcher::new(watch_cfg.clone()).unwrap();
    c.bench_function("watch::FileWatcher::config", |b| {
        b.iter(|| black_box(fw.config()));
    });
    c.bench_function("watch::FileWatcher::tracked_file_count", |b| {
        b.iter(|| black_box(fw.tracked_file_count()));
    });
    c.bench_function("watch::FileWatcher::check_for_changes", |b| {
        b.iter(|| {
            let _ = black_box(fw.check_for_changes());
        });
    });
    // SKIPPED: watch::watch_blocking — runs an infinite loop with a callback.

    // dev_server — process_batch / output_to_url / run_dev_loop.
    use ssg::dev_server::{output_to_url, process_batch};
    let dev_dir = tempfile::tempdir().unwrap();
    let output = dev_dir.path().join("public").join("index.html");
    c.bench_function("dev_server::output_to_url", |b| {
        b.iter(|| {
            black_box(output_to_url(
                black_box(&output),
                black_box(&dev_dir.path().join("public")),
            ))
        });
    });
    let batch = ChangeBatch { paths: Vec::new() };
    let dg_for_dev = ssg::depgraph::DepGraph::new();
    c.bench_function("dev_server::process_batch (empty)", |b| {
        b.iter(|| {
            black_box(process_batch(
                black_box(&batch),
                black_box(&dg_for_dev),
                black_box(dev_dir.path()),
            ))
        });
    });
    // SKIPPED: dev_server::run_dev_loop — infinite loop.

    // server: generate_locale_redirect + prepare_serve_dir.
    use ssg::server::{generate_locale_redirect, prepare_serve_dir};
    let srv_dir = tempfile::tempdir().unwrap();
    let locales = vec!["en".to_string(), "fr".to_string()];
    c.bench_function("server::generate_locale_redirect", |b| {
        b.iter(|| {
            let _ = black_box(generate_locale_redirect(
                black_box(srv_dir.path()),
                black_box(&locales),
                black_box("en"),
            ));
        });
    });
    let srv_paths = ssg::Paths {
        site: srv_dir.path().to_path_buf(),
        content: srv_dir.path().to_path_buf(),
        build: srv_dir.path().to_path_buf(),
        template: srv_dir.path().to_path_buf(),
    };
    let serve_dir = srv_dir.path().to_path_buf();
    c.bench_function("server::prepare_serve_dir", |b| {
        b.iter(|| {
            let _ = black_box(prepare_serve_dir(
                black_box(&srv_paths),
                black_box(&serve_dir),
            ));
        });
    });
    // SKIPPED: server::serve_site / serve_site_with — spawns the HTTP server.
    // SKIPPED: server::handle_server — blocks waiting for clients.
}

// ====================================================================
// group_ssg_core — content_provider / isr_manifest / lib
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_ssg_core(c: &mut Criterion) {
    use ssg_core::{
        build_entry, build_search_entry, compile_markdown, compile_page,
        hash_sources, parse_frontmatter, reading_time, slugify,
        strip_html_tags, CachePolicy, FsContentProvider, Manifest,
        MemoryContentProvider,
    };

    // Free functions in lib.rs.
    c.bench_function("ssg_core::compile_markdown", |b| {
        b.iter(|| black_box(compile_markdown(black_box(SAMPLE_MARKDOWN))));
    });
    c.bench_function("ssg_core::parse_frontmatter", |b| {
        b.iter(|| {
            black_box(parse_frontmatter(black_box("---\ntitle: A\n---\nbody")))
        });
    });
    c.bench_function("ssg_core::compile_page", |b| {
        b.iter(|| {
            let _ =
                black_box(compile_page(black_box("---\ntitle: A\n---\nbody")));
        });
    });
    c.bench_function("ssg_core::strip_html_tags", |b| {
        b.iter(|| {
            black_box(strip_html_tags(black_box("<p>Hello <b>world</b></p>")))
        });
    });
    c.bench_function("ssg_core::build_search_entry", |b| {
        b.iter(|| {
            black_box(build_search_entry(
                black_box("Title"),
                black_box("/u"),
                black_box("<p>x</p>"),
            ))
        });
    });
    c.bench_function("ssg_core::reading_time", |b| {
        b.iter(|| {
            black_box(reading_time(black_box("one two three four five six")))
        });
    });
    c.bench_function("ssg_core::slugify", |b| {
        b.iter(|| black_box(slugify(black_box("Hello World!"))));
    });

    // isr_manifest module.
    c.bench_function("ssg_core::hash_sources", |b| {
        b.iter(|| black_box(hash_sources(black_box(&[b"hello", b"world"]))));
    });
    c.bench_function("ssg_core::build_entry", |b| {
        b.iter(|| {
            black_box(build_entry(
                black_box(vec!["a.md".into()]),
                black_box(&[b"hello"]),
                black_box(None),
            ))
        });
    });
    let policy = CachePolicy {
        s_maxage: 60,
        swr: 600,
    };
    c.bench_function("ssg_core::CachePolicy::to_cache_control", |b| {
        b.iter(|| black_box(policy.to_cache_control()));
    });

    let mut m = Manifest::new("build-bench");
    c.bench_function("ssg_core::Manifest::new", |b| {
        b.iter(|| black_box(Manifest::new(black_box("b"))));
    });
    let entry = build_entry(vec!["a.md".into()], &[b"a"], None);
    c.bench_function("ssg_core::Manifest::insert", |b| {
        b.iter(|| {
            m.insert(black_box("/a.html"), black_box(entry.clone()));
            black_box(())
        });
    });
    c.bench_function("ssg_core::Manifest::get", |b| {
        b.iter(|| black_box(m.get(black_box("/a.html"))));
    });
    c.bench_function("ssg_core::Manifest::len", |b| {
        b.iter(|| black_box(m.len()));
    });
    c.bench_function("ssg_core::Manifest::is_empty", |b| {
        b.iter(|| black_box(m.is_empty()));
    });
    c.bench_function("ssg_core::Manifest::to_pretty_json", |b| {
        b.iter(|| {
            let _ = black_box(m.to_pretty_json());
        });
    });
    c.bench_function("ssg_core::Manifest::urls_for_source", |b| {
        b.iter(|| black_box(m.urls_for_source(black_box("a.md"))));
    });

    // ContentProviders.
    let mut mem = MemoryContentProvider::new();
    let _ = mem.insert("a.md", b"hello".to_vec());
    c.bench_function("ssg_core::MemoryContentProvider::new", |b| {
        b.iter(|| black_box(MemoryContentProvider::new()));
    });
    c.bench_function("ssg_core::MemoryContentProvider::insert", |b| {
        b.iter(|| {
            let mut m = MemoryContentProvider::new();
            let _ = m.insert(black_box("a.md"), black_box(b"x".to_vec()));
            black_box(m)
        });
    });
    c.bench_function("ssg_core::MemoryContentProvider::len", |b| {
        b.iter(|| black_box(mem.len()));
    });
    c.bench_function("ssg_core::MemoryContentProvider::is_empty", |b| {
        b.iter(|| black_box(mem.is_empty()));
    });

    let fs_tmp = tempfile::tempdir().unwrap();
    std::fs::write(fs_tmp.path().join("hello.md"), b"# Hi").unwrap();
    let fs_prov = FsContentProvider::new(fs_tmp.path());
    c.bench_function("ssg_core::FsContentProvider::new", |b| {
        b.iter(|| black_box(FsContentProvider::new(fs_tmp.path())));
    });
    c.bench_function("ssg_core::FsContentProvider::root", |b| {
        b.iter(|| black_box(fs_prov.root()));
    });
}

// ====================================================================
// group_ssg_search — encoder / engine / artifacts / manifest / quantize
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_ssg_search(c: &mut Criterion) {
    use ssg_search::artifacts::{Artifacts, ArtifactsBuilder, InputDoc};
    use ssg_search::encoder::{
        deserialize_projection_encoder, Encoder, ProjectionConfig,
        ProjectionEncoder,
    };
    use ssg_search::engine::VectorEngine;
    use ssg_search::quantize::{dequantize_int8, quantize_int8};
    use ssg_search::{Manifest, ManifestEntry};

    // encoder
    let cfg = ProjectionConfig::default();
    c.bench_function("ssg_search::ProjectionEncoder::new", |b| {
        b.iter(|| black_box(ProjectionEncoder::new(cfg)));
    });
    let enc = ProjectionEncoder::default();
    c.bench_function("ssg_search::ProjectionEncoder::config", |b| {
        b.iter(|| black_box(enc.config()));
    });
    c.bench_function("ssg_search::ProjectionEncoder::seed", |b| {
        b.iter(|| black_box(enc.seed()));
    });
    c.bench_function("ssg_search::Encoder::dim", |b| {
        b.iter(|| black_box(<ProjectionEncoder as Encoder>::dim(&enc)));
    });
    c.bench_function("ssg_search::Encoder::embed", |b| {
        b.iter(|| {
            black_box(<ProjectionEncoder as Encoder>::embed(
                &enc,
                black_box("the quick brown fox"),
            ))
        });
    });
    let model_bytes = <ProjectionEncoder as Encoder>::serialize_model(&enc);
    let tok_bytes = <ProjectionEncoder as Encoder>::serialize_tokenizer(&enc);
    c.bench_function("ssg_search::Encoder::serialize_model", |b| {
        b.iter(|| {
            black_box(<ProjectionEncoder as Encoder>::serialize_model(&enc))
        });
    });
    c.bench_function("ssg_search::Encoder::serialize_tokenizer", |b| {
        b.iter(|| {
            black_box(<ProjectionEncoder as Encoder>::serialize_tokenizer(&enc))
        });
    });
    c.bench_function("ssg_search::deserialize_projection_encoder", |b| {
        b.iter(|| {
            black_box(deserialize_projection_encoder(black_box(&model_bytes)))
        });
    });

    // engine
    let docs = vec![InputDoc {
        url: "/a".into(),
        title: "A".into(),
        body: "hello world".into(),
        excerpt: "x".into(),
    }];
    let arts = Artifacts::from_docs(&docs);
    let arts_count = arts.count();
    let engine = VectorEngine::new(
        &arts.model,
        &tok_bytes,
        &arts.embeddings,
        arts_count,
    )
    .unwrap();
    c.bench_function("ssg_search::VectorEngine::new", |b| {
        b.iter(|| {
            let _ = black_box(VectorEngine::new(
                black_box(&arts.model),
                black_box(&tok_bytes),
                black_box(&arts.embeddings),
                black_box(arts_count),
            ));
        });
    });
    c.bench_function("ssg_search::VectorEngine::encoder", |b| {
        b.iter(|| black_box(engine.encoder()));
    });
    c.bench_function("ssg_search::VectorEngine::dim", |b| {
        b.iter(|| black_box(engine.dim()));
    });
    c.bench_function("ssg_search::VectorEngine::count", |b| {
        b.iter(|| black_box(engine.count()));
    });
    c.bench_function("ssg_search::VectorEngine::corpus", |b| {
        b.iter(|| black_box(engine.corpus()));
    });
    c.bench_function("ssg_search::VectorEngine::embed_query", |b| {
        b.iter(|| black_box(engine.embed_query(black_box("hi"))));
    });
    let qv = engine.embed_query("hi");
    c.bench_function("ssg_search::VectorEngine::search_vec", |b| {
        b.iter(|| black_box(engine.search_vec(black_box(&qv), black_box(3))));
    });
    c.bench_function("ssg_search::VectorEngine::search", |b| {
        b.iter(|| black_box(engine.search(black_box("hi"), black_box(3))));
    });

    // artifacts
    c.bench_function("ssg_search::Artifacts::from_docs", |b| {
        b.iter(|| black_box(Artifacts::from_docs(black_box(&docs))));
    });
    c.bench_function("ssg_search::Artifacts::dim", |b| {
        b.iter(|| black_box(arts.dim()));
    });
    c.bench_function("ssg_search::Artifacts::count", |b| {
        b.iter(|| black_box(arts.count()));
    });
    let mut ab = ArtifactsBuilder::new(ProjectionEncoder::default());
    c.bench_function("ssg_search::ArtifactsBuilder::new", |b| {
        b.iter(|| {
            black_box(ArtifactsBuilder::new(ProjectionEncoder::default()))
        });
    });
    c.bench_function("ssg_search::ArtifactsBuilder::add_doc", |b| {
        b.iter(|| {
            let _ = ab.add_doc(black_box(docs[0].clone()));
            black_box(())
        });
    });
    c.bench_function("ssg_search::ArtifactsBuilder::len", |b| {
        b.iter(|| black_box(ab.len()));
    });
    c.bench_function("ssg_search::ArtifactsBuilder::is_empty", |b| {
        b.iter(|| black_box(ab.is_empty()));
    });
    c.bench_function("ssg_search::ArtifactsBuilder::encoder", |b| {
        b.iter(|| black_box(ab.encoder()));
    });
    c.bench_function("ssg_search::ArtifactsBuilder::build", |b| {
        b.iter(|| {
            let ab = ArtifactsBuilder::new(ProjectionEncoder::default());
            black_box(ab.build())
        });
    });

    // manifest
    let entries = vec![ManifestEntry {
        url: "/a".into(),
        title: "A".into(),
        excerpt: "x".into(),
    }];
    c.bench_function("ssg_search::Manifest::new", |b| {
        b.iter(|| {
            black_box(Manifest::new(
                black_box(32),
                black_box("h".into()),
                black_box(entries.clone()),
            ))
        });
    });
    let mf = Manifest::new(32, "h".into(), entries);
    c.bench_function("ssg_search::Manifest::is_valid", |b| {
        b.iter(|| black_box(mf.is_valid()));
    });

    // quantize
    let v: Vec<f32> = (0..64).map(|i| (i as f32) / 64.0).collect();
    c.bench_function("ssg_search::quantize_int8", |b| {
        b.iter(|| black_box(quantize_int8(black_box(&v))));
    });
    let q = quantize_int8(&v);
    c.bench_function("ssg_search::dequantize_int8", |b| {
        b.iter(|| black_box(dequantize_int8(black_box(&q))));
    });
}

// ====================================================================
// group_ssg_rpc — lib / dispatch / schema / ts
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_ssg_rpc(c: &mut Criterion) {
    use ssg_rpc::dispatch::{
        dispatch, find, iter_descriptors, registered_names,
    };
    use ssg_rpc::schema::{schema_for, schema_for_result};
    use ssg_rpc::ts::{emit_typescript, emit_typescript_for, EmitOptions};
    use ssg_rpc::RpcError;

    // lib (RpcError).
    let err_br = RpcError::BadRequest("x".into());
    c.bench_function("ssg_rpc::RpcError::status_code", |b| {
        b.iter(|| black_box(err_br.status_code()));
    });
    c.bench_function("ssg_rpc::RpcError::to_wire_body", |b| {
        b.iter(|| black_box(err_br.to_wire_body()));
    });

    // dispatch — the `echo` descriptor is registered into the inventory
    // by ssg-rpc's own test module at link time, so these benches always
    // see at least one descriptor.
    c.bench_function("ssg_rpc::dispatch::iter_descriptors", |b| {
        b.iter(|| black_box(iter_descriptors().count()));
    });
    c.bench_function("ssg_rpc::dispatch::registered_names", |b| {
        b.iter(|| black_box(registered_names()));
    });
    c.bench_function("ssg_rpc::dispatch::find", |b| {
        b.iter(|| black_box(find(black_box("echo"))));
    });
    c.bench_function("ssg_rpc::dispatch::dispatch (unknown)", |b| {
        b.iter(|| {
            let _ = black_box(dispatch(black_box("nope"), black_box("{}")));
        });
    });

    // schema
    c.bench_function("ssg_rpc::schema::schema_for::<String>", |b| {
        b.iter(|| black_box(schema_for::<String>()));
    });
    c.bench_function(
        "ssg_rpc::schema::schema_for_result::<Result<String, RpcError>>",
        |b| {
            b.iter(|| {
                black_box(schema_for_result::<Result<String, RpcError>>())
            });
        },
    );

    // ts
    let opts = EmitOptions::default();
    c.bench_function("ssg_rpc::ts::EmitOptions::default", |b| {
        b.iter(|| black_box(EmitOptions::default()));
    });
    c.bench_function("ssg_rpc::ts::emit_typescript", |b| {
        b.iter(|| black_box(emit_typescript(black_box(&opts))));
    });
    let schemas: Vec<ssg_rpc::RpcSchema> = Vec::new();
    c.bench_function("ssg_rpc::ts::emit_typescript_for", |b| {
        b.iter(|| {
            black_box(emit_typescript_for(
                black_box(&schemas),
                black_box(&opts),
            ))
        });
    });
}

// ====================================================================
// group_lib_root — top-level ssg::* functions
// ====================================================================

#[allow(unreachable_pub)]
pub fn bench_lib_root(c: &mut Criterion) {
    use ssg::{create_directories, now_iso, Paths, PathsBuilder};

    c.bench_function("ssg::now_iso", |b| {
        b.iter(|| black_box(now_iso()));
    });
    c.bench_function("ssg::Paths::default_paths", |b| {
        b.iter(|| black_box(Paths::default_paths()));
    });
    c.bench_function("ssg::Paths::builder", |b| {
        b.iter(|| black_box(Paths::builder()));
    });
    let paths = Paths::default_paths();
    c.bench_function("ssg::Paths::validate", |b| {
        b.iter(|| {
            let _ = black_box(paths.validate());
        });
    });
    c.bench_function(
        "ssg::PathsBuilder::site+content+build+template+build",
        |b| {
            b.iter(|| {
                let _ = black_box(
                    PathsBuilder::default()
                        .site("out")
                        .content("c")
                        .build_dir("b")
                        .template("t")
                        .build(),
                );
            });
        },
    );
    c.bench_function("ssg::PathsBuilder::relative_to", |b| {
        b.iter(|| {
            let _ = black_box(
                PathsBuilder::default()
                    .relative_to(black_box("base"))
                    .build(),
            );
        });
    });
    // create_directories writes — use a fresh tempdir per iter so the
    // benchmark isn't measuring the cost of mkdir() against a fully
    // primed inode cache.
    c.bench_function("ssg::create_directories", |b| {
        b.iter(|| {
            let tmp = tempfile::tempdir().unwrap();
            let p = Paths {
                site: tmp.path().join("public"),
                content: tmp.path().join("content"),
                build: tmp.path().join("build"),
                template: tmp.path().join("templates"),
            };
            let _ = black_box(create_directories(black_box(&p)));
        });
    });
    // SKIPPED: ssg::run — boots the CLI dispatcher; only callable
    //          from real `main()`.
}

// ====================================================================
// Criterion entry points — one group per logical area.
// ====================================================================

criterion_group!(group_audit_gates, bench_audit_gates);
criterion_group!(group_audit_output, bench_audit_output);
criterion_group!(group_audit_runner, bench_audit_runner);
criterion_group!(group_cmd, bench_cmd);
criterion_group!(group_core, bench_core);
criterion_group!(group_plugins_seo, bench_plugins_seo);
criterion_group!(group_plugins_jsonld_iso20022, bench_plugins_jsonld_iso20022);
criterion_group!(
    group_plugins_postprocess_agentic,
    bench_plugins_postprocess_agentic
);
criterion_group!(
    group_plugins_postprocess_edge_headers,
    bench_plugins_postprocess_edge_headers
);
criterion_group!(
    group_plugins_view_transitions,
    bench_plugins_view_transitions
);
criterion_group!(group_plugins_misc, bench_plugins_misc);
criterion_group!(group_plugins_agent_surfaces, bench_plugins_agent_surfaces);
criterion_group!(group_util, bench_util);
criterion_group!(group_server, bench_server);
criterion_group!(group_ssg_core, bench_ssg_core);
criterion_group!(group_ssg_search, bench_ssg_search);
criterion_group!(group_ssg_rpc, bench_ssg_rpc);
criterion_group!(group_lib_root, bench_lib_root);

criterion_main!(
    group_audit_gates,
    group_audit_output,
    group_audit_runner,
    group_cmd,
    group_core,
    group_plugins_seo,
    group_plugins_jsonld_iso20022,
    group_plugins_postprocess_agentic,
    group_plugins_postprocess_edge_headers,
    group_plugins_view_transitions,
    group_plugins_misc,
    group_plugins_agent_surfaces,
    group_util,
    group_server,
    group_ssg_core,
    group_ssg_search,
    group_ssg_rpc,
    group_lib_root,
);
