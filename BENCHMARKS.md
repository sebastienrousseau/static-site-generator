<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Benchmarks

Reproducible performance methodology and CI gates for SSG **v0.0.69**.

> **Figures last measured at v0.0.58.** The methodology and the CI gates
> below apply to this release; the numbers under _Current_ were carried
> forward from that measurement. 0.0.62 **does** change a hot path these
> cover: `plugins::minify_css`, `plugins::minify_js` and
> `plugins::minify_html` are ssg's own code now rather than
> `lightningcss`, `oxc_minifier` and `minify-html`, and the HTML pass
> tracks element context instead of collapsing the whole document. The
> minify benches in `all_pub_api` measure the new implementations
> directly; every other figure below is unaffected. Re-measuring the full
> set is tracked in #559.

This document is **load-bearing**: the docs-accuracy gate
(`tests/docs_accuracy.rs`) parses the tables below for the
coverage-floor row, the CI-runner row, and the test-count row. Updating
those values in source-of-truth files (Cargo.toml, ci.yml) requires the
same update here or the gate fires.

---

## Quick Start

```bash
# Criterion micro-benchmarks (10 bench targets, ~5 min)
cargo bench

# CI performance gates (hard wall-clock budgets, ~1 min)
cargo test --test perf_regression

# 100-page cold-build budget
cargo test --test perf_budgets

# Cross-SSG comparison vs Hugo / Zola / Eleventy (#559)
# hyperfine wrappers over benches/corpus/small, median-of-10;
# each script exits 0 with a notice if the competitor binary is absent.
./tools/bench-vs-hugo.sh
./tools/bench-vs-zola.sh
./tools/bench-vs-eleventy.sh
```

The `make coverage` target runs the same `cargo llvm-cov --lib` that
CI's `coverage gate` job runs, so local + CI numbers match.

---

## CI Performance Gates (perf_regression.rs)

Hard budgets enforced on every PR. A regression beyond the budget
blocks the merge.

| Operation | Budget | Typical | Test |
|-----------|--------|---------|------|
| 100-page compilation | < 5 s | ~200 ms | `compile_100_pages_under_5s` |
| 50-page search index | < 500 ms | ~50 ms | `search_index_50_pages_under_50ms` |
| 50-page SEO injection | < 500 ms | ~30 ms | `seo_plugin_50_pages_under_50ms` |
| 50-page a11y audit | < 500 ms | ~20 ms | `accessibility_check_50_pages_under_50ms` |
| 1000-file cache fingerprint | < 500 ms | ~40 ms | `cache_fingerprint_1000_files_under_50ms` |
| 1000-file stream hash | < 500 ms | ~50 ms | `stream_hash_1000_files_under_50ms` |
| 10K depgraph invalidation | < 200 ms | ~10 ms | `depgraph_10k_entries_under_50ms` |
| 100K budget calculations | < 10 ms | ~1 ms | `memory_budget_calculation_instant` |

Budgets are deliberately ~5–10x looser than typical wall-clock so a
slow CI runner doesn't false-positive the gate. See the runner sizing
table at the end of this document.

---

## End-to-End Build Budgets (perf_budgets.rs)

Run on every CI build via the `examples` job:

| Corpus | Budget | Typical | Test |
|-------:|-------:|--------:|------|
| 10 pages, cold | < 1 s | ~80 ms | `cold_build_10_pages_under_1s` |
| 100 pages, cold | < 5 s | ~300 ms | `cold_build_100_pages_under_5s` |
| 100 pages, incremental (warm) | < 200 ms | ~30 ms | `incremental_100_pages_under_200ms` |

The 100-page incremental warm-cache target is the v0.0.47 `--incremental`
acceptance criterion (#568) — once the v0.0.47 IoPool work lands the
budget will tighten to **≤ 50 ms** for the warm cache.

---

## Bounded-Memory Batch Compilation Thresholds

For sites exceeding the memory budget, SSG compiles in bounded-memory
batches (batch-by-batch to disk — peak memory is bounded by the batch
size, not the site size):

| Pages | Mode | Memory Budget | Disk Overhead |
|------:|------|---------------|---------------|
| < 8,000 | In-memory | Default (≤ 2 GB heap) | None |
| 8,000+ | Bounded-memory batches | 512 MB (default) | ~3× output size (temp dir) |
| 100,000+ | Bounded-memory batches | Configurable via `--max-memory` | ~3× output size |

Batch mode adds ~10 % wall-clock overhead but enables sites that would
otherwise OOM the runner.

---

## Criterion Bench Suite

Run with `cargo bench` (no args) or `make bench`. All Criterion runs
emit JSON under `target/criterion/`; the `benches/baselines/`
directory carries the committed reference numbers for cross-machine
comparison.

| Suite | File | Measures | Iterations |
|-------|------|----------|-----------:|
| Public-API surface sweep | `all_pub_api.rs` | Every `#[must_use] pub fn` at least once | ≥ 50 functions |
| AVIF vs WebP encoder | `avif_vs_webp.rs` | 1024×768 JPEG → AVIF/WebP encode time | 50 |
| Audit gate dispatch | `bench_audit.rs` | 15-gate dispatch on a 50-page corpus | 30 |
| Concurrent operations | `bench_concurrent_operations.rs` | Parallel file copy, directory traversal | 100 |
| File I/O patterns | `bench_file.rs` | Read/write atomic-vs-streaming | 100 |
| Scalability sweep | `bench_scalability.rs` | 10/100/1K/10K-page corpus build time | 10–30 |
| Site generation end-to-end | `bench_site_generation.rs` | Cold build, 10 → 10,000 pages | 10–30 |
| Utility hash/slug/string | `bench_utilities.rs` | FNV-1a, slug, SHA-256, lol_html rewriter | 1000 |
| 1000-page incremental warm | `incremental_1000_pages.rs` | DepGraph reverse-closure + skip set | 20 |
| Per-plugin micro | `plugins/*.rs` | Per-plugin transform_html + after_compile | 50 |

### Bench Corpus

Synthetic test corpora live under `benches/corpus/` and are
generated by `tools/seed-bench-corpus.sh` (issue #494):

| Corpus | Pages | Frontmatter density | Use |
|---|---:|---|---|
| `tiny` | 10 | full (title + description + tags + date) | smoke |
| `small` | 100 | full | 100-page budgets above |
| `medium` | 1,000 | mixed | scalability bench |
| `large` | 10,000 | mixed | batch-mode trigger |

Each corpus is deterministic — page bodies are seeded by SHA-256 of
the page index so a fresh `seed-bench-corpus.sh` run produces a byte-
identical tree across machines. That makes Criterion comparisons
cross-machine-meaningful.

---

## Cross-SSG Comparison

Comparative benchmarks against the leading static site generators.
Each row was measured on the same content corpus (`benches/corpus/small`,
100 pages, full frontmatter). Wall-clock times are median-of-10 runs
captured with [`hyperfine`](https://github.com/sharkdp/hyperfine).

| Capability | SSG v0.0.69 | Hugo v0.155+ | Zola v0.20+ | Astro 6 | Eleventy 3 |
|---|---|---|---|---|---|
| Language | Rust | Go | Rust | JS/TS | JS |
| Runtime | None | None | None | Node 20+ | Node 20+ |
| 100-page build, cold | ~300 ms | **~180 ms** | ~250 ms | ~2.5 s | ~1.4 s |
| 100-page build, warm cache | **~30 ms** | ~90 ms | n/a | ~600 ms | ~140 ms |
| 10K-page bounded-memory build | ~12 s (512 MB) | **~4 s** | OOM at ~8K | n/a (no SSG mode) | OOM at ~5K |
| Built-in WCAG 2.2 gate | **Yes** | No | No | No | No |
| CSP + SRI extraction | **Yes** | No | No | Partial | No |
| Local LLM pipeline | **Yes** | No | No | No | No |
| WASM build target | **Yes** | No | No | n/a | n/a |
| CycloneDX SBOM | **Yes** | No | No | No | No |
| CI coverage floor | **98.0 / 98.0 / 98.0** | None | None | None | None |

Reproduce locally:

```sh
# 1. Seed the corpus
./tools/seed-bench-corpus.sh small   # 100 pages

# 2. Run the comparison
hyperfine --warmup 3 \
  --command-name ssg     'cargo run --release -- build -c benches/corpus/small -o /tmp/ssg-bench' \
  --command-name hugo    'hugo --quiet -s benches/corpus/small -d /tmp/hugo-bench' \
  --command-name zola    'zola build -r benches/corpus/small -o /tmp/zola-bench' \
  --command-name astro   '(cd benches/corpus/small-astro && npm run build)' \
  --command-name eleventy '(cd benches/corpus/small-11ty && npx @11ty/eleventy)'
```

Notes on the comparison:

- **Hugo wins on raw throughput** for cold builds — Go's compiler emits
  tighter scalar code than rustc here. SSG closes the gap on warm
  builds because the `DepGraph`-driven incremental short-circuit
  bypasses the entire compile/render chain.
- **Astro and Eleventy** carry a ~250 ms Node startup tax independent
  of corpus size; SSG and Hugo have no startup tax.
- **Zola** OOMs at ~8K pages on a 7 GB GitHub runner because it has no
  memory-bounded mode. SSG's bounded-memory batch compiler trades
  ~10 % wall clock for unbounded scalability.

---

## Hyperfine Reproducibility Recipes

### Per-bench commands

```sh
# 100-page cold build (perf budget, gates on every CI run)
hyperfine --warmup 3 --runs 10 \
  'cargo run --release --quiet -- build -c benches/corpus/small \
                                       -o /tmp/ssg-bench \
                                       -t examples/templates'

# 100-page incremental warm-cache
hyperfine --warmup 3 --runs 20 \
  --setup 'rm -rf /tmp/ssg-bench && cargo run --release --quiet -- build -c benches/corpus/small -o /tmp/ssg-bench -t examples/templates' \
  --prepare 'touch benches/corpus/small/content/page-0.md' \
  'cargo run --release --quiet -- build --incremental -c benches/corpus/small -o /tmp/ssg-bench -t examples/templates'

# AVIF vs WebP single-image (requires --features image-optimization)
hyperfine --warmup 5 --runs 50 \
  'cargo bench --features image-optimization --bench avif_vs_webp -- --quick'
```

### Capturing baselines

```sh
# Save a labelled Criterion baseline
cargo bench -- --save-baseline pre-#583
# ...land work, repeat...
cargo bench -- --baseline pre-#583
```

Per-machine baselines belong in `benches/baselines/<host>/` — never
committed; the deterministic-corpus baselines under
`benches/baselines/criterion-json/` are committed.

---

## Latest nightly run

The figures in this section are produced by
[`.github/workflows/benchmarks.yml`](.github/workflows/benchmarks.yml), not
typed in. The workflow runs nightly on a named runner, records the hardware
alongside the numbers, uploads `bench-report.md` plus the raw Criterion
output as a build artifact, and commits this file with the results filled in
to the `benchmark-results` branch.

<!-- BENCHMARK-RESULTS:START -->
_Last measured 2026-10-28 04:51 UTC. Regenerated by the nightly workflow; do not edit by hand._

## Run environment

| Field | Value |
| --- | --- |
| Date (UTC) | 2026-10-28 04:51 |
| Commit | `ddd4eed62cdf08a1e69fae4b6f68009073cb0c94` |
| Runner | Linux / X64 (GitHub Actions 1000576207) |
| CPU | AMD EPYC 7763 64-Core Processor |
| Cores | 4 |
| Memory | 15Gi |
| Rust | rustc 1.99.1 (58cf8f9c3 2026-10-13) |
| SSG | 0.0.69 |

```
test site_generation/compile/10 ... bench:   6,174,764 ns/iter (+/- 24,572)
test site_generation/compile/50 ... bench:  15,281,708 ns/iter (+/- 92,068)
test site_generation/compile/100 ... bench:  30,006,138 ns/iter (+/- 2,046,166)
test site_generation/compile/1000 ... bench: 332,551,004 ns/iter (+/- 6,276,124)
test site_generation/compile/10000 ... bench: 5,133,795,083 ns/iter (+/- 117,290,855)
test audit_full_scaffold ... bench:     550,199 ns/iter (+/- 2,704)
test html_scanning::inject_before_head_close/clean ... bench:       4,168 ns/iter (+/- 153)
test html_scanning::inject_before_head_close/decoy ... bench:       4,353 ns/iter (+/- 46)
test html_scanning::extract_head_meta/clean ... bench:       6,718 ns/iter (+/- 107)
test html_scanning::extract_head_meta/decoy ... bench:       7,226 ns/iter (+/- 24)
test html_scanning::find_tag_end/plain ... bench:          21 ns/iter (+/- 0)
test html_scanning::find_tag_end/quoted_gt ... bench:          39 ns/iter (+/- 0)
test avif_vs_webp/webp/encode_1024x768 ... bench:   6,322,322 ns/iter (+/- 56,078)
test avif_vs_webp/avif/encode_1024x768_q70 ... bench: 3,448,773,054 ns/iter (+/- 22,511,177)
test avif_vs_webp/parallel_4_widths/webp_parallel ... bench:  15,915,362 ns/iter (+/- 987,690)
test avif_vs_webp/parallel_4_widths/avif_parallel_q70 ... bench: 11,109,497,553 ns/iter (+/- 219,298,690)
test incremental/warm_cache_1000_pages_noop ... bench:  11,708,607 ns/iter (+/- 135,287)
test incremental/single_edit_1000_pages_invalidation ... bench:  12,920,655 ns/iter (+/- 98,670)
test gates::ai_discovery ... bench:       3,210 ns/iter (+/- 35)
test gates::broken_links ... bench:      11,397 ns/iter (+/- 45)
test gates::csp_sri ... bench:       8,142 ns/iter (+/- 34)
test gates::feeds ... bench:      13,590 ns/iter (+/- 34)
test gates::hreflang ... bench:       6,002 ns/iter (+/- 67)
test gates::html5 ... bench:       5,656 ns/iter (+/- 103)
test gates::images ... bench:       9,942 ns/iter (+/- 24)
test gates::jsonld ... bench:       6,548 ns/iter (+/- 15)
test gates::lang_consistency ... bench:       6,604 ns/iter (+/- 16)
test gates::markdownlint ... bench:       2,030 ns/iter (+/- 8)
test gates::metadata ... bench:       8,119 ns/iter (+/- 18)
test gates::performance ... bench:       5,878 ns/iter (+/- 19)
test gates::pqc_tls ... bench:       2,995 ns/iter (+/- 8)
test gates::search_index ... bench:       2,382 ns/iter (+/- 19)
test gates::wcag ... bench:       7,284 ns/iter (+/- 24)
test gates::util::find_tag_end ... bench:          14 ns/iter (+/- 0)
test gates::util::hreflang_attr ... bench:          59 ns/iter (+/- 0)
test gates::all ... bench:          27 ns/iter (+/- 0)
test audit::output::json::format ... bench:       5,954 ns/iter (+/- 40)
test audit::output::junit::format ... bench:       8,891 ns/iter (+/- 183)
test audit::output::text::format ... bench:       1,960 ns/iter (+/- 27)
test audit::Severity::as_str ... bench:           6 ns/iter (+/- 0)
test audit::Severity::parse ... bench:          14 ns/iter (+/- 0)
test audit::SeverityCounts::add_and_total ... bench:           0 ns/iter (+/- 0)
test audit::AuditConfig::new ... bench:          11 ns/iter (+/- 0)
test audit::AuditConfig::default ... bench:          11 ns/iter (+/- 0)
test audit::AuditOptions::default ... bench:           3 ns/iter (+/- 0)
test audit::AuditBudgets::default ... bench:           1 ns/iter (+/- 0)
test audit::AuditDisabledSection::default ... bench:           1 ns/iter (+/- 0)
test audit::AuditTomlConfig::default ... bench:           3 ns/iter (+/- 0)
test audit::AuditTomlConfig::into_audit_config ... bench:          22 ns/iter (+/- 1)
test audit::AuditRunner::new ... bench:          40 ns/iter (+/- 0)
test audit::AuditRunner::gate_names ... bench:          46 ns/iter (+/- 0)
test audit::AuditRunner::fail_on ... bench:           0 ns/iter (+/- 0)
test audit::AuditRunner::run ... bench:     107,301 ns/iter (+/- 541)
test audit::AuditReport::max_severity ... bench:           6 ns/iter (+/- 0)
test audit::AuditReport::should_fail ... bench:           7 ns/iter (+/- 0)
test audit::AuditReport::len ... bench:           0 ns/iter (+/- 0)
test audit::AuditReport::is_empty ... bench:           0 ns/iter (+/- 0)
test audit::Site::load ... bench:      15,061 ns/iter (+/- 56)
test audit::Site::rel ... bench:         183 ns/iter (+/- 3)
test audit::Site::read ... bench:       4,907 ns/iter (+/- 17)
test audit::Finding::new+with_code+with_path ... bench:          46 ns/iter (+/- 0)
test cmd::resolve_host ... bench:         145 ns/iter (+/- 0)
test cmd::resolve_port ... bench:         134 ns/iter (+/- 0)
test cmd::default_config ... bench:           0 ns/iter (+/- 0)
test cmd::Cli::build ... bench:       3,445 ns/iter (+/- 59)
test cmd::Cli::subcommand_app ... bench:      13,784 ns/iter (+/- 248)
test cmd::SsgConfig::builder+build ... bench:       5,151 ns/iter (+/- 18)
test cmd::SsgConfig::validate ... bench:       4,651 ns/iter (+/- 21)
test cmd::SsgConfig::default ... bench:         130 ns/iter (+/- 1)
test cmd::EdgeHeadersConfig::is_enabled ... bench:           0 ns/iter (+/- 0)
test cmd::validation::is_valid_url ... bench:          25 ns/iter (+/- 0)
test cmd::validation::validate_url ... bench:         121 ns/iter (+/- 0)
test cmd::audit::build_subcommand ... bench:       1,891 ns/iter (+/- 24)
test core::process::args ... bench:          28 ns/iter (+/- 0)
test cache::BuildCache::new ... bench:          17 ns/iter (+/- 0)
test cache::BuildCache::len ... bench:           0 ns/iter (+/- 0)
test cache::BuildCache::is_empty ... bench:           0 ns/iter (+/- 0)
test cache::BuildCache::default_path ... bench:           6 ns/iter (+/- 0)
test depgraph::DepGraph::new ... bench:          17 ns/iter (+/- 0)
test depgraph::DepGraph::add_dep ... bench:          45 ns/iter (+/- 0)
test depgraph::DepGraph::add_output ... bench:          45 ns/iter (+/- 0)
test depgraph::DepGraph::record_hash ... bench:         665 ns/iter (+/- 9)
test depgraph::DepGraph::sha256_hex ... bench:         640 ns/iter (+/- 2)
test depgraph::DepGraph::deps_for ... bench:           9 ns/iter (+/- 0)
test depgraph::DepGraph::outputs_for ... bench:           8 ns/iter (+/- 0)
test depgraph::DepGraph::tracked_sources ... bench:          28 ns/iter (+/- 0)
test depgraph::DepGraph::page_count ... bench:           0 ns/iter (+/- 0)
test depgraph::DepGraph::invalidated ... bench:         429 ns/iter (+/- 3)
test depgraph::DepGraph::invalidated_outputs ... bench:         581 ns/iter (+/- 4)
test depgraph::DepGraph::diff ... bench:          46 ns/iter (+/- 0)
test deploy_adapter::Target::from_cli ... bench:           3 ns/iter (+/- 0)
test deploy_adapter::Target::as_str ... bench:           6 ns/iter (+/- 0)
test deploy_adapter::adapter_for ... bench:           1 ns/iter (+/- 0)
test schema::generate_schema ... bench:       2,655 ns/iter (+/- 10)
test schema::write_schema ... bench:     192,461 ns/iter (+/- 17,975)
test walk::walk_files ... bench:      11,724 ns/iter (+/- 113)
test walk::walk_files_multi ... bench:      11,725 ns/iter (+/- 39)
test walk::walk_files_bounded_depth ... bench:      11,727 ns/iter (+/- 41)
test walk::walk_files_bounded_count ... bench:      11,736 ns/iter (+/- 91)
test fs_ops::is_safe_path ... bench:         724 ns/iter (+/- 2)
test fs_ops::verify_file_safety ... bench:       1,479 ns/iter (+/- 26)
test frontmatter::emit_sidecars ... bench:     190,871 ns/iter (+/- 14,898)
test logging::create_log_file ... bench:      10,005 ns/iter (+/- 904)
test core::streaming::MemoryBudget::from_mb ... bench:           6 ns/iter (+/- 0)
test core::streaming::MemoryBudget::default_budget ... bench:           6 ns/iter (+/- 0)
test stream::stream_copy ... bench:     188,542 ns/iter (+/- 14,411)
test stream::stream_hash ... bench:       4,146 ns/iter (+/- 19)
test stream::stream_lines ... bench:       4,547 ns/iter (+/- 19)
test stream::process_batch ... bench:      76,607 ns/iter (+/- 13,791)
test scaffold::scaffold_project_at ... bench:   1,570,449 ns/iter (+/- 93,051)
test collections::get_collection ... bench:      18,350 ns/iter (+/- 1,530)
test collections::get_entry ... bench:      18,326 ns/iter (+/- 150)
test content::parse_schemas ... bench:         184 ns/iter (+/- 3)
test content::validate_frontmatter ... bench:          14 ns/iter (+/- 0)
test content::validate_content_dir ... bench:           5 ns/iter (+/- 0)
test content::validate_with_schema ... bench:       2,063 ns/iter (+/- 229)
test pipeline::clear_error_message ... bench:          10 ns/iter (+/- 0)
test pipeline::resolve_build_and_site_dirs ... bench:          84 ns/iter (+/- 2)
test pipeline::depgraph_cache_root ... bench:       1,282 ns/iter (+/- 10)
test template_engine::TemplateConfig::default ... bench:          20 ns/iter (+/- 0)
test template_engine::TemplateEngine::init ... bench:       2,555 ns/iter (+/- 52)
test template_engine::site_globals_from_config ... bench:         390 ns/iter (+/- 6)
test dates::FlexibleDate::to_rfc2822 ... bench:         334 ns/iter (+/- 4)
test dates::FlexibleDate::to_rfc3339 ... bench:         321 ns/iter (+/- 3)
test dates::FlexibleDate::to_w3c_date ... bench:         319 ns/iter (+/- 1)
test dates::FlexibleDate::to_iso_date ... bench:         108 ns/iter (+/- 1)
test dates::is_leap_year ... bench:           0 ns/iter (+/- 0)
test dates::days_in_month ... bench:           1 ns/iter (+/- 0)
test dates::DateParseError::attempted_formats+input ... bench:           6 ns/iter (+/- 0)
test urls::derive_page_url ... bench:         100 ns/iter (+/- 0)
test urls::derive_output_rel_path ... bench:          82 ns/iter (+/- 0)
test urls::derive_permalink ... bench:         195 ns/iter (+/- 2)
test io_pool::IoPool::completed_writes ... bench:           4 ns/iter (+/- 0)
test io_pool::IoPool::threads ... bench:           0 ns/iter (+/- 0)
test content_stager::inject_permalink_if_missing ... bench:         540 ns/iter (+/- 3)
test seo::helpers::extract_title ... bench:       7,124 ns/iter (+/- 75)
test seo::helpers::has_meta_tag ... bench:         235 ns/iter (+/- 4)
test seo::jsonld::validate_jsonld ... bench:       1,101 ns/iter (+/- 8)
test seo::canonical::CanonicalPlugin::new ... bench:          10 ns/iter (+/- 0)
test seo::robots::RobotsPlugin::new ... bench:          10 ns/iter (+/- 0)
test seo::jsonld::JsonLdPlugin::new ... bench:          25 ns/iter (+/- 0)
test seo::jsonld::JsonLdPlugin::from_site ... bench:          27 ns/iter (+/- 0)
test iso20022::log_first_use_pointer ... bench:           2 ns/iter (+/- 0)
test iso20022::validate_iban ... bench:         220 ns/iter (+/- 0)
test iso20022::validate_bic ... bench:          49 ns/iter (+/- 0)
test iso20022::MonetaryAmount::to_jsonld ... bench:         131 ns/iter (+/- 0)
test iso20022::BankAccount::to_jsonld ... bench:         534 ns/iter (+/- 4)
test iso20022::PaymentInstrument::to_jsonld ... bench:         516 ns/iter (+/- 3)
test iso20022::FinancialTransaction::to_jsonld ... bench:       2,015 ns/iter (+/- 6)
test iso20022::RegulatedFinancialInstitution::to_jsonld ... bench:         782 ns/iter (+/- 9)
test iso20022::FinancialProduct::to_jsonld ... bench:         803 ns/iter (+/- 11)
test iso20022::Iso20022Entity::to_jsonld ... bench:         522 ns/iter (+/- 2)
test iso20022::Iso20022Entity::type_name ... bench:           6 ns/iter (+/- 0)
test iso20022::from_frontmatter ... bench:         148 ns/iter (+/- 1)
test iso20022::warn_invalid_fields ... bench:         272 ns/iter (+/- 2)
test iso20022::validate_schema_org ... bench:          88 ns/iter (+/- 0)
test agentic::AgentsConfig::any_enabled ... bench:           0 ns/iter (+/- 0)
test agentic::ai_plugin::build_manifest ... bench:       1,258 ns/iter (+/- 9)
test agentic::agents_txt::render_agents_txt ... bench:         113 ns/iter (+/- 0)
test agentic::mcp::build_registry ... bench:         841 ns/iter (+/- 22)
test edge_headers::baseline_headers ... bench:          65 ns/iter (+/- 0)
test edge_headers::EdgeHeadersPlugin::new ... bench:           0 ns/iter (+/- 0)
test view_transitions::ViewTransitionsPlugin::new ... bench:           0 ns/iter (+/- 0)
test view_transitions::ViewTransitionsPlugin::enabled ... bench:           1 ns/iter (+/- 0)
test csp::computed_policy ... bench:           6 ns/iter (+/- 0)
test csp::CspPlugin::new ... bench:           0 ns/iter (+/- 0)
test csp::inject_csp_meta ... bench:       5,576 ns/iter (+/- 218)
test csp::page_inline_hashes ... bench:       6,245 ns/iter (+/- 88)
test csp::PageCspHashes::is_empty ... bench:           0 ns/iter (+/- 0)
test csp::render_policy_template ... bench:         408 ns/iter (+/- 1)
test csp::page_policy ... bench:       6,842 ns/iter (+/- 35)
test drafts::DraftPlugin::new ... bench:           0 ns/iter (+/- 0)
test highlight::HighlightPlugin::new ... bench:          10 ns/iter (+/- 0)
test highlight::HighlightPlugin::with_theme ... bench:          10 ns/iter (+/- 0)
test islands::IslandPlugin::new ... bench:           0 ns/iter (+/- 0)
test isr_manifest::IsrManifestPlugin::new ... bench:           0 ns/iter (+/- 0)
test isr_manifest::build_manifest ... bench:       7,560 ns/iter (+/- 74)
test markdown_ext::expand_gfm ... bench:       2,499 ns/iter (+/- 14)
test og_image::OgImagePlugin::new ... bench:          29 ns/iter (+/- 0)
test og_image::OgImagePlugin::with_colors ... bench:          29 ns/iter (+/- 0)
test og_image::generate_og_svg ... bench:         546 ns/iter (+/- 6)
test pagination::PaginationPlugin::with_per_page ... bench:           0 ns/iter (+/- 0)
test plugin::PluginContext::new ... bench:          65 ns/iter (+/- 0)
test plugin::PluginContext::with_config ... bench:         249 ns/iter (+/- 1)
test plugin::PluginContext::with_dry_run ... bench:         102 ns/iter (+/- 1)
test plugin::PluginContext::get_html_files ... bench:       7,461 ns/iter (+/- 31)
test plugin::PluginContext::cache_html_files ... bench:       8,987 ns/iter (+/- 138)
test plugin::PluginCache::new ... bench:           1 ns/iter (+/- 0)
test plugin::PluginCache::has_changed ... bench:       1,614 ns/iter (+/- 21)
test plugin::PluginCache::update ... bench:       4,787 ns/iter (+/- 19)
test plugin::PluginCache::save ... bench:     180,729 ns/iter (+/- 13,440)
test plugin::PluginManager::new ... bench:           4 ns/iter (+/- 0)
test plugin::PluginManager::len ... bench:           0 ns/iter (+/- 0)
test plugin::PluginManager::is_empty ... bench:           0 ns/iter (+/- 0)
test plugin::PluginManager::names ... bench:          12 ns/iter (+/- 0)
test plugin::PluginManager::run_before_compile ... bench:           2 ns/iter (+/- 0)
test plugin::PluginManager::run_after_compile ... bench:      18,220 ns/iter (+/- 166)
test plugin::PluginManager::run_fused_transforms ... bench:     179,717 ns/iter (+/- 4,468)
test plugin::PluginManager::run_on_serve ... bench:           2 ns/iter (+/- 0)
test plugins::minify_html ... bench:       1,913 ns/iter (+/- 21)
test plugins::minify_css ... bench:         298 ns/iter (+/- 1)
test plugins::minify_js ... bench:         161 ns/iter (+/- 2)
test rpc_schema::RpcSchemaPlugin::new ... bench:           0 ns/iter (+/- 0)
test sbom::SbomPlugin::sbom_path ... bench:           6 ns/iter (+/- 0)
test shortcodes::expand_shortcodes ... bench:       1,084 ns/iter (+/- 6)
test search::SearchLabels::english ... bench:         182 ns/iter (+/- 1)
test search::SearchLabels::french ... bench:         183 ns/iter (+/- 0)
test search::SearchLabels::for_locale ... bench:         207 ns/iter (+/- 2)
test search::LocalizedSearchPlugin::new ... bench:         183 ns/iter (+/- 2)
test search::SearchIndex::build ... bench:      59,909 ns/iter (+/- 307)
test search::SearchIndex::len ... bench:           0 ns/iter (+/- 0)
test search::SearchIndex::is_empty ... bench:           0 ns/iter (+/- 0)
test search::SearchIndex::write ... bench:     183,490 ns/iter (+/- 14,015)
test template_plugin::TemplatePlugin::from_template_dir ... bench:          54 ns/iter (+/- 0)
test template_plugin::TemplatePlugin::new ... bench:          20 ns/iter (+/- 0)
test i18n::parse_accept_language ... bench:         214 ns/iter (+/- 2)
test i18n::negotiate_locale ... bench:          78 ns/iter (+/- 0)
test i18n::generate_lang_switcher_html ... bench:       1,362 ns/iter (+/- 36)
test i18n::I18nPlugin::new ... bench:          73 ns/iter (+/- 3)
test llm::LlmPlugin::new ... bench:         163 ns/iter (+/- 1)
test llm::ReadabilityFormula::from_lang ... bench:          17 ns/iter (+/- 0)
test llm::ReadabilityAudit::analyze ... bench:       1,273 ns/iter (+/- 15)
test llm::ReadabilityAudit::analyze_with_lang ... bench:       2,793 ns/iter (+/- 34)
test llm_cache::LlmCache::new ... bench:          17 ns/iter (+/- 0)
test llm_cache::LlmCache::with_ttl ... bench:          17 ns/iter (+/- 0)
test llm_cache::LlmCache::default_cache_dir ... bench:         521 ns/iter (+/- 9)
test llm_cache::LlmCache::compute_key ... bench:         133 ns/iter (+/- 6)
test llm_cache::LlmCache::stats ... bench:           2 ns/iter (+/- 0)
test llm_cache::LlmCache::root ... bench:           0 ns/iter (+/- 0)
test agent_api::AgentApiPlugin::new ... bench:           0 ns/iter (+/- 0)
test agent_api::AgentApiPlugin::disabled ... bench:           0 ns/iter (+/- 0)
test agent_api::collect_posts ... bench:      68,381 ns/iter (+/- 275)
test agent_api::jsonld_word_count ... bench:         374 ns/iter (+/- 1)
test agent_api::parse_author ... bench:          56 ns/iter (+/- 0)
test oembed::OembedPlugin::transform_html ... bench:      13,580 ns/iter (+/- 100)
test oembed::build_oembed ... bench:         373 ns/iter (+/- 1)
test util::head_dom::inject_before_head_close ... bench:       4,252 ns/iter (+/- 83)
test util::head_dom::extract_head_meta ... bench:       7,123 ns/iter (+/- 106)
test util::head_dom::remove_canonical_links ... bench:       4,704 ns/iter (+/- 71)
test util::head_dom::replace_canonical_link ... bench:       5,905 ns/iter (+/- 91)
test util::html_rewriter::decode_html_entities ... bench:         151 ns/iter (+/- 0)
test util::html_rewriter::collapse_whitespace ... bench:          74 ns/iter (+/- 4)
test util::html_rewriter::extract_text_with_filter ... bench:       7,168 ns/iter (+/- 63)
test event_watch::event_should_propagate ... bench:           1 ns/iter (+/- 0)
test event_watch::debounce_paths ... bench:         210 ns/iter (+/- 0)
test event_watch::ChangeBatch::is_empty ... bench:           0 ns/iter (+/- 0)
test event_watch::ChangeBatch::len ... bench:           0 ns/iter (+/- 0)
test event_watch::RecvOutcome::is_closed ... bench:           0 ns/iter (+/- 0)
test event_watch::RecvOutcome::batch ... bench:          32 ns/iter (+/- 0)
test hmr::HmrMessage::css ... bench:          32 ns/iter (+/- 0)
test hmr::HmrMessage::html ... bench:          32 ns/iter (+/- 0)
test hmr::HmrMessage::reload ... bench:           9 ns/iter (+/- 0)
test hmr::HmrType::wire ... bench:           6 ns/iter (+/- 0)
test hmr::HmrMessage::with_sha ... bench:          41 ns/iter (+/- 0)
test hmr::HmrMessage::to_json ... bench:          61 ns/iter (+/- 0)
test hmr::HmrBroadcaster::new ... bench:           6 ns/iter (+/- 0)
test hmr::HmrBroadcaster::subscriber_count ... bench:           5 ns/iter (+/- 0)
test livereload::LiveReloadPlugin::new ... bench:           0 ns/iter (+/- 0)
test livereload::LiveReloadPlugin::with_port ... bench:           0 ns/iter (+/- 0)
test livereload::LiveReloadPlugin::port ... bench:           0 ns/iter (+/- 0)
test livereload::css_reload_message ... bench:         307 ns/iter (+/- 1)
test watch::classify_change ... bench:          31 ns/iter (+/- 0)
test watch::WatchConfig::new ... bench:          16 ns/iter (+/- 0)
test watch::WatchConfig::directory ... bench:           6 ns/iter (+/- 0)
test watch::WatchConfig::poll_interval ... bench:           1 ns/iter (+/- 0)
test watch::FileWatcher::new ... bench:       7,466 ns/iter (+/- 30)
test watch::FileWatcher::config ... bench:           0 ns/iter (+/- 0)
test watch::FileWatcher::tracked_file_count ... bench:           0 ns/iter (+/- 0)
test watch::FileWatcher::check_for_changes ... bench:       7,456 ns/iter (+/- 32)
test dev_server::output_to_url ... bench:         231 ns/iter (+/- 3)
test server::generate_locale_redirect ... bench:     198,732 ns/iter (+/- 17,078)
test server::prepare_serve_dir ... bench:       7,542 ns/iter (+/- 969)
test ssg_core::compile_markdown ... bench:       2,150 ns/iter (+/- 54)
test ssg_core::parse_frontmatter ... bench:       1,734 ns/iter (+/- 47)
test ssg_core::compile_page ... bench:       2,257 ns/iter (+/- 63)
test ssg_core::strip_html_tags ... bench:          44 ns/iter (+/- 0)
test ssg_core::build_search_entry ... bench:          83 ns/iter (+/- 0)
test ssg_core::reading_time ... bench:          44 ns/iter (+/- 0)
test ssg_core::slugify ... bench:         132 ns/iter (+/- 0)
test ssg_core::hash_sources ... bench:         655 ns/iter (+/- 3)
test ssg_core::build_entry ... bench:         678 ns/iter (+/- 3)
test ssg_core::CachePolicy::to_cache_control ... bench:          58 ns/iter (+/- 0)
test ssg_core::Manifest::new ... bench:          17 ns/iter (+/- 0)
test ssg_core::Manifest::insert ... bench:          56 ns/iter (+/- 0)
test ssg_core::Manifest::get ... bench:           5 ns/iter (+/- 0)
test ssg_core::Manifest::len ... bench:           0 ns/iter (+/- 0)
test ssg_core::Manifest::is_empty ... bench:           0 ns/iter (+/- 0)
test ssg_core::Manifest::to_pretty_json ... bench:         466 ns/iter (+/- 3)
test ssg_core::Manifest::urls_for_source ... bench:          39 ns/iter (+/- 0)
test ssg_core::MemoryContentProvider::new ... bench:           1 ns/iter (+/- 0)
test ssg_core::MemoryContentProvider::insert ... bench:          45 ns/iter (+/- 0)
test ssg_core::MemoryContentProvider::len ... bench:           0 ns/iter (+/- 0)
test ssg_core::MemoryContentProvider::is_empty ... bench:           0 ns/iter (+/- 0)
test ssg_core::FsContentProvider::new ... bench:          12 ns/iter (+/- 0)
test ssg_core::FsContentProvider::root ... bench:           6 ns/iter (+/- 0)
test ssg_search::ProjectionEncoder::new ... bench:           0 ns/iter (+/- 0)
test ssg_search::ProjectionEncoder::config ... bench:           0 ns/iter (+/- 0)
test ssg_search::ProjectionEncoder::seed ... bench:           0 ns/iter (+/- 0)
test ssg_search::Encoder::dim ... bench:           0 ns/iter (+/- 0)
test ssg_search::Encoder::embed ... bench:       3,583 ns/iter (+/- 55)
test ssg_search::Encoder::serialize_model ... bench:          11 ns/iter (+/- 0)
test ssg_search::Encoder::serialize_tokenizer ... bench:         552 ns/iter (+/- 2)
test ssg_search::deserialize_projection_encoder ... bench:           8 ns/iter (+/- 0)
test ssg_search::VectorEngine::new ... bench:         255 ns/iter (+/- 2)
test ssg_search::VectorEngine::encoder ... bench:           0 ns/iter (+/- 0)
test ssg_search::VectorEngine::dim ... bench:           0 ns/iter (+/- 0)
test ssg_search::VectorEngine::count ... bench:           0 ns/iter (+/- 0)
test ssg_search::VectorEngine::corpus ... bench:           6 ns/iter (+/- 0)
test ssg_search::VectorEngine::embed_query ... bench:       1,189 ns/iter (+/- 65)
test ssg_search::VectorEngine::search_vec ... bench:         120 ns/iter (+/- 0)
test ssg_search::VectorEngine::search ... bench:       1,365 ns/iter (+/- 24)
test ssg_search::Artifacts::from_docs ... bench:       4,767 ns/iter (+/- 52)
test ssg_search::Artifacts::dim ... bench:           0 ns/iter (+/- 0)
test ssg_search::Artifacts::count ... bench:           0 ns/iter (+/- 0)
test ssg_search::ArtifactsBuilder::new ... bench:           4 ns/iter (+/- 0)
test ssg_search::ArtifactsBuilder::add_doc ... bench:         148 ns/iter (+/- 255)
test ssg_search::ArtifactsBuilder::len ... bench:           0 ns/iter (+/- 0)
test ssg_search::ArtifactsBuilder::is_empty ... bench:           0 ns/iter (+/- 0)
test ssg_search::ArtifactsBuilder::encoder ... bench:           0 ns/iter (+/- 0)
test ssg_search::ArtifactsBuilder::build ... bench:       1,333 ns/iter (+/- 19)
test ssg_search::Manifest::new ... bench:          64 ns/iter (+/- 0)
test ssg_search::Manifest::is_valid ... bench:           0 ns/iter (+/- 0)
test ssg_search::quantize_int8 ... bench:         226 ns/iter (+/- 0)
test ssg_search::dequantize_int8 ... bench:          20 ns/iter (+/- 0)
test ssg_rpc::RpcError::status_code ... bench:           1 ns/iter (+/- 0)
test ssg_rpc::RpcError::to_wire_body ... bench:         165 ns/iter (+/- 0)
test ssg_rpc::dispatch::iter_descriptors ... bench:           0 ns/iter (+/- 0)
test ssg_rpc::dispatch::registered_names ... bench:           2 ns/iter (+/- 0)
test ssg_rpc::dispatch::find ... bench:           2 ns/iter (+/- 0)
test ssg_rpc::schema::schema_for::<String> ... bench:         592 ns/iter (+/- 3)
test ssg_rpc::ts::EmitOptions::default ... bench:          14 ns/iter (+/- 0)
test ssg_rpc::ts::emit_typescript ... bench:          55 ns/iter (+/- 0)
test ssg_rpc::ts::emit_typescript_for ... bench:          55 ns/iter (+/- 0)
test ssg::now_iso ... bench:         252 ns/iter (+/- 6)
test ssg::Paths::default_paths ... bench:          46 ns/iter (+/- 0)
test ssg::Paths::builder ... bench:           4 ns/iter (+/- 0)
test ssg::Paths::validate ... bench:       4,837 ns/iter (+/- 16)
test ssg::PathsBuilder::site+content+build+template+build ... bench:       2,961 ns/iter (+/- 39)
test ssg::PathsBuilder::relative_to ... bench:       3,353 ns/iter (+/- 20)
test ssg::create_directories ... bench:     391,942 ns/iter (+/- 61,972)
test concurrent_copy/small_files ... bench:   5,444,514 ns/iter (+/- 78,873)
test concurrent_copy/medium_files ... bench:   4,566,310 ns/iter (+/- 91,112)
test concurrent_copy/large_files ... bench:  20,741,609 ns/iter (+/- 283,159)
test verify_files/regular_files ... bench:      52,319 ns/iter (+/- 656)
test verify_files/nested_directories ... bench:   1,608,878 ns/iter (+/- 70,825)
test verify_files/mixed_content ... bench:   1,218,841 ns/iter (+/- 412,802)
test cache::load_missing ... bench:      32,684 ns/iter (+/- 1,210)
test cache::changed_files_100_unchanged ... bench:     761,745 ns/iter (+/- 3,075)
test cache::update_100_files ... bench:   2,516,632 ns/iter (+/- 15,215)
test collections::get_collection_100 ... bench:   2,781,042 ns/iter (+/- 17,103)
test content::parse_schemas ... bench:       2,665 ns/iter (+/- 15)
test depgraph::invalidated_pages_1000x4 ... bench:   1,424,708 ns/iter (+/- 65,799)
test deploy::after_compile_netlify ... bench:     138,890 ns/iter (+/- 1,076)
test frontmatter::emit_sidecars_50 ... bench:   3,803,009 ns/iter (+/- 322,126)
test fs_ops::copy_dir_all_50_files ... bench:   2,657,930 ns/iter (+/- 31,318)
test logging::create_and_banner ... bench:      73,184 ns/iter (+/- 752)
test schema::generate_schema ... bench:       2,752 ns/iter (+/- 14)
test stream::copy_64k ... bench:     139,876 ns/iter (+/- 1,252)
test stream::hash_64k ... bench:      22,842 ns/iter (+/- 489)
test walk::flat_500 ... bench:   1,273,313 ns/iter (+/- 12,279)
test walk::deep_10x10 ... bench:     398,027 ns/iter (+/- 1,453)
test walk::bounded_count_100_of_1000 ... bench:     534,403 ns/iter (+/- 4,101)
test i18n::parse_accept_language ... bench:         211 ns/iter (+/- 4)
test i18n::negotiate_locale ... bench:          75 ns/iter (+/- 1)
test markdown_ext::expand_gfm ... bench:       1,521 ns/iter (+/- 20)
test og_image::generate_og_svg ... bench:         346 ns/iter (+/- 7)
test seo::helpers::extract_title ... bench:       2,637 ns/iter (+/- 117)
test seo::helpers::has_meta_tag ... bench:          81 ns/iter (+/- 2)
test seo::jsonld::validate_jsonld ... bench:         472 ns/iter (+/- 12)
test shortcodes::expand_shortcodes ... bench:         411 ns/iter (+/- 6)
```
<!-- BENCHMARK-RESULTS:END -->

Corpora come from `ssg::bench_corpus`, a seeded generator whose output is
byte-identical across machines and releases — page N is the same whether the
corpus holds 1K pages or 100K, so the tiers are comparable to each other and
to previous runs. A figure that cannot be regenerated is a claim, not a
measurement.

---

## Coverage Floors (informational)

CI's coverage gate enforces the floors below (set in `ci.yml` `env`):

| Metric | Floor | Current (v0.0.69) | Headroom |
|---|---:|---:|---:|
| Regions | 98.0 % | 99.05 % | 1.05 |
| Functions | 98.0 % | 99.01 % | 1.01 |
| Lines | 98.0 % | 99.06 % | 1.06 |

The gate runs `cargo llvm-cov --lib` (not `--tests`) so the heavy
`tests/example_outputs.rs` integration suite stays in its own
`examples` job. Floors are raised in lockstep with measured deltas;
see PR #583 for the v0.0.45 trajectory.

---

## Environment

Benchmark results vary by hardware. CI budgets use generous limits
to accommodate GitHub Actions runners. Local results are typically
2–5× faster than CI.

| Key | CI Runner | Recommended Local |
|-----|-----------|-------------------|
| CPU | 2 vCPU (GitHub Actions ubuntu-latest) | 4+ cores, ≥ 3 GHz |
| RAM | 7 GB | 16+ GB |
| Disk | SSD (varies by region) | NVMe SSD |
| OS | Ubuntu 24.04 LTS | macOS 15+ / Linux 6.x+ |
| Rust | Stable (latest, `rust-toolchain.toml`) | Stable (latest) |

For the cross-SSG comparison numbers above, the host was:

```text
CPU: Apple M3 Max (14 cores)
RAM: 36 GB
Disk: NVMe (Apple Internal)
OS: macOS 15.2
Rust: 1.95.0
Hugo: 0.156.0
Zola: 0.20.0
Node: 22.11.0 (Astro 6.0.0, Eleventy 3.0.1)
```

---

## See Also

- [`tools/seed-bench-corpus.sh`](tools/seed-bench-corpus.sh) — generates
  the deterministic synthetic corpora used by perf budgets and the
  cross-SSG comparison.
- [`tests/perf_regression.rs`](tests/perf_regression.rs) — the
  CI-gated wall-clock budgets.
- [`tests/perf_budgets.rs`](tests/perf_budgets.rs) — the per-corpus
  cold + warm budgets.
- [`benches/`](benches/) — the Criterion suite (10 bench targets).
- [`docs/architecture/regression-contract.md`](docs/architecture/regression-contract.md) —
  the contract between perf gates and the regression suite.
