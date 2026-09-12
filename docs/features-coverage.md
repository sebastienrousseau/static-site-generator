<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Features × Coverage Matrix

Every user-facing feature in v0.0.47 is exercised by at least one
**example**, one **benchmark**, and one **regression test**. The matrix
below is the source of truth — and the
`tests/docs_accuracy.rs::features_matrix_is_exhaustive` test fails the
build if a new top-level plugin module ships without an entry here.

Updating this file: when you add a new plugin under
`src/plugins/<name>.rs`, add a row whose left-hand cell matches the
module name. The test reads `src/plugins/*.rs` and the table below; a
plugin missing from the table fires the assertion with the offending
name.

| Plugin / Feature | Module | Example | Benchmark | Integration test |
|---|---|---|---|---|
| Accessibility (WCAG 2.2 + EAA) | `accessibility` | [`examples/blog`](../examples/blog_example.rs), [`examples/landing`](../examples/landing_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/audit_gates.rs`](../tests/audit_gates.rs), [`tests/element_presence.rs`](../tests/element_presence.rs) — the WCAG rule-checking logic itself now lives in the standalone [`crates/ssg-a11y`](../crates/ssg-a11y) crate, unit-tested in-crate (`cargo test -p ssg-a11y`); `src/plugins/accessibility.rs` is a thin wrapper covered by its own `#[cfg(test)] mod tests` (`cargo test --lib -- plugins_group::accessibility`) |
| Agent JSON API (`/api/agents/*.json`, #586 port 3) | `agent_api` | [`examples/agent_api`](../examples/agent_api_example.rs), [`examples/blog`](../examples/blog_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugins/agent_api.rs`](../tests/plugins/agent_api.rs) (via plugins/ submodule) |
| AI metadata + alt-text | `ai` | [`examples/agentic_discovery`](../examples/agentic_discovery_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/agentic_discovery.rs`](../tests/agentic_discovery.rs) |
| Asset fingerprinting + SRI | `assets` | [`examples/blog`](../examples/blog_example.rs) | [`benches/bench_concurrent_operations.rs`](../benches/bench_concurrent_operations.rs) | [`tests/regression.rs`](../tests/regression.rs) |
| CSP build-time extraction | `csp` | [`examples/blog`](../examples/blog_example.rs), [`examples/landing`](../examples/landing_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/csp_preserve_attrs.rs`](../tests/csp_preserve_attrs.rs), [`tests/audit_gates.rs`](../tests/audit_gates.rs) |
| Draft filtering | `drafts` | [`examples/blog`](../examples/blog_example.rs), [`examples/portfolio`](../examples/portfolio_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugins/drafts.rs`](../tests/plugins/drafts.rs) (via plugins/ submodule) |
| Syntax highlighting | `highlight` | [`examples/basic`](../examples/basic_example.rs), [`examples/docs`](../examples/docs_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/regression.rs`](../tests/regression.rs) |
| Internationalisation | `i18n` | [`examples/multilingual`](../examples/multilingual_example.rs), [`examples/multilingual_full`](../examples/multilingual_full_example.rs) (nested `content/<lang>/` tree; exercises `staticdatagen 0.0.10` recursive walk) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/regression.rs`](../tests/regression.rs), [`tests/regression_user_site.rs::nested_locale_subdirectories_build_per_language`](../tests/regression_user_site.rs) (currently `#[ignore]`d pending the `staticdatagen 0.0.10` dep bump on `feat/v0.0.46`) |
| Responsive image pipeline | `image_plugin` | [`examples/blog`](../examples/blog_example.rs), [`examples/portfolio`](../examples/portfolio_example.rs) | [`benches/avif_vs_webp.rs`](../benches/avif_vs_webp.rs) | [`tests/plugins/image_plugin.rs`](../tests/plugins/image_plugin.rs) |
| Web-Components islands | `islands` | [`examples/docs`](../examples/docs_example.rs), [`examples/landing`](../examples/landing_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/regression.rs`](../tests/regression.rs) |
| ISR manifest emission | `isr_manifest` | [`examples/isr`](../examples/isr_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/isr_manifest_shape.rs`](../tests/isr_manifest_shape.rs), [`tests/isr_edge_contract.rs`](../tests/isr_edge_contract.rs), [`tests/isr_backcompat.rs`](../tests/isr_backcompat.rs) |
| Local-LLM content pipeline | `llm` | [`examples/agentic_discovery`](../examples/agentic_discovery_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/llm_no_shellout.rs`](../tests/llm_no_shellout.rs) |
| LLM inference cache | `llm_cache` | (transitive via `llm`) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/llm_cache.rs`](../tests/llm_cache.rs) |
| GFM Markdown extensions | `markdown_ext` | [`examples/blog`](../examples/blog_example.rs) | [`benches/bench_utilities.rs`](../benches/bench_utilities.rs) | [`tests/regression.rs`](../tests/regression.rs) |
| oEmbed 1.0 documents + discovery link (#586 port 4) | `oembed` | [`examples/agent_api`](../examples/agent_api_example.rs), [`examples/blog`](../examples/blog_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugins/oembed.rs`](../tests/plugins/oembed.rs) (via plugins/ submodule) |
| OG social-card images | `og_image` | [`examples/blog`](../examples/blog_example.rs) | [`benches/plugins/`](../benches/plugins/) | [`tests/audit_gates.rs`](../tests/audit_gates.rs) |
| Pagination | `pagination` | [`examples/blog`](../examples/blog_example.rs), [`examples/docs`](../examples/docs_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugin_contracts.rs`](../tests/plugin_contracts.rs) |
| Plugin trait + lifecycle | `plugin` | [`examples/plugins`](../examples/plugins_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugin_contracts.rs`](../tests/plugin_contracts.rs) |
| Plugin registry | `plugins` | [`examples/plugins`](../examples/plugins_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugin_contracts.rs`](../tests/plugin_contracts.rs) |
| Post-process (RSS/Atom/Sitemap/Manifest/HTML-Fix) | `postprocess` | [`examples/blog`](../examples/blog_example.rs), [`examples/docs`](../examples/docs_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/json_feed_compliance.rs`](../tests/json_feed_compliance.rs), [`tests/regression.rs`](../tests/regression.rs) |
| RPC schema emitter | `rpc_schema` | [`examples/rpc`](../examples/rpc_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/regression.rs`](../tests/regression.rs) |
| CycloneDX SBOM | `sbom` | (every build) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/audit_gates.rs`](../tests/audit_gates.rs) |
| Search widget + index | `search` | [`examples/search`](../examples/search_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/search_index_integrity.rs`](../tests/search_index_integrity.rs) |
| Search-index emitter (`VectorSearchPlugin`) | `search_index` | [`examples/blog`](../examples/blog_example.rs) (registers the plugin), [`examples/search`](../examples/search_example.rs) (consumes the artifacts via `ssg-search`) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/search_index_integrity.rs`](../tests/search_index_integrity.rs) |
| SEO + JSON-LD + canonical + robots | `seo` | [`examples/portfolio`](../examples/portfolio_example.rs), [`examples/iso20022`](../examples/iso20022_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/seo_canonical_lol_html.rs`](../tests/seo_canonical_lol_html.rs), [`tests/seo_extractors_lol_html.rs`](../tests/seo_extractors_lol_html.rs), [`tests/jsonld_validation.rs`](../tests/jsonld_validation.rs), [`tests/jsonld_iso20022.rs`](../tests/jsonld_iso20022.rs) |
| Shortcodes | `shortcodes` | [`examples/landing`](../examples/landing_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/regression.rs`](../tests/regression.rs) |
| Taxonomy (tags + categories + per-term landing pages, #586 port 5) | `taxonomy` | [`examples/blog`](../examples/blog_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/taxonomy_templated.rs`](../tests/taxonomy_templated.rs), [`tests/test_tags.rs`](../tests/test_tags.rs) |
| Listings (named, filtered, paginated archives + year archives; #587) | `listings` | [`examples/blog`](../examples/blog_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugins/pagination.rs`](../tests/plugins/pagination.rs) |
| Topic curation (`_data/topics.toml` — pillar-page title, lede, banner, order; #587) | `topic_clusters` | [`examples/blog`](../examples/blog_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/taxonomy_templated.rs`](../tests/taxonomy_templated.rs) |
| Template engine (MiniJinja) | `template_engine` | [`examples/basic`](../examples/basic_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/template_data_yaml.rs`](../tests/template_data_yaml.rs) |
| Template plugin | `template_plugin` | [`examples/basic`](../examples/basic_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugin_contracts.rs`](../tests/plugin_contracts.rs) |
| View Transitions API | `view_transitions` | [`examples/view_transitions`](../examples/view_transitions_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/view_transitions_plugin.rs`](../tests/view_transitions_plugin.rs) |
| Edge-headers emitter | `postprocess::edge_headers` | [`examples/edge_headers`](../examples/edge_headers_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/edge_headers_emit.rs`](../tests/edge_headers_emit.rs) |
| Agentic discovery (agents.txt + MCP) | `postprocess::agentic_discovery` | [`examples/agentic_discovery`](../examples/agentic_discovery_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/agentic_discovery.rs`](../tests/agentic_discovery.rs) |
| Audit CLI (15 gates + SARIF) | `audit` | [`examples/audit`](../examples/audit_example.rs) | [`benches/bench_audit.rs`](../benches/bench_audit.rs) | [`tests/audit_gates.rs`](../tests/audit_gates.rs), [`tests/audit_perf.rs`](../tests/audit_perf.rs) |

## Non-plugin features

| Feature | Source-of-truth | Example | Benchmark | Integration test |
|---|---|---|---|---|
| Scaffold (`ssg --new`) | `src/core/scaffold.rs` | [`examples/quickstart`](../examples/quickstart_example.rs) | n/a (one-shot) | [`tests/golden_files.rs`](../tests/golden_files.rs) (×10 golden files) |
| Dev server + HMR | `src/server/` | [`examples/landing`](../examples/landing_example.rs) (uses `-w`) | n/a (interactive) | [`tests/server/`](../tests/server/) |
| Incremental compilation | `src/core/depgraph.rs` | (every example with `-w`) | [`benches/incremental_1000_pages.rs`](../benches/incremental_1000_pages.rs) | [`tests/incremental_correctness.rs`](../tests/incremental_correctness.rs) |
| Bounded-memory batch compilation (≥ 8K pages) | `src/core/streaming.rs` | (use `--max-memory` on bench corpus) | [`benches/bench_scalability.rs`](../benches/bench_scalability.rs) | [`tests/regression.rs`](../tests/regression.rs) |
| Path-safety | `src/core/fs_ops.rs::is_safe_path` | n/a (defensive) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/build_does_not_mutate_sources.rs`](../tests/build_does_not_mutate_sources.rs), [`tests/chaos.rs`](../tests/chaos.rs) |
| LLM HTTP client (`ureq`, no shellout) | `src/plugins/llm.rs` | [`examples/agentic_discovery`](../examples/agentic_discovery_example.rs) | n/a (network) | [`tests/llm_no_shellout.rs`](../tests/llm_no_shellout.rs) |
| Content-staging shim (workaround for upstream regressions) | `src/core/content_stager.rs` | (every build) | n/a (pre-pass) | [`tests/regression_user_site.rs`](../tests/regression_user_site.rs) |
| Fault-injection failpoints | `fail = "0.5"` | n/a | n/a | [`tests/fault_injection.rs`](../tests/fault_injection.rs) (×8 failpoints) |
| Subcommand surface (`ssg dev`/`build`/`check`/`audit`/`deploy`) | `src/cmd/cli.rs` | each example wraps one subcommand | n/a | [`tests/cli_subcommands.rs`](../tests/cli_subcommands.rs) |
| Flexible date parsing (spec A4, #586) | `src/core/dates.rs` | [`examples/quickstart`](../examples/quickstart_example.rs) (`cupping-notes-july.md` — long-form `date`, minimal front matter) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/core/dates.rs`](../tests/core/dates.rs) |
| Permalink / feed-link derivation (spec A2/B1, #586) | `src/core/urls.rs`, `src/core/content_stager.rs` | [`examples/quickstart`](../examples/quickstart_example.rs) (no `permalink:` declared; derived feed link asserted post-build) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/core/urls.rs`](../tests/core/urls.rs), [`tests/core/pipeline.rs`](../tests/core/pipeline.rs), [`tests/core/content_stager.rs`](../tests/core/content_stager.rs) |
| `[security] sri_algorithm` knob (plan §3 item 2.3) | `src/cmd/config.rs::SriAlgorithm` | [`examples/edge_headers`](../examples/edge_headers_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/audit_gates.rs`](../tests/audit_gates.rs) (csp_sri gate), unit tests in [`src/plugins/assets.rs`](../src/plugins/assets.rs) |
| Per-page CSP → edge `_headers` (spec B4, plan §3 item 2.4) | `src/plugins/csp.rs::page_policy`, `postprocess::edge_headers` | [`examples/edge_headers`](../examples/edge_headers_example.rs) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/plugins/postprocess/edge_headers.rs`](../tests/plugins/postprocess/edge_headers.rs) |
| Buffered I/O write pool | `src/core/io_pool.rs` | (every build — fused transform pass writes through the pool) | [`benches/all_pub_api.rs`](../benches/all_pub_api.rs) | [`tests/fault_injection.rs`](../tests/fault_injection.rs) |
| Language-consistency audit gate (spec A5) | `src/audit/gates/lang_consistency.rs` | [`examples/audit`](../examples/audit_example.rs) (runs the full gate set) | [`benches/bench_audit.rs`](../benches/bench_audit.rs) | [`tests/audit_gates.rs`](../tests/audit_gates.rs) |

## CI gates (v0.0.45 additions)

| Gate | Workflow | Resolves |
|---|---|---|
| `repo-hygiene` (no stray profraw, no tracked profraw) | [`ci.yml`](../.github/workflows/ci.yml) | #556 |
| `no-shellout` lint (`Command::new("curl"\|"wget"\|…)` in `src/`) | [`ci.yml`](../.github/workflows/ci.yml) | #558 |
| `ADR citation graph` (every `adr: ADR-NNNN` resolves to `docs/adr/`) | [`ci.yml`](../.github/workflows/ci.yml) | #557 |
| `feature powerset` (`cargo hack check --feature-powerset --depth 2`) | [`ci.yml`](../.github/workflows/ci.yml) | #584 |
| `cargo-vet` | [`ci.yml`](../.github/workflows/ci.yml) | #561 |
| `Miri` (schedule + `run-miri` label) | [`miri.yml`](../.github/workflows/miri.yml) | #560 |
| SARIF upload to GitHub Code Scanning | [`ci.yml`](../.github/workflows/ci.yml) | #562 |
| Coverage floor 95.5 / 95.5 / 96.5 (regions / functions / lines) | [`ci.yml`](../.github/workflows/ci.yml) | (v0.0.45 lift, ~+0.45 over baseline) |

## See also

- [`BENCHMARKS.md`](../BENCHMARKS.md) — perf-gate budgets + cross-SSG comparison
- [`docs/adr/`](adr/) — Architecture Decision Records
- [`supply-chain/README.md`](../supply-chain/README.md) — `cargo-vet` policy
- [`SECURITY.md`](../SECURITY.md) — threat model + security defaults
