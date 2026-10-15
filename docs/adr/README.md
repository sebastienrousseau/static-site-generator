<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Architecture Decision Records (ADRs)

Load-bearing architectural commitments for the `ssg` codebase. Each ADR
captures *why* a structural decision was made, not *what* the code does
— the code is self-evidencing for the what.

ADRs use the **Nygard format** (Michael Nygard, *Documenting
Architecture Decisions*, 2011):

1. **Context** — the forces in play when the decision was made.
2. **Decision** — what we chose.
3. **Consequences** — what follows from the choice, good and bad.
4. **Alternatives Considered** — what we rejected, and why.
5. **Status** — `Accepted` / `Superseded by ADR-NN` / `Deprecated`.

ADRs are immutable once accepted. To change a decision, write a new ADR
that supersedes the old one — never edit history.

## Index

| ID | Title | Status |
|---|---|---|
| [ADR-0001](0001-tokio-free.md) | Tokio-free architecture | Accepted |
| [ADR-0002](0002-rayon-orchestration.md) | Rayon for build-pipeline orchestration | Accepted |
| [ADR-0003](0003-lol_html-over-html5ever.md) | `lol_html` for streaming HTML rewriting | Accepted |
| [ADR-0004](0004-sync-tungstenite-for-hmr.md) | Sync `tungstenite` for HMR fan-out | Accepted (superseded planned in v0.0.48 — see #571) |
| [ADR-0005](0005-ureq-for-llm.md) | `ureq` for the local-LLM HTTP path | Accepted |
| [ADR-0006](0006-cyclonedx-over-spdx.md) | CycloneDX 1.5 as the primary SBOM format | Accepted |
| [ADR-0007](0007-staticdatagen-staging-shim.md) | Content-staging shim for `staticdatagen` upstream regressions | Accepted (temporary — superseded once upstream lands; see [#585](https://github.com/sebastienrousseau/static-site-generator/issues/585)) |
| ADR-0008 | *(reserved)* async-tungstenite HMR migration | Planned — see [#571](https://github.com/sebastienrousseau/static-site-generator/issues/571) |
| [ADR-0009](0009-versioning-policy-0.0.x-until-0.0.999.md) | `0.0.x`-only versioning until `0.0.999` — no `0.1.0`/`1.0.0` yet | Accepted |
| [ADR-0010](0010-msrv-1.89-shared-floor.md) | MSRV 1.89.0, one floor shared with the maintainer's other crates | Proposed |

## Linking from code

Source files that act on an ADR must cite it inline:

```rust
// adr: ADR-0001 — tokio-free; this writer must not block a Rayon worker
self.io_pool.submit_write(path, bytes);
```

A CI gate (`tools/lint-adr.sh`) refuses any `// adr: ADR-NN` reference
that does not resolve to a file in this directory. The same gate is
extended to `KV.put()` call sites in any sibling Cloudflare-Workers
project per the global CLAUDE.md rule.

## Template

```markdown
<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# ADR-NNNN: <Title in one line>

- **Date:** YYYY-MM-DD
- **Status:** Accepted

## Context

What forced this decision. Concrete constraints, prior incidents, the
regulatory or competitive frame, not aspirational mush.

## Decision

The choice made, in declarative present tense ("We use X.").

## Consequences

What this commits us to. Positive (capabilities unlocked, surfaces
closed) and negative (options foreclosed, costs incurred).

## Alternatives Considered

Each rejected alternative with a one-line rationale for rejection.

## Status

Accepted / Superseded by ADR-NN (YYYY-MM-DD) / Deprecated.
```

## Adding a new ADR

1. Copy the template above into `docs/adrs/NNNN-kebab-slug.md`. ID is
   the next free integer, zero-padded to four digits.
2. Update the Index table in this file.
3. Cite the ADR from any code that depends on it.
4. Open a PR. The `lint-adr` CI gate verifies the citation graph.
