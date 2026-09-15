<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->
<!-- markdownlint-disable MD025 -->
<!--
  MD025 (single H1) is disabled for this file only. mdBook's SUMMARY.md
  format uses `# Heading` for part titles, so multiple H1s are the
  required syntax here rather than a mistake. Scoped to this file so the
  rule keeps working everywhere else.
-->

# Summary

[Introduction](README.md)

---

# Getting started

- [Installation](guide/installation.md)
- [Quick start](guide/quick-start.md)
- [The CLI](guide/cli.md)
- [Configuration](guide/configuration.md)

# Authoring content

- [Content](guide/content.md)
- [Content schema](guide/content-schema.md)
- [Templates](guide/templates.md)
- [Images](guide/images.md)
- [Internationalisation](guide/i18n.md)
- [Topics](guide/topics.md)
- [Listings](guide/listings.md)

# Capabilities

- [Accessibility](guide/accessibility.md)
  - [WCAG compliance](guide/wcag-compliance.md)
- [SEO](guide/seo.md)
- [Search](guide/search.md)
- [Plugins](guide/plugins.md)
  - [Plugin API](guide/plugin-api.md)
- [Interactive islands](guide/islands.md)
- [Streaming compilation](guide/streaming.md)
- [WebAssembly](guide/wasm.md)
- [LLM content pipelines](guide/llm-content-pipelines.md)

# Operating

- [Deployment](guide/deployment.md)
- [Developer experience](guide/dev-experience.md)
- [Packaging for distributions](packaging.md)

# Reference

- [Architecture](ARCHITECTURE.md)
  - [API stability audit](architecture/api-stability-audit.md)
  - [Regression contract](architecture/regression-contract.md)
- [Feature coverage](features-coverage.md)
- [Performance baseline](perf/baseline-100p.md)
- [SBOM and provenance](security/sbom-provenance.md)
- [CSP without compromise](whitepaper/csp-without-compromise.md)

# Compared with

- [Astro](compare/ssg-vs-astro.md)
- [Hugo](compare/ssg-vs-hugo.md)
- [Zola](compare/ssg-vs-zola.md)

# Decisions

- [Architecture Decision Records](adr/README.md)
  - [ADR-0001 — Tokio-free architecture](adr/0001-tokio-free.md)
  - [ADR-0002 — Rayon for build orchestration](adr/0002-rayon-orchestration.md)
  - [ADR-0003 — lol_html over html5ever](adr/0003-lol_html-over-html5ever.md)
  - [ADR-0004 — Sync tungstenite for HMR](adr/0004-sync-tungstenite-for-hmr.md)
  - [ADR-0005 — ureq for the LLM transport](adr/0005-ureq-for-llm.md)
  - [ADR-0006 — CycloneDX over SPDX](adr/0006-cyclonedx-over-spdx.md)
  - [ADR-0007 — staticdatagen staging shim](adr/0007-staticdatagen-staging-shim.md)
  - [ADR-0009 — Versioning policy](adr/0009-versioning-policy-0.0.x-until-0.0.999.md)

---

[Implementation plan (v0.0.47)](plans/v0.0.47-implementation-plan.md)
