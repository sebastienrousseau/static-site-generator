<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Contributing to SSG

Welcome! We're thrilled that you're interested in contributing to SSG. Whether you're looking to report a bug, suggest a feature, or submit code, this guide will help you get started.

## Development setup

### Prerequisites

- [Rust](https://rustup.rs/) 1.89.0 or later
- Git with commit signing configured (see below)

### Getting started

```bash
git clone https://github.com/sebastienrousseau/static-site-generator.git
cd static-site-generator
cargo build
cargo test
```

### Useful commands

```bash
make init        # Bootstrap (rustfmt + clippy + cargo-deny + hooks + build)
make build       # Build the project
make test        # Run all tests
make bench       # Run Criterion benchmarks
make lint        # Lint with Clippy
make format      # Format code with rustfmt
make deny        # Check dependencies for security/license issues
make doc         # Generate API docs and open in browser
make a11y        # Run accessibility audit on example site
make clean       # Remove build artifacts
```

## Signed commits

All **commits and tags** must be signed. We accept SSH (recommended) or GPG signatures. Unsigned PRs are blocked by branch protection on `main`.

### One-time setup

```bash
# SSH signing — works on macOS, Linux, WSL and Windows (Git for Windows)
git config --global gpg.format ssh
git config --global user.signingkey ~/.ssh/id_ed25519.pub
git config --global commit.gpgsign true
git config --global tag.gpgsign true
```

Then register the same key on GitHub: **Settings → SSH and GPG keys → New SSH key → Key type: Signing Key**.

Prefer GPG?

```bash
git config --global commit.gpgsign true
git config --global tag.gpgsign true
git config --global user.signingkey YOUR_GPG_KEY_ID
```

### Per-repo defaults

```bash
cd static-site-generator
git config commit.gpgsign true
git config tag.gpgsign true
```

### Verify

```bash
git commit -S --allow-empty -m "chore: signature smoke test"
git log --show-signature -1
# Expect: Good "git" signature for <your email>
```

> **Tip:** with `commit.gpgsign = true` set globally you never need to remember the `-S` flag.

## Architecture

```text
src/
  lib.rs            — Orchestrator: run() → plugin pipeline → compile → serve
  lib.rs            — Orchestrator: run() → plugin pipeline → compile → serve
  main.rs           — Binary entry point (delegates to lib::run)
  cmd.rs            — CLI parsing, SsgConfig, input validation
  process.rs        — Argument-driven site processing + directory creation
  plugin.rs         — Plugin trait + PluginManager
  plugins.rs        — Built-in MinifyPlugin, ImageOptiPlugin, DeployPlugin
  frontmatter.rs    — Frontmatter extraction + .meta.json sidecars
  tera_engine.rs    — Tera template engine wrapper
  tera_plugin.rs    — Tera rendering plugin
  seo.rs            — SeoPlugin, JsonLdPlugin, CanonicalPlugin, RobotsPlugin
  ai.rs             — AI readiness (llms.txt, meta tags, alt validation)
  accessibility.rs  — WCAG checker + ARIA validation
  search.rs         — Client-side search index + localized SearchLabels
  highlight.rs      — Syntax highlighting for code blocks
  shortcodes.rs     — Shortcode expansion (youtube, gist, figure, admonitions)
  markdown_ext.rs   — GFM extensions (tables, strikethrough, task lists)
  image_plugin.rs   — Image optimization (WebP, responsive srcset)
  assets.rs         — Asset fingerprinting + SRI hashes
  deploy.rs         — Deployment adapters (Netlify, Vercel, Cloudflare, GitHub Pages)
  scaffold.rs       — Project scaffolding (ssg --new)
  schema.rs         — JSON Schema generator for configuration
  cache.rs          — Incremental build cache
  stream.rs         — High-performance streaming I/O
  walk.rs           — Shared bounded directory walkers
  watch.rs          — Polling-based file watcher for live rebuild
  livereload.rs     — WebSocket live-reload injection
  pagination.rs     — Pagination plugin for listing pages
  taxonomy.rs       — Taxonomy generation (tags, categories)
  drafts.rs         — Draft content filtering plugin
```

### Plugin lifecycle

Every build runs the plugin pipeline in this order:

```mermaid
sequenceDiagram
    autonumber
    participant CLI as ssg::run()
    participant PM as PluginManager
    participant P as Plugin (×N)
    participant SDG as staticdatagen::compile
    participant SRV as serve_site

    CLI->>PM: run_before_compile(&ctx)
    PM->>P: before_compile(&ctx)
    Note right of P: TeraPlugin emits .meta.json sidecars<br/>MarkdownExtPlugin pre-processes GFM<br/>DraftPlugin filters draft pages

    CLI->>SDG: compile(build, content, site, template)
    SDG-->>CLI: Vec&lt;FileData&gt;

    CLI->>PM: run_after_compile(&ctx)
    PM->>P: after_compile(&ctx)
    Note right of P: SeoPlugin · JsonLdPlugin · CanonicalPlugin<br/>RobotsPlugin · MinifyPlugin · SearchPlugin<br/>AccessibilityPlugin · HighlightPlugin

    CLI->>PM: run_on_serve(&ctx)
    PM->>P: on_serve(&ctx)
    Note right of P: LiveReloadPlugin injects WS script

    CLI->>SRV: serve_site(site_dir)
```

Implement whichever hooks your plugin needs — the others default to no-ops.

### Writing a Plugin

```rust
use ssg::plugin::{Plugin, PluginContext};
use anyhow::Result;

#[derive(Debug)]
struct MyPlugin;

impl Plugin for MyPlugin {
    fn name(&self) -> &str { "my-plugin" }
    fn after_compile(&self, ctx: &PluginContext) -> Result<()> {
        if let Some(config) = &ctx.config {
            println!("Site: {}", config.site_name);
        }
        Ok(())
    }
}
```

## How to contribute

### Reporting bugs

- Open an [issue](https://github.com/sebastienrousseau/static-site-generator/issues/new) with a descriptive title.
- Include steps to reproduce, expected vs actual behavior, and your OS/Rust version.

### Suggesting features

- Open an [issue](https://github.com/sebastienrousseau/static-site-generator/issues/new) describing the use case and proposed solution.

### Submitting code

1. Fork the repository.
2. Create a feature branch: `git checkout -b feat/my-feature`
3. Make your changes in `src/`. Add tests for new functionality.
4. Ensure all checks pass:

   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets
   cargo test
   ```

5. Commit with a signed, [conventional commit](https://www.conventionalcommits.org/) message:

   ```bash
   git commit -S -m "feat: add support for TOML frontmatter"
   ```

6. Push and open a pull request against `main`.

### Pull request guidelines

- Keep PRs focused on a single change.
- Include a clear description of what changed and why.
- Ensure CI passes before requesting review.
- Reference related issues (e.g., `Closes #123`).

## Testing standards

SSG enforces enterprise-grade quality gates. All PRs must pass:

### Coverage floors (CI-enforced)

| Metric | Floor |
|--------|-------|
| Regions | 95% |
| Lines | 95% |
| Functions | 95% |

Coverage is measured with `cargo llvm-cov` and enforced in CI. If a PR
drops coverage below the floor, the build fails.

### Property-based testing

The project uses [proptest](https://crates.io/crates/proptest) for
generative testing. Property tests generate 1,000+ random inputs per
property and verify invariants like "never panics" and "output is valid."

Add property tests when modifying:

- Frontmatter parsing (`src/frontmatter.rs`)
- Shortcode expansion (`src/shortcodes.rs`)
- Markdown rendering (`src/markdown_ext.rs`)
- Slug generation (`src/taxonomy.rs`)
- Cache hashing (`src/stream.rs`)

### Performance regression gates

`tests/perf_regression.rs` contains timed assertions:

| Operation | Budget |
|-----------|--------|
| 100-page compilation | < 5s |
| 50-page search index | < 500ms |
| 1000-file cache fingerprint | < 500ms |
| 10K depgraph invalidation | < 200ms |

If a change makes an operation slower than its budget, CI fails.

### Enterprise regression suite

`tests/perf_regression.rs` also validates:

- **Cache resilience**: corruption recovery, determinism, deleted files
- **Licence compliance**: SPDX headers on all `.rs` files, correct identifier
- **Localisation**: Unicode/RTL/CJK slug generation, BCP 47 codes
- **Pipeline integrity**: fused transform idempotency, plugin order

### Running tests locally

```bash
cargo test --workspace              # All tests
cargo test --test perf_regression   # Performance + enterprise gates
cargo test --test example_outputs   # Example validation (serial, slow)
```

## Code of Conduct

Please read our [Code of Conduct](.github/CODE-OF-CONDUCT.md) before participating.

## License

By contributing, you agree that your contributions will be licensed under the same dual license as the project (MIT OR Apache-2.0).
