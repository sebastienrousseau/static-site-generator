<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# SSG Regression Contract

This document is the **canonical inventory of what CI guarantees**
against regressions and divergence. Every line below maps a concrete
test (or set of tests) to the user-facing promise it enforces. If a
change passes CI, the contract holds; if a change wants to break a
contract, it must explicitly update both the test and this document.

## 1. Library Behaviour

| Surface | Test | Promise |
|---|---|---|
| Lib unit tests | `cargo test --lib` (1,685+ tests) | Every public function and plugin maintains its documented behaviour. |
| Plugin trait contracts | `tests/plugin_contracts.rs` | Every built-in plugin honours the `before_compile` / `after_compile` / `transform_html` lifecycle hook signatures and idempotency. |
| Doc examples | `cargo test --doc` | Every `///` rustdoc example compiles and returns `Ok`. |
| **README + docs accuracy** | **`tests/docs_accuracy.rs`** | **README claims (test count, WCAG version, coverage floors, MSRV, version, CycloneDX spec version) match source-of-truth files (`Cargo.toml`, `ci.yml`, `src/accessibility.rs`, `src/sbom.rs`).** Catches doc-drift on every PR. |
| **Internal Markdown link integrity** | **`tests/doc_links.rs`** | **Every relative Markdown link in `README.md`, `CHANGELOG.md`, `SECURITY.md`, `CONTRIBUTING.md`, and the entire `docs/` tree resolves to an existing file.** Prevents broken cross-references creeping in. |

## 2. End-to-End Output

| Surface | Test | Promise |
|---|---|---|
| Example output validators | `tests/example_outputs.rs` | Every shipped example (`basic`, `blog`, `docs`, `landing`, `multilingual`, `plugins`, `portfolio`, `quickstart`) builds without panic and produces HTML that passes 8 hand-curated regression checks (preload `href`, mobile-menu CSS, manifest icons, etc.). |
| **Universal HTML core invariants** | **`tests/element_presence.rs::core_invariants_hold_for_every_page`** | **Every page emitted by every example satisfies 8 core invariants**: `<html lang>`, non-empty `<title>`, non-empty `<meta name=description>`, `<main>` landmark, charset declared, `<link rel=canonical>`, full Open Graph chain (`og:title`/`og:description`/`og:type`), Twitter Card meta. Any new page or template change failing one of these is a CI block. |
| **Universal HTML invariants** | **`tests/element_presence.rs::every_built_example_page_satisfies_universal_invariants`** | **Adds `<h1>`-exactly-once and `<meta name=viewport>` on top of the core set, across every page of every built example.** Formerly `#[ignore]`d because some shipped taxonomy and index templates omitted them; those template gaps are closed, so the gate is always-on and a regression is a CI block. |
| JSON-LD validation | `tests/jsonld_validation.rs` | Every `<script type="application/ld+json">` block in every example output parses and contains the schema.org-required fields for its `@type`. |
| Golden files | `tests/golden_files.rs` | Specific deterministic artifacts byte-match a checked-in golden after normalisation (timestamps, hashes, SRI stripped). Currently seeded with `scaffold_config_toml.golden`; expanding incrementally per #466. |

## 3. Performance

| Surface | Test | Promise |
|---|---|---|
| Atomic operations sub-50ms | `tests/perf_regression.rs` | Slugify, URL parse, frontmatter walk, depgraph load all under 50 ms on the CI runner. |
| **End-to-end build budgets** | **`tests/perf_budgets.rs`** | **10-page build < 100 ms, 100-page < 500 ms, 500-page < 2 s** (CI ceilings; local M-arm64 is ~10× faster). Measured as the median of 3 runs after a warmup iteration to filter scheduler hiccups. |
| Scalability bench | `benches/bench_scalability.rs` | Reproduces the baseline (`docs/perf/baseline-100p.md`) for trend tracking. Not a hard gate — informational. |

## 4. Resilience / Chaos

| Surface | Test | Promise |
|---|---|---|
| I/O failpoint injection | `tests/fault_injection.rs` | Every `fs::write` / `fs::create_dir_all` failpoint is propagated as `anyhow::Error` with context, never as a panic. |
| Real-world malformed input | `tests/chaos.rs` | Corrupt frontmatter (missing delimiters, invalid UTF-8, unterminated strings), zero-byte and truncated images, symlink loops, 130-deep directories, read-only output, concurrent builds — all return cleanly without panic. |
| Schema validation | `tests/schema_validation.rs` | Frontmatter schema enforcement catches every documented malformation pattern. |

## 5. Security

| Surface | Test | Promise |
|---|---|---|
| Dependency audit | `cargo deny check` (CI `audit` job) | Zero unflagged advisories; allow-list documented in `deny.toml`. |
| Reproducible build | `scheduled.yml` `reproducible` job | `cargo build --release --locked --offline -p ssg` produces byte-identical output across two consecutive runs at the same SHA. |
| SRI integrity | `src/assets.rs` test suite | Every fingerprinted CSS/JS file's `integrity="sha256-..."` attribute is canonical SHA-256 + base64 (verified against NIST test vectors). |
| `cargo-geiger`-clean | `#![forbid(unsafe_code)]` at every crate root | Zero `unsafe` blocks in source. |
| Action SHA pinning | All `.github/workflows/*.yml` use 40-char commit SHAs | Tag-replay attacks blocked. |

## 6. Accessibility

| Surface | Test | Promise |
|---|---|---|
| WCAG 2.2 build-time checks | `src/accessibility.rs` (in `cargo test --lib`) | Every emitted page is checked for SC 1.1.1, 1.3.1, 2.3.1, 2.4.4, 2.4.13, 2.5.8, 3.1.1, plus ARIA landmarks. Reports written to `accessibility-report.json`; full criterion matrix at `wcag-compliance.json`. |
| WCAG 2.2 runtime gate | `tests/visual/a11y.spec.ts` (axe-core via Playwright) | Every page surface passes axe-core's 100+ rules in a real Chromium. |

## 7. Cross-platform

| Surface | Test | Promise |
|---|---|---|
| Per-PR 3-OS test matrix | `ci.yml` `test` job (ubuntu, macos, windows) | Lib + integration tests pass on all three platforms. |
| Multi-OS example outputs | `scheduled.yml` `examples-portability` job | Full `example_outputs.rs` suite runs weekly + on tag on macOS and Windows. |
| Multi-OS × MSRV | `scheduled.yml` `portability` job | Build + lib tests + doc-build pass on stable + MSRV (1.89) × ubuntu/macos/windows. |

## What is NOT gated

The following are reviewer-judgement items where automated coverage
is impractical or out of scope. Changes touching these areas need
explicit reviewer attention:

- **Cognitive accessibility** — plain-language editing, predictable
  navigation copy, consistent interaction patterns.
- **Cross-page WCAG checks** — 3.2.6 Consistent Help requires
  comparing pages, not validating one in isolation. Marked `Manual`
  in `wcag-compliance.json`.
- **Visual design** — axe-core gates contrast and layout but cannot
  judge whether a redesign is *better*; visual regression diffs
  (`tests/visual/`) provide the change report, but the merge
  decision is human.
- **Locale-specific content** — i18n correctness is gated for
  English; the full 28-locale matrix is tracked in #465 (its own
  milestone).
- **Screen-reader / assistive-tech testing** — NVDA, JAWS, VoiceOver,
  TalkBack — manual testing per major release.

## How to evolve the contract

1. Adding a new gate: write the test, wire it into `ci.yml` (or
   `scheduled.yml` if heavy), and add a row to the matching table
   above with the test path and the precise promise.
2. Tightening a budget: edit `tests/perf_budgets.rs` and update the
   "Budget table" comment plus the row in §3.
3. Loosening a budget *requires* a CHANGELOG entry citing why the
   relaxation is acceptable.
4. Removing a gate is a breaking change — needs a `chore!:` commit
   and a documented reason.

## Status

Last reviewed: 2026-05-10. Branch: `feat/v0.0.39`. PR #493.

The current state is **8 hard gates active**:
core HTML invariants (`tests/element_presence.rs`), end-to-end build
budgets (`tests/perf_budgets.rs`), JSON-LD validation
(`tests/jsonld_validation.rs`), reproducible build
(`scheduled.yml`), README + docs accuracy (`tests/docs_accuracy.rs`),
internal Markdown link integrity (`tests/doc_links.rs`), and the
1,685+ lib test suite.

The universal HTML invariants gate (§2) is no longer `#[ignore]`d — the
example-template gaps it was waiting on are closed, so it now blocks CI
like the rest. End-to-end build budgets currently
verify 10-page < 100 ms and 100-page < 500 ms on every PR; the
500-page < 2 s gate is opt-in via `--ignored` to keep PR runtime low.
