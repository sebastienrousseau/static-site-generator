<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **`staticdatagen` 0.0.17 → 0.0.18.** The upstream file walker now sorts
  directory entries by name, so tag pages list their members in the same
  order on APFS and ext4. This was the last cross-platform ordering source
  the golden suite had found, and it was outside this repository.
- **`frontmatter-gen` 0.0.6 → 0.0.10.** The four versions between them were
  tagged and published today (they had been bumped on that crate's `main`
  without a release). 0.0.10 moves YAML scalars into `Value` instead of
  copying them; the sidecar goldens and the `emit_sidecars` heap gate are
  unchanged by it. It pulls `noyalib` 0.0.26, a third incompatible 0.0.x
  copy beside 0.0.15 and 0.0.19; collapsing those is a separate bump.
- **cargo-vet trusts `frontmatter-gen`'s publisher**, the owner's user id,
  with the same window as `noyalib` and `staticdatagen`.
- **cargo-vet trusts staticdatagen's release workflow.** From 0.0.18 the
  crate is published through crates.io Trusted Publishing, so its publisher
  record is `github:sebastienrousseau/staticdatagen` rather than the owner's
  user id. `supply-chain/audits.toml` gains a `trusted-publisher` entry with
  the same criteria and end date as the user-id entry; the exemption count
  moves 506 → 505 (one Mozilla import replaced an exemption on refresh).

### Testing

- **`search-index.json` is back in the per-example golden list**, full
  content, for all eight examples and both feature sets. It had been
  scoped down to an entry-set view (`search_index_entry_urls.golden`) while
  the upstream ordering made a macOS-seeded golden fail on Linux; that view
  stays as the readable first diff, and the full snapshot is pinned again.
  The goldens are seeded and verified under Docker `rust:1.90` on Linux.

## [0.0.59] - 2026-09-04

A patch release for a regression that 0.0.58 shipped, plus the golden
that should have caught it and a theming fix the same investigation
turned up.

### Fixed

- **Generated pages no longer open with the starter templates' licence
  comments.** `templates/tera/*.html` carried their SPDX header as an HTML
  comment, so every rendered page began with that licence comment — twice on
  index pages, since both the child template and `base.html` emitted one —
  before the doctype. The header is now a Tera comment and the newline after
  it is consumed, so the source file stays REUSE compliant and the doctype is
  the first byte of the output. The one-line golden snapshots of those pages
  also failed `reuse lint` 6.x, which reads a comment closer only at end of
  line.
- **Authored markup inside `<pre><code>` is no longer escaped.** 0.0.58
  escaped `<` and `>` inside every bare `<code>` element, so a theme
  shipping hand-highlighted code had its `<span class="code-kw">` tags
  printed on the page as visible text. `escape_markup_inside_code_spans`
  exists to repair markdown *inline* spans -- the legacy compiler renders
  `` `<img>` `` as a live element -- but its guard also matched
  `<pre><code><span ...>`. A bare `<code>` opening a `<pre>` now passes
  through untouched. Markdown-derived blocks are unaffected either way:
  they carry `<code class="language-x">`, which the pass never matched.

### Performance

- **`emit_sidecars` peak heap on a 10,000-page site: 1,927 KiB → 941 KiB
  (−51%).** The function collected a sorted `Vec<PathBuf>` of every content
  file and held it for the whole pass; measured with a counting allocator,
  that vector *was* the peak — no single document's parse and serialisation
  ever exceeded it. It now streams the walk, sorting each directory's names
  before descending, so the order is unchanged on every platform and the
  footprint is one listing of names rather than the tree of paths. A new
  workspace member, `ssg-heap-probe`, measures this on the fixture and a
  root test asserts ≤60% of the recorded baseline. Wall-clock followed:
  the `frontmatter::emit_sidecars` benchmark went from 619.72 µs to
  232.55 µs, and the `fuzz_frontmatter` target ran 5.86 million cases in
  five minutes against the new path without a crash (#578).

### Added

- **`data-ssg-search`**, an optional placeholder a theme can put in its
  header to say where the search trigger belongs. Without one the trigger
  stays `position: fixed` in the viewport corner, where it cannot line up
  with a header it is not inside; the offsets that existed to compensate
  could never solve the horizontal case, because the control it should sit
  beside is at the content container's edge, not the viewport's. A theme
  that provides no slot is byte-for-byte unaffected.

### Testing

- A golden covering code-block post-processing (#466). The golden suite
  existed while the escaping regression shipped, because not one of its
  seventeen goldens contained a `<pre>` block: it was green throughout and
  simply never exercised that output. The new golden pins both directions
  at once -- block markup survives, inline spans still escape -- since a
  fix for either is an easy way to break the other.
- The end-to-end golden now fails when a listed artefact is missing
  instead of skipping it. The loop passed over anything the build did not
  emit, so an entry could be added for a file the pipeline never produces
  and read as coverage while asserting nothing.

## [0.0.58] - 2026-09-04

The trust-the-gates release. Repository-standard Phases 1 and 2 — the
developer entry point and the Unix install contract — and a run of
defects that all had one shape: something reporting success while
asserting nothing.

Fixed in that class: 29 integration test files that no workflow ran, a
golden test that skipped on every run and had no golden file, a
benchmark corpus missing front matter the templates read, a `--new` flag
declared since 0.0.42 and never dispatched, and a fuzz corpus that was
committed but never replayed. Each now has a gate that was watched
failing before it was trusted.

### Added

- **Generated man page and shell completions** — `ssg.1` plus bash, zsh,
  fish and PowerShell scripts, all walked out of the clap definition
  (`src/cmd/man.rs`, `src/cmd/completions.rs`) rather than hand-written,
  so `man ssg` cannot drift from `ssg --help`. Neither uses a crate:
  `clap_complete` and `roff` are both unaudited against this
  repository's `cargo vet` policy, whose exemption ratchet may only move
  downward, so a narrow in-tree emitter costs less than the audit it
  avoids. Exemptions stay at 506.

- **`GNUmakefile`** — `make install` / `make uninstall` honouring
  `PREFIX` (default `/usr/local`) and `DESTDIR`, installing to FHS
  paths. Developer targets stay in `Makefile` and are forwarded, because
  GNU make reads `GNUmakefile` *instead of* `Makefile` and would
  otherwise hide them.

- **`DEVELOPMENT.md`** — the single developer entry point, built around a
  table mapping every CI job to the exact command that reproduces it.
  `tests/development_docs.rs` gates the table against `ci.yml` in both
  directions, because a stale reproduction table is worse than none: a
  contributor runs it, sees green, and pushes red.

- **`install contract` and `docs lint` CI jobs**, plus
  `scripts/install-smoke.sh` and `scripts/repo-hygiene.sh`, which CI runs
  verbatim so local and CI share one path rather than two that resemble
  each other.

### Fixed

- **`ssg --new` produced a project `ssg` could not build** (#752). Three
  defects stacked: the flag was declared and never dispatched; the
  scaffolder wrote only the MiniJinja templates and none of the four
  StaticWeaver root templates the compile step reads; and the base
  template assumed `page` exists, which taxonomy pages do not provide.
  `ssg --new mysite && ssg -f config.toml` now exits 0 with 47 files,
  from 0.

- **Financial identifiers were written to logs in cleartext**
  (code-scanning #242–#244). IBANs and BICs are published in the emitted
  JSON-LD on purpose; a build log is a different channel. Redacted in
  logs only — output is unchanged.

- **Three packaging manifests pinned 0.0.37** — twenty-one releases of
  drift in Scoop, WinGet and the AUR `PKGBUILD`, with nothing comparing
  them to the crate. Corrected, and `tests/release_versions.rs` now
  fails when they disagree.

- **The fuzz lockfile was gitignored**, so CI re-resolved every
  dependency on every run and a freshly published `tinyvec 1.13.0` that
  does not compile broke the OSS-Fuzz build for a commit that touched
  one test file. Now committed, as it should be for a workspace of
  binaries.

- **Completions marked every option as a bare flag.** `get_num_args()`
  is `None` for every argument in this parser, so reading it as "takes no
  value" dropped `-r` from the fish specs and `:DIR:_files` from the zsh
  ones — the shell offered the next option where a path belonged. The
  signal is `get_action()`.

### Changed

- **`docs/adrs/` is now `docs/adr/`**, per the repository standard, with
  every reference across eleven files moved with it.

## [0.0.57] - 2026-08-22

Three defects that produced wrong output, one new crate, and a duplication
that had been costing CI time for a week.

### Fixed

- **Commented-out blocks reached CSP and the filesystem** (#721). Three
  functions scanned for the literal bytes `<script` / `<style>`, which match
  inside HTML comments. `collect_inline_contents` hashed dead code into the
  policy, so CSP stopped describing the document. Worse,
  `find_inline_script` and `find_inline_block` *hoist* what they match into
  external files and rewrite the page around it — a commented-out script
  became a real file. Each was proven with a failing test before any code
  moved.

- **Meta tags were read by byte scan** (#719). A commented-out
  `<meta name="twitter:image">` left over from an edit beat the live tag
  that followed it.

- **`<head>` injection took the first byte match** (#720). `oembed` and
  `view_transitions` spliced at `html.find("</head>")`, so a `</head>`
  inside a comment or script body in the head captured the payload — inert,
  and silently, because the document still parses.

### Added

- **`ssg-mcp`** (#723): a Model Context Protocol stdio server over the
  existing `#[ssg_rpc]` registry. Tools are not declared; the registry is
  walked at runtime, so a tool added to ssg appears over MCP with no second
  declaration. Note that `tools/list` reflects what the *host binary*
  linked: ssg itself registers no production RPC yet, so the five tools
  #576 names remain to be written.

- Benchmarks for the HTML paths that moved to a parser, with a committed
  criterion baseline, and `tools/quality_scorecard.py` — 25 measured
  metrics that report `unmeasured` rather than guessing.

### Changed

- **One tag-end scanner instead of four** (#718). The copies were
  byte-identical apart from visibility, and when clippy 1.98 added
  `missing_const_for_fn` the lint fired on each separately — four
  sequential CI round-trips for one function.

- **One crate-level test lint allowance instead of 150 copies.** `src/lib.rs`
  already granted it; the repetitions were redundant, and the six workspace
  crates now carry the same line.

### Performance

- CSP hashing walked the document once per tag. `collect_inline_script_and_style`
  does both in one pass: 51.082 µs → 42.596 µs, which recovers the cost of
  the correctness fix and lands at parity with the byte scan it replaced.

## [0.0.56] - 2026-08-21

Three defects that only appeared in configurations nobody was checking.

### Fixed

- **The placeholder site title reached rendered pages** (#705). `DEFAULT_SITE_TITLE`
  was `"My SSG Site"`, so a site that never set `site_title` shipped that string
  in its `<title>` — 7,189 live pages on one corpus. The unit test asserting the
  default pinned the placeholder itself (`assert_eq!(default, "My SSG Site")`), so
  it stayed green the entire time it was wrong. The default is now empty, and the
  templates omit the suffix rather than inventing branding.

- **A configured site title was dropped without the `templates` feature** (#705).
  `default = ["templates", ...]`, so `--no-default-features` selects the
  hand-written renderer — and `site_title` was read only in the minijinja path.
  The same config therefore produced differently branded pages depending on how
  the binary was compiled. The fallback now mirrors the template exactly, and the
  tests covering it are deliberately ungated so both renderers must agree.

- **Entity-encoded `<title>` text reached consumers double-escaped** (#705).
  `lol_html` hands text through without unescaping, so `extract_title` returns
  encoded text; decoding is now explicit, with `&amp;` decoded last so a literal
  `&amp;lt;` in a title does not collapse into `<`.

### Testing

- Every shipped example is now exercised. The per-example assertions covered 7 of
  18; a fleet sweep runs the rest and pins the discovered set against cargo's own
  view, so a newly added example cannot go untested silently.

## [0.0.55] - 2026-08-21

Four fixes. Three were shipping corrupted output while looking healthy.

### Fixed

- **`og:title` double-escaped `&` since 0.0.46** (#706, #708). `escape_attr`
  was naive, and the SEO plugin applies it to values the template layer has
  already escaped, so `&` reached the page as `&amp;amp;`. Bisecting the
  published binaries against one identical input file puts the first bad
  release at **0.0.46** — the release that *closed* #589, the same symptom.
  That issue was only half-fixed: `staticweaver` was made entity-aware and
  this call site was left alone, with no test asserting
  `escape(escape(x)) == escape(x)`. There is one now. On a 34-locale corpus
  this failed a no-double-encoding assertion on 1,494 pages.

  Two siblings fell out of the same trace. `lol_html` passes text chunks
  through "as-is, without unescaping", so everything built on
  `extract_title` inherits encoded text: `og_image` escaped it again into
  the generated preview SVG, and the search index decoded `content` but not
  `title` or `headings`.

- **Slug-colliding taxonomy terms overwrote each other** (#710). Terms were
  ordered by `to_lowercase()` over a `HashMap`, a stable sort over a
  randomised iteration order, so ties broke differently per process and the
  build was not reproducible. Distinct spellings that slugify alike —
  `SWIFT`/`Swift`, `CBPR`/`CBPR+` — rendered to one path and the survivor
  was a coin flip, silently dropping the other spelling's pages. Measured on
  a real corpus: 595 colliding slugs, 340 lowercase ties.

- **Search results linked to the host root** (#712). 0.0.54 fixed loading
  the index; it did not fix where a result sends you. `lp` was hardcoded to
  `/`, so on a site served under a path prefix every result 404'd — while
  the widget looked entirely healthy, which is why it went unnoticed.

- **Structured data was requested from a fragment** (staticdatagen 0.0.13).
  The step ran against the Markdown body, which has no `<head>` and so can
  never carry a `<title>`; it failed on every page ever compiled and logged
  at Error each time. Structured data is generated downstream from front
  matter, where the values are actually known.

### Changed

- `tests/example_outputs.rs` compiles each example *outside* its 30-second
  timeout. The budget previously had to cover compilation — ~100ms of work
  behind a multi-minute cold build — so CI reported `public dir not created`,
  indistinguishable from a generator bug and green on any warm machine.

## [0.0.54] - 2026-08-20

Ships the search fix from #707, which landed on `main` after 0.0.53 was
already published and so reached no release.

### Fixed

- **Search fetched its index from the host root, not the site** (#707).
  The widget requested a bare `/search-index.json`, so a site published
  under a path — `https://example.com/apex` — asked the *host* root for an
  index that is not its own.

  It failed quietly and with plausible results. On a host where something
  else answers at `/search-index.json`, search returned that other site's
  entries rather than erroring: no 404, no console message, results that
  looked real. The themes showcase was served under a domain whose root
  does serve an index, so every theme's search had been querying unrelated
  content. It surfaced only when the showcase moved to a host with nothing
  at the root.

  The index URL now carries the path component of `base_url`, as the
  `_csp/` assets, islands loader, SBOM link and taxonomy home link already
  did. A site that owns its host is unaffected.

- **Eight clippy findings from Rust 1.98** (#707). The runners moved to
  1.98 and `missing_const_for_fn` (6 sites), `chunks_exact_to_as_chunks`
  and `map_or_identity` began firing on pre-existing code. Behaviour is
  unchanged; `as_chunks` additionally replaces a runtime length assumption
  with a type guarantee.

### Notes

Anyone publishing a site under a path — a project page, or several themes
under one domain — should take this release: on 0.0.53 and earlier their
search either 404s or silently returns another site's results.

## [0.0.53] - 2026-08-20

A one-line fix for a defect shipped in 0.0.52.

### Fixed

- **`SSG_NO_TAG_PAGES=1` aborted the build** (#702). `--no-tag-pages` used
  `ArgAction::SetTrue` together with `.env()`. clap parses an environment
  variable as a *value*, and its default bool parser accepts only
  `"true"`/`"false"`, so the conventional form failed outright:

      error: invalid value '1' for '--no-tag-pages'
        [possible values: true, false]

  The flag form was unaffected, which is exactly why 0.0.52 shipped this way:
  `--no-tag-pages` was tested thoroughly and the environment variable
  advertised alongside it was never exercised end to end. It surfaced within
  minutes of using the feature on a real site.

  The variable now accepts `1`/`true`/`yes`/`on` and `0`/`false`/`no`/`off`,
  case- and whitespace-insensitive. An unrecognised value is an **error**
  rather than a silent false — `SSG_NO_TAG_PAGES=ture` should say so, not
  quietly generate the pages the operator asked to skip.

  Anyone using the environment variable from 0.0.52 needs this release; the
  `--no-tag-pages` flag works in both.

## [0.0.52] - 2026-08-19

Two tag-handling defects and one capability, all found by building a
35-locale site against v0.0.50 and v0.0.51.

### ⚠️ Behaviour change

**Tag lists now split on non-ASCII separators.** If any post's `tags:`,
`categories:` or `topics:` field separates terms with `،` (Arabic comma),
`，` or `、` (CJK) or `;`, those terms were previously collected as **one**
term whose name was the entire list. They are now split correctly, so the
generated tag tree changes shape on upgrade: more terms, each shorter, and
the old combined term disappears.

This is the fix rather than a regression, but it is a visible output change:
existing URLs under `tags/` derived from a mis-split term will not be
regenerated. Sites that publish only ASCII-separated tags are unaffected.

### Fixed

- **A multilingual tag list could kill the build** (#695). Term lists were
  split on ASCII `,` alone, so a post written in Arabic — which separates
  with `،` (U+060C) — had its whole list collected as a single term. That
  term then slugified into one enormous path component: 230 characters, but
  **348 UTF-8 bytes**, because `is_alphanumeric` is Unicode-aware and
  non-Latin scripts survive at 2–4 bytes per character. ext4 caps a path
  component at 255 *bytes*, so the build aborted with
  `File name too long (os error 36)`. APFS caps at 255 *characters*, so the
  same site built cleanly on macOS and only Linux CI failed — which is how
  it shipped. `split_terms` in `ssg-core` now recognises `,` `،` `，` `、`
  and `;`, and the four call sites that each open-coded `split(',')` share
  one definition.
- **`slugify` had no length cap** (#695). Fixed independently of the
  separator bug, because it is reachable without it: any sufficiently long
  legitimate term hits the same 255-byte wall. Slugs are now truncated to
  200 bytes on a character boundary — byte-slicing a multi-byte sequence
  panics — with a dangling separator trimmed from the cut.

### Added

- **`--no-tag-pages` / `SSG_NO_TAG_PAGES`** (#696), with a
  `no_taxonomy_pages` config field. Skips taxonomy page generation entirely,
  for sites that curate their own vocabulary. One 35-locale site declares
  825 distinct raw tags in English alone and collapses them to a canonical
  53 with a ≥3-article threshold before a term earns a landing page; ssg
  cannot know that decision exists and emitted ~7,172 pages against 6,856
  real content pages, roughly doubling the URL surface with the thin half.
  **The default is unchanged** — the flag only ever turns generation off, so
  no existing build changes behaviour by upgrading. Verified byte-identical
  output with the flag absent.

## [0.0.51] - 2026-08-16

The follow-through release. Every item below was found by building real
sites against v0.0.50 rather than by reading the code, and several were
silent — producing wrong output with no error at all.

### Fixed

- **Taxonomy was locale-blind** (#680). Every locale's pages were collected
  into one index at `tags/`, so an English-language tag page listed French
  pages beside English ones, and a French reader had no tag index at all.
  Entries now group by locale, inferred from the first path segment of each
  page's URL. The default locale keeps the root path, so existing links and
  sitemaps do not move; other locales get their own prefix (`fr/tags/`).
  Each tree renders through a locale-scoped renderer in both the MiniJinja
  and no-templates paths: `site.language` follows the tree, navigation uses
  a locale-scoped prefix, and `page_url` carries the locale segment so a
  canonical points at the file actually written. Single-locale sites are
  unaffected.
- **Islands vanished from minified pages** (#680). `extract_island_components`
  matched the literal `component="`, but `html-generator` minifies some
  pages during generation and strips quotes it does not need, so
  `component=feature-tabs` matched nothing. The bundle was never copied, the
  component never reached `_islands/manifest.json`, and the page served its
  static fallback for ever without erroring. The extractor now accepts
  `a="v"`, `a='v'` and bare `a=v`, and refuses to match a longer attribute
  that merely starts with the name.
- **GPG signatures never reached the release** (#678).
- **`multilingual_full` failed the HTML invariants gate** (#677).

### Added

- **`site_prefix` in taxonomy templates** (#680). `url_prefix` is
  locale-scoped; assets are not. A template building asset URLs from the
  scoped prefix asked for `/atlas/fr/styles.css`, which does not exist.
- **Coverage now measures both feature configurations** (#683). The job ran
  only with default features, so `#[cfg(not(feature = "templates"))]` code
  never entered the coverage binary and counted as uncovered on every diff.
  A second `--no-default-features` pass is accumulated into the same report.
- **3,446 library tests that had never been compiled** (#683).
  `cargo test --lib --no-default-features` did not build: `src/core/lang.rs`
  imported `HashMap` behind the `templates` feature while its test module
  used it ungated, and a taxonomy test called a templates-only function. The
  feature-powerset job runs `cargo check`, which does not build test code,
  so nothing had ever compiled them.
- **CI gates that scan nothing now fail** (#681), and Miri failures break
  the build rather than being reported and ignored.

### Changed

- **Minification's ordering is documented accurately** (#682). Two comments
  claimed it "must be last content transform". It is registered last, but
  `MinifyPlugin` only implements `after_compile`, and every `after_compile`
  hook runs before any `transform_html` — so it rewrites markup that later
  transforms then read. The comments now say so, and record that
  `html-generator` minifies some pages before any plugin runs at all, which
  no plugin ordering can affect.

### Notes

`v0.0.50` was published from a commit predating the two fixes above, so a
site using taxonomy or islands should move to `0.0.51`. Themes pinning
`min_version = "0.0.50"` continue to work; the floor is unchanged.

## [0.0.50] - 2026-08-14

The theming release. Building three real themes against v0.0.49 surfaced
four defects that made documented features unusable — each failing silently,
which is why none had been reported. Multi-locale sites additionally gain
translated slugs.

It also repairs the release pipeline itself. Nothing had published cleanly
since v0.0.46: crates.io was two versions behind the README's own install
instruction, and v0.0.49 produced no artefacts at all.

### Fixed

- **The release pipeline could not publish.** Three separate faults, each
  masking the next:
  - Workspace members were frozen at `0.0.47` while the root crate advanced
    to `0.0.50`, so the first `cargo publish -p ssg-core` of every release
    aborted with `already exists on crates.io index`. All six members and
    the four root dependency pins now track the root version, and
    `tests/release_versions.rs` fails the build if they drift again —
    before a tag is cut rather than after.
  - The step's own idempotency guard, which should have absorbed that,
    probed `https://crates.io/api/v1/crates/<crate>/<version>` over HTTP.
    That request failed from the runner during v0.0.48 (it returns 200 from
    a developer machine), so a crate that *was* on crates.io was reported
    missing, the step exited 1, and `Publish ssg to crates.io` never ran.
    The guard now reads cargo's own diagnostic and needs no network.
  - The GHCR job exported its build cache to the GitHub Actions Cache
    service, which failed with `ERROR: not_found` after a 126.9s export —
    a green image build that shipped no image. The cache moved to GHCR
    itself, with `ignore-error=true` so a cache fault can never fail a
    release again.
- **A release tag started two heavyweight workflow runs.** `scheduled.yml`
  triggered on `v*` tags as "release-gating coverage", but nothing in
  `release.yml` waited on its result, so it gated nothing while doubling
  the load. Both runs were cancelled together at v0.0.49, leaving that tag
  with no release. The tag trigger is gone, and `release.yml` gained a
  `concurrency` group with `cancel-in-progress: false` so a release is
  never silently superseded mid-publish.
- **`examples/multilingual_full` audited the pre-0.0.50 output layout.** It
  expected each locale home page at `<lang>/index/index.html` — the extra
  directory level this release removes — and so reported all five as
  missing on every run. It now checks `<lang>/index.html`, and additionally
  asserts that the translated-slug pages are *reciprocally linked* rather
  than merely present; unpaired translations fail the example hard, in CI
  included.
- **`layout` was ignored on every page** (`src/plugins/template_plugin.rs`).
  `before_compile` writes front-matter sidecars to `<build_dir>/.meta`, but
  `staticdatagen` promotes `output.build-tmp` onto `output` once the compile
  finishes and takes `.meta/` with it. `after_compile` still read the old
  path, found nothing, and fell back to `layout = "page"` for every page — so
  a theme whose layouts were `index`/`about`/`contact` rendered through none
  of them, and through nothing at all without a `page.html`. The sidecar
  directory now resolves from `build_dir` and falls back to `site_dir`.
  `TemplateEngine::has_template` lets the plugin distinguish a real render
  from `render_page`'s pass-through arm, which it had been counting as a
  success — a pipeline doing nothing logged `Rendered N page(s)` exactly like
  a working one.
- **`content/content.schema.toml` broke the build it configures**
  (`src/core/content_stager.rs`). The documented location for typed
  front-matter schemas was staged as a page, and `staticdatagen` aborted with
  `Failed to extract metadata: No valid front matter found`. Build-time
  control files are now excluded from staging.
- **Nested `index.md` gained a directory level**
  (`src/core/content_stager.rs`). `fr/index.md` compiled to
  `fr/index/index.html` rather than `fr/index.html`, so every locale home
  page was wrong. The root cause is upstream — `staticdatagen`'s
  `write_files_to_build_directory` compares the whole processed name against
  `"index"` — so this is a staging-time side-step, not a fix, and it leaves
  the file alone when both `fr.md` and `fr/index.md` are authored.
- **Extracted CSS and JS 404'd on sub-path deployments**
  (`src/plugins/csp.rs`). Inline blocks are externalised into fingerprinted,
  SRI-signed `_csp/` files referenced as `/_csp/…`, which resolves against
  the domain root. On a GitHub Pages project site the whole stylesheet was
  lost. The prefix now derives from `base_url`'s path component; sites at the
  domain root are unaffected.
- **Markdown tables broke reflow on every phone width**
  (`src/plugins/postprocess/html_fix.rs`). A table cannot reflow — its
  columns have a minimum width — so a five-column Markdown table pushed the
  document to 588px inside a 320px viewport, failing WCAG 1.4.10. Tables are
  now wrapped in a focusable, labelled scroll container, which is the
  accepted remedy. Applied in the generator because Markdown-generated
  tables have no wrapper a theme could style.
- **Generated taxonomy pages had no skip link** and no focus styling
  (`src/plugins/builtin_templates/base.html`). Every authored page opened
  with one; the generated ones dropped a keyboard user straight into the
  navigation. These pages link no theme stylesheet, so the link carries its
  own rules, using system colours so it survives forced-colours mode.
- **The injected search trigger sat on top of theme header controls**
  (`src/plugins/search.rs`). It is pinned to the top-right, which is exactly
  where a themed site puts its own controls; measured across a 13-viewport
  matrix it overlapped the navigation toggle, the theme toggle and the
  language switcher at every phone width, so a tap landed on whichever won
  the z-order. Below 48rem it now sits as a 44px circle in the bottom
  corner — the conventional mobile affordance, and out of the header's way.
- **Markdown table alignment emitted obsolete `align` attributes**
  (`src/plugins/postprocess/html_fix.rs`,
  [#618](https://github.com/sebastienrousseau/static-site-generator/issues/618)).
  Column-alignment syntax (`:---`, `---:`, `:---:`) rendered as
  `<th align="left">` / `<td align="right">`. `align` has been obsolete
  since HTML5 and pa11y flags it as
  `WCAG2AAA.Principle1.Guideline1_3.1_3_1.H49.AlignAttr`. The attribute is
  now replaced by an equivalent `text-left` / `text-center` / `text-right`
  class, so the alignment survives — and `<th>` gains a class it never had,
  which is what makes header alignment stylable at all. Done with a real
  parser, since an `align=` literal inside a `<pre>` block is content, not
  markup.
- **Islands never hydrated** (`src/plugins/islands.rs`,
  `src/plugins/assets.rs`). Three independent faults, each sufficient alone:
  the injected loader tag was root-absolute like `_csp/` above; the loader
  resolved component bundles with a root-absolute dynamic `import()`, now
  resolved relative to its own module URL so the runtime needs no knowledge
  of the mount point; and the asset fingerprinter renamed `_islands/*.js`
  without being able to rewrite a specifier built at runtime, so every bundle
  404'd. `_islands/` is excluded from fingerprinting.

### Added

- **Translated slugs across locales** (`src/plugins/i18n.rs`). Pages were
  paired across locales by identical relative path, so `about/index.html`
  and `a-propos/index.html` were two unrelated singletons and **neither
  received any `hreflang`** — silently, because from the plugin's view they
  were simply untranslated. Pages now declare a `translation_key` in front
  matter, read from the sidecars; the locale matrix inverts from
  `rel_path -> {locale}` to `key -> {locale -> rel_path}`. Pages without a
  key keep path matching, so existing sites are unaffected.
- **Taxonomy pages work for the first time** (`src/plugins/taxonomy.rs`).
  `resolve_user_template_dir` fell back to `<template_dir>` itself when no
  `tera/` existed, so MiniJinja was handed the theme's StaticWeaver
  `base.html` and aborted the **whole build** with
  `syntax error: unexpected character (in base.html:26)`, attributed to
  `tag.html` — a file the author never wrote. Taxonomy was therefore
  unusable for any theme using the default page-layout engine. User
  templates now come from `tera/` only; a theme without one gets the
  built-in fallbacks and a site that builds.

  Three further fixes on top: term-page URLs and the index's term links are
  prefixed from `base_url`, so they resolve on a sub-path deployment rather
  than 404ing; the built-in base template emits a Content-Security-Policy,
  which generated pages previously lacked while every authored page had one;
  and the index's term links are marked `| safe`, since autoescape was
  rendering `/` as `&#x2f;`.
- **Derived path globals** (`src/core/content_stager.rs`). Templates can
  reference `{{site_path}}`, `{{site_url}}`, `{{locale_path}}` and
  `{{locale_url}}`; the stager derives each from `base_url` and the page's
  own location, so pages no longer hand-maintain them. Two scopes, named at
  the call site: `site_*` addresses the site root, where assets, feeds, the
  manifest and the favicon are published once regardless of locale;
  `locale_*` addresses the current locale's root, where page links live.

  Conflating them is not hypothetical — a hand-maintained pair carrying no
  scope in either name had French pages requesting `/atlas/fr/styles.css`,
  which is never written. `{{site_path}}styles.css` and
  `{{locale_path}}articles/` both read correctly;
  `{{locale_path}}styles.css` reads visibly wrong.

  Values are injected only when a template actually references them, and
  author front matter always wins. On a single-locale site `locale_*` equals
  `site_*`, so a theme can use the locale forms throughout and gain locales
  later without editing content. Removed 60 hand-maintained fields and 20
  hardcoded permalinks from the reference themes.
- **Theme compatibility is enforced** (`src/core/theme_manifest.rs`). A theme
  declares the oldest generator it works with — `min_version` in
  `theme.toml`, or `min_ssg_version` in `theme.json` — and nothing read it.
  That mattered because every way a too-old generator breaks a theme is
  silent: the `layout` named in front matter ignored, a bundled
  `content.schema.toml` aborting the compile, extracted CSS 404ing under a
  sub-path. The build now stops at the start with one message naming both
  versions and the manifest that declared the floor. A theme with no
  manifest, no `min_version`, or an unparseable one imposes no floor and
  builds as before.
- **Root-hosted default locale** (`src/plugins/i18n.rs`). Every locale
  previously needed its own directory, including the default, which forced
  `/en/about/` and left the site root empty. The default locale may now live
  at the root — `/about/` alongside `/fr/a-propos/` — matching Hugo's
  `defaultContentLanguageInSubdir = false`, Astro's `prefixDefaultLocale:
  false` and Next.js's default.

### Changed

- **Alternate `hreflang` labels now describe the target document.** An
  English page labelled its Hindi alternate `hreflang="hi"` (the locale
  directory) while the Hindi page advertised `hreflang="hi-IN"` for itself.
  The two sides disagreed, failing reciprocity and the `hreflang` audit gate.
  Alternates now carry the target's resolved language. Two tests asserting
  the old asymmetry were updated, with the reasoning recorded in their
  bodies.
- **`x-default` is emitted only when the default locale serves the page**,
  rather than pointing at a URL that may not exist.

### Documentation

Translated slugs shipped with no user-facing documentation: on merge,
`translation_key` appeared in exactly two files in the repository — the
plugin source and this changelog. The `docs_accuracy` gate did not catch it
because it verifies numeric claims (version strings, test counts, coverage
floors, MSRV), not whether a feature was described anywhere.

- **`docs/guide/i18n.md`** documented the identical-path pairing model this
  release replaced, illustrated with `/en/about` ↔ `/fr/about`. Rewritten
  against actual behaviour: a new *Page Pairing* section covering
  `translation_key` and the path fallback, a *Root-hosted default locale*
  section giving the three conditions detection requires, and an
  explanation of how each `hreflang` value is chosen and why reciprocity
  depends on it. The sitemap and language-switcher sections were corrected
  to match — the switcher lists a locale only when a paired page exists.
- **`docs/guide/content.md`** gained `translation_key` in the standard
  front-matter field table, where an author would actually look for it.
- **`src/plugins/i18n.rs`** module docs now explain pairing, root-locale
  serving and reciprocity. The existing rustdoc was accurate but attached to
  private items, so none of it reached docs.rs.
- **`README.md`** describes the feature in the i18n row rather than only
  bumping its version badge, and the examples table lists
  `multilingual_full`, which was absent.

### Security

- **js-yaml 4.3.0 → 4.3.1** in the `tests/a11y` harness
  ([GHSA-5p4m-2wfm-xmqj](https://github.com/advisories/GHSA-5p4m-2wfm-xmqj),
  high): quadratic CPU consumption resolving `!!omap`. Development-only — the
  harness is not part of the published crate.
- **`extract-zip` 2.0.1 symlink path traversal**
  ([GHSA-jmr9-qjv8-65gv](https://github.com/advisories/GHSA-jmr9-qjv8-65gv),
  high) is **accepted, not fixed**. No patched version exists: the advisory
  covers `<= 2.0.1` and upstream has published no fix. The path is
  `pa11y → puppeteer → @puppeteer/browsers → extract-zip`; `pa11y` 9.1.1 is
  the latest release and pins `puppeteer ^24.37.5`, which requires
  `@puppeteer/browsers` 2.x. Forcing `@puppeteer/browsers` 3.x — which
  replaced `extract-zip` with `modern-tar` — resolves the advisory but breaks
  installation, because puppeteer 24.x calls the 2.x API (`downloadBrowsers`
  fails in `puppeteer/lib/esm/puppeteer/node/install.js`). Verified, then
  reverted.

  Risk accepted on the basis that the harness is development-only and never
  part of the published crate, and that the only archive it extracts is the
  Chromium build downloaded from Google's CDN over HTTPS — not
  attacker-controlled input. Revisit when `pa11y` moves to puppeteer 25.x.

### Dependencies

- oxc family 0.142 → 0.143 (`minifier`, `parser`, `codegen`, `allocator`,
  `span`), bumped in lockstep as the crates require.
- `noyalib` 0.0.17 → 0.0.18, `@playwright/test` 1.62.0 → 1.62.1, and the
  minor/patch group across the resolved graph.
- GitHub Actions: `github/codeql-action/*` v4.37.4 → v4.37.6,
  `actions/attest-build-provenance` v4.1.1 → v4.2.2,
  `Swatinem/rust-cache` to 6323deb1. `scheduled.yml` had been pinning
  `attest-build-provenance` independently of `release.yml` and drifting
  behind it; both now match.
- `deny.toml`: removed five `ignore` entries that no longer match any crate
  (RUSTSEC-2025-0057, -2025-0119, -2026-0173, -2026-0194, -2026-0195). The
  quick-xml removal plan recorded there on 2026-07-04 has completed. Each
  unmatched entry raises an `advisory-not-detected` warning, so the stale
  ones were burying any real one.

## [0.0.49] - 2026-08-05

Dependency maintenance only; no functional changes. Recorded here because
the release shipped without a changelog entry.

### Changed

- `oxc_minifier` and `oxc_allocator` 0.138 → 0.141 (lockstep family bump)
  ([#626](https://github.com/sebastienrousseau/static-site-generator/pull/626),
  [#627](https://github.com/sebastienrousseau/static-site-generator/pull/627)).
- `tungstenite` 0.29 → 0.30
  ([#624](https://github.com/sebastienrousseau/static-site-generator/pull/624)).
- Minor and patch group across 9 further dependencies
  ([#631](https://github.com/sebastienrousseau/static-site-generator/pull/631)).
- CI action bumps: `actions/checkout`, `actions/setup-node`,
  `github/codeql-action`, `ossf/scorecard-action`,
  `docker/setup-buildx-action`, `@playwright/test`.

## [0.0.48] - 2026-07-25

### Planned / Upcoming (deferred — not v0.0.46 scope)

- **Residual content-staging shim removal** — two narrow gaps remain in `src/core/content_stager.rs` until upstream follow-ups land: template-default injection (blocked on [staticdatagen#99](https://github.com/sebastienrousseau/staticdatagen/issues/99) — opting the staticweaver Engine into `lax_undefined`) and multi-line quoted-scalar collapse (blocked on [staticdatagen#100](https://github.com/sebastienrousseau/staticdatagen/issues/100) — bumping the transitive `metadata-gen` dep to `0.0.5`). Once both land, the module collapses to ~50 LOC.
- **Complete internal anyhow elimination** across 9 core modules (`cache`, `collections`, `content`, `depgraph`, `deploy`, `frontmatter`, `scaffold`, `stream`, `template_engine`) and 7 plugin modules (`ai`, `csp`, `llm`, `postprocess/{helpers,html_fix}`, `seo/{canonical,seo_plugin}`). `scaffold.rs` is the heaviest module in this sweep (14 uses). Once complete, `anyhow` will be dropped from the library's `[dependencies]` list in `Cargo.toml`.
- **Ratchet CI coverage floor to ≥98.0%** (regions, lines, functions). Currently at 95.71 / 96.87 / 95.77 with `--lib`. The remaining uncovered regions sit in I/O-heavy production glue that needs source-level seams.

## [0.0.47] - 2026-07-04

The trust release: every headline claim is now byte-verifiable in code, the
site-migration correctness defects (spec A1–A7) are fixed end-to-end with a
cross-platform determinism gate, and the three largest performance wins
landed with measured evidence. Implements the
[v0.0.47 plan](docs/plans/v0.0.47-implementation-plan.md) and tracker
[#586](https://github.com/sebastienrousseau/static-site-generator/issues/586).

### Added

- **Flexible date parsing** (`src/core/dates.rs`, spec A4): RFC 2822 →
  long-form (`July 1, 2026`) → ISO 8601, zero new deps, proptest
  round-tripped; wired into the RSS, Atom, JSON Feed, news-sitemap, and
  sitemap plugins. Unparseable fields log which format failed.
- **Native permalink derivation** (`src/core/urls.rs` + content stager, spec
  A2/B1): pages without `permalink`/`url` get one derived from
  `base_url + output_path` at staging time — feeds can never hard-fail on a
  missing channel link. Active in the real build path via
  `compile_site_with_base_url`.
- **Single page-language resolver** (spec A5): frontmatter `language` →
  `hreflang` → locale path prefix → site default. JSON-LD `inLanguage`,
  `og:locale`, `<html lang>`, and the hreflang self-reference now agree on
  every page, enforced by the new **`lang_consistency` audit gate** (gate
  15).
- **`IoPool` writer pool** ([#569](https://github.com/sebastienrousseau/static-site-generator/issues/569)
  phase 1): bounded-channel writer threads decouple `fs::write` from rayon
  CPU workers; the fused transform pass now **skips unchanged files**
  (a no-op rebuild writes zero files). io_uring backend remains phase 2.
- **`AgentApiPlugin`** (#586 port 3): `/api/agents/{index,posts,topics,person}.json`
  — a stable, deterministic JSON API for AI crawlers and agent toolchains.
- **`OembedPlugin`** (#586 port 4, opt-in): per-page `oembed.json` +
  discovery `<link>`.
- **Per-tag landing pages** (#586 port 5): `/tags/<tag>/index.html` with
  canonical/OG essentials inlined; author-authored tag hubs are never
  clobbered.
- **`[security] sri_algorithm` config** (spec B3): SRI `integrity=`
  attributes now default to **SHA-384** (matching the long-documented
  claim), configurable to sha256/sha512; CSP directive source hashes stay
  SHA-256 for UA compatibility.
- **Per-page CSP → edge headers** (spec B4): inline script/style/JSON-LD
  hashes are computed per page and emitted as per-path entries in
  `_headers` / `vercel-headers.json` — hash-strict CSP without
  `'unsafe-inline'`.
- **Social-meta derivation cascade** (spec B8): `og:*`/`twitter:*` derive
  from base frontmatter (`twitter_title ⇐ seo_title ⇐ title`,
  `og_image ⇐ banner ⇐ image`, …); explicit fields always win, no global
  bleed-through.
- **CI**: `determinism.yml` (macOS↔Linux output byte-diff + double-build
  reproducibility — the gate that would have caught spec A1 on day one),
  `fuzz.yml` + four cargo-fuzz targets
  ([#566](https://github.com/sebastienrousseau/static-site-generator/issues/566)),
  OSSF Scorecard, `cargo-semver-checks` job, cargo-vet exemption ratchet,
  and a multi-arch (amd64+arm64) GHCR image.
- **`tools/bench-vs-{hugo,zola,eleventy}.sh`** — the comparison scripts
  BENCHMARKS.md documented (closes the remainder of
  [#559](https://github.com/sebastienrousseau/static-site-generator/issues/559)).
- **cargo-vet first-party audits**: 13 genuine `safe-to-deploy` audits of
  the same-author dependency stack; exemptions 544 → 533 with a
  ratchet-only-downward CI gate.

### Fixed

- **Audit-gate false positives** (~122 alerts): the markdownlint gate no
  longer lints YAML frontmatter; the broken-links gate no longer parses
  `<a href>` inside `<script>`/`<style>`; the CSP/images/WCAG gates now
  parse minified HTML (unquoted, valueless, reordered attributes). The demo
  site audit is at **zero error-severity findings** (was 27 errors / 183
  total alerts).
- **Latent UTF-8 corruption** in strikethrough expansion: multi-byte
  characters were mangled on any line processed through the old
  byte-by-byte path.
- **Demo-site defects behind real alerts**: SPDX comment before
  `<!DOCTYPE html>`, missing H1s, missing `og:image`, empty-src logos,
  valueless `alt`.
- SARIF stdout purity: the "Site generated successfully." status line moved
  to stderr so `ssg audit --sarif > file` yields strictly valid JSON.
- `benches/all_pub_api` compiles under `--no-default-features`.

### Changed

- **One URL convention everywhere**: canonical, feed `<link>`, sitemap and
  news-sitemap `<loc>` all derive via `urls::derive_page_url`
  (`…/foo/index.html` → `…/foo/`).
- **Claims reconciliation**: "PQC-aware" → "PQC posture guidance" (ML-DSA
  provenance signing is roadmap
  [#579](https://github.com/sebastienrousseau/static-site-generator/issues/579));
  "streaming compilation" → "bounded-memory batch compilation" where it
  describes `core/streaming.rs`; `tests/docs_accuracy.rs` pins the new
  wording.
- Workspace version 0.0.46 → 0.0.47 across all six crates.

### Performance

- Markdown render clone elimination + `Cow` strikethrough fast path:
  **−41–63%** on realistic mostly-plain pages, −6–10% on dense GFM.
- Zero-copy frontmatter parsing
  ([#578](https://github.com/sebastienrousseau/static-site-generator/issues/578)):
  −17–25% on TOML frontmatter, intermediate body clone removed.
- Per-thread scratch reuse in search extraction; i18n locale-matrix cache
  `Mutex` → `RwLock` (read-mostly fast path).

### Upstream (staged in sibling repos, releases pending)

- `staticdatagen 0.0.11`: `allow_unsafe_html: true` explicit (spec A1),
  permalink fallback chain + never-abort feed semantics (spec A2), flexible
  date parsing (spec A4).
- `html-generator 0.0.7`: comrak 0.52, enforced escape-by-default for raw
  HTML, structured-data title from metadata (spec A3).
- `frontmatter-gen 0.0.7`: path-traversal guard scoped to path-typed fields
  (spec A6).
- `mdx-gen 0.0.5`: **security** — raw-HTML pass-through no longer forced;
  safe-by-default sanitization honoring the documented contract.

### New crate: `ssg-a11y`

- Extracted the WCAG 2.2 AA accessibility checker (`src/plugins/accessibility.rs`)
  into a standalone workspace crate,
  [#608](https://github.com/sebastienrousseau/static-site-generator/issues/608):
  report/matrix types, WCAG rule checks (heading hierarchy, ARIA landmarks,
  link purpose, focus appearance, target size, banned elements, page
  language), and the compliance-matrix builder now have zero dependency on
  ssg's `Plugin` trait, `PluginContext`, or `SsgError` — `#![forbid(unsafe_code)]`,
  its own error type, own README/tests. `src/plugins/accessibility.rs`
  becomes a thin wrapper (`AccessibilityPlugin` + file I/O); public paths
  under `ssg::accessibility::*` are unchanged for existing consumers.
  `accessibility-report.json` / `wcag-compliance.json` output is
  byte-shape-identical — this is a refactor, not a behavior change.
  Workspace grows to 7 crates.

### Fixed (determinism, round 2)

- `atom.xml` / `feed.json`: `AtomFeedPlugin`/`JsonFeedPlugin` sorted entries
  by date only; since `read_meta_sidecars` walks the filesystem (genuinely
  OS-order-dependent — ext4 vs APFS) and the stable sort never breaks ties
  on equal dates, synthetic fixtures where every page shares a date leaked
  raw directory-walk order straight into the feed. Fixed with an `id`-based
  tiebreaker at all three feed plugins (RSS/Atom/JSON Feed), plus a
  defensive sort at `read_meta_sidecars` itself so future consumers inherit
  deterministic input by default.
- `sbom.cdx.json`: a **second**, independent `SbomPlugin` implementation
  (`src/plugins/sbom.rs`, distinct from `src/plugins/postprocess/sbom.rs`)
  runs later in the pipeline and overwrites the first's output — it had
  its own unpinned `SystemTime::now()` call the earlier `SOURCE_DATE_EPOCH`
  fix never touched. Fixed and covered by a matching regression test.
- Determinism gate exclusions (`determinism.yml`) refined: `.meta/**/*.json`
  (upstream `staticdatagen` unordered-map serialization, tracked for the
  `=0.0.11` pin) and `.ssg-plugin-cache.json` (ssg's own incremental cache
  — legitimately path-keyed to each build's absolute output location, not
  a bug) are excluded from cross-path comparisons with documented reasons;
  the cache's own key order is now deterministic (`BTreeMap`, not
  `HashMap`) for the common same-directory-rebuild case.
- `core/lang.rs`: `DEFAULT_PAGE_LANG` and `resolve_render_lang` were dead
  code under `--no-default-features` (their sole caller is gated behind
  the `templates` feature) — gated to match, verified against all 45
  `cargo-hack --feature-powerset --depth 2` combinations.
- `examples/audit_example.rs`: un-backticked `lang_consistency` in a doc
  comment (clippy `doc_markdown`).

### Versioning policy

- [ADR-0009](docs/adr/0009-versioning-policy-0.0.x-until-0.0.999.md):
  `ssg` stays on `0.0.x` versioning — incrementing by `0.0.1` per release —
  through `0.0.999` at the earliest, to mature the API surface and
  enterprise adoption before any `0.1.0`/`1.0.0` SemVer commitment.
  `ROADMAP.md` and `docs/architecture/api-stability-audit.md`'s prior
  `0.1.0`/`1.0.0` milestone targets are corrected to match.

## [0.0.46] - 2026-06-28

The "shim retirement" release. All 8 upstream fixes filed during the v0.0.45 cycle landed and shipped; the bulk of the v0.0.45 content-staging shim is gone.

### Upstream PRs that landed in this release

| Repo | Released | PR | Closes (upstream) | Closes (downstream) |
|---|---|---|---|---|
| `staticdatagen` → 0.0.10 | 2026-06-28 12:39 UTC | [#72](https://github.com/sebastienrousseau/staticdatagen/pull/72) | `#67` / `#68` / `#69` / `#70` / `#71` + 4 dependabot bumps | `inject_default_layout_if_missing`, `stage_templates_with_required_stubs`, `ensure_tags_stub`, `stage_content_with_default_layout` (4 stager pub fns retired) |
| `staticweaver` → 0.0.3 (then 0.0.4) | 2026-06-28 09:21 / 17:40 UTC | [#29](https://github.com/sebastienrousseau/staticweaver/pull/29) | `#28` + `askama_escape` drop | closes [ssg#589](https://github.com/sebastienrousseau/static-site-generator/issues/589) (idempotent HTML escape) via the `staticdatagen 0.0.10 → staticweaver 0.0.3` transitive dep |
| `rss-gen` → 0.0.6 | 2026-06-28 10:19 UTC | [#35](https://github.com/sebastienrousseau/rssgen/pull/35) | `#34` (context-prefixed validation errors + relative item-link URLs per RSS 2.0 §5.7) | (no ssg shim — pure upstream improvements) |
| `metadata-gen` → 0.0.5 | 2026-06-28 20:01 UTC | [#21](https://github.com/sebastienrousseau/metadata-gen/pull/21) | `#20` | held — `staticdatagen 0.0.10` still pins `metadata-gen = "0.0.4"`. Tracked: [staticdatagen#100](https://github.com/sebastienrousseau/staticdatagen/issues/100). |

### Changed

- **`Cargo.toml`** — `staticdatagen 0.0.9 → 0.0.10`. Lockfile reflects the transitive bumps: `staticweaver 0.0.2 → 0.0.3`, `rss-gen 0.0.5 → 0.0.6`.
- **`src/core/content_stager.rs`** — reduced from ~1,300 to ~660 LOC. Four shim public functions retired (their bugs closed natively in `staticdatagen 0.0.10`): `stage_content_with_default_layout`, `inject_default_layout_if_missing`, `ensure_tags_stub` (private), `stage_templates_with_required_stubs`. The `DEFAULT_LAYOUT` + `REQUIRED_TEMPLATE_FILES` consts and three helpers (`copy_templates_tree`, `frontmatter_has_layout_key`, the layout-injection branch of `copy_tree`) went with them. 21 obsolete unit tests removed. Module docstring rewritten to document the two residual shims and their upstream tracking issues.
- **`src/core/pipeline.rs::compile_site`** — drops the `stage_templates_with_required_stubs` call; the user's `template_dir` is now passed directly to `staticdatagen::compile`. The pipeline doc-comment now describes the v0.0.46 residual scope rather than the v0.0.45 regression matrix.
- **3 v0.0.45-era input-validation tests updated** (`test_compile_site_error`, `test_internal_compile_with_empty_directories`, `test_args_all_required_arguments`) — empty directories are now a valid "no work to do" build under `staticdatagen 0.0.10` (closes upstream #68 / #69) so the tests were re-pointed at a genuine io error (a file passed where a directory is expected) to keep error-propagation coverage.

### Added

- **`examples/multilingual_full/`** — 32-file content tree (5 locales × `index.md` + 5 posts) demonstrating the nested `content/<lang>/<slug>.md` layout that `staticdatagen 0.0.10` (closes upstream #70) walks recursively. Registered as the `multilingual_full` `[[example]]`. Verified end-to-end: 30/30 per-locale pages land.
- **`tests/regression_user_site.rs::nested_locale_subdirectories_build_per_language`** — new always-on regression covering the recursive walk on a 3 × 2 in-memory tempdir.
- **Two upstream follow-up issues filed** to track the remaining staticdatagen wiring needed before the residual shim disappears: [staticdatagen#99](https://github.com/sebastienrousseau/staticdatagen/issues/99) (`Engine::with_lax_undefined(true)`) and [staticdatagen#100](https://github.com/sebastienrousseau/staticdatagen/issues/100) (bump `metadata-gen` to `0.0.5`).

### Fixed

- **[#589](https://github.com/sebastienrousseau/static-site-generator/issues/589) — HTML-entity double-escape** (`&` → `&amp;amp;`) corrupting body text, `og:title`, `twitter:title`, JSON-LD strings, and `href`s on ~30 % of real-world pages. Closed transitively by bumping `staticdatagen` to 0.0.10 (which pulls in `staticweaver 0.0.3`'s idempotent `escape_html_into` from [staticweaver#29](https://github.com/sebastienrousseau/staticweaver/pull/29)).

### Workspace versions

`ssg`, `ssg-core`, `ssg-rpc`, `ssg-rpc-macro`, `ssg-search`, `ssg-wasm` — all bumped 0.0.45 → 0.0.46.

## [0.0.45] - 2026-06-27

### Added

- **Hygiene + correctness + supply-chain attestation baseline.** PR [#583](https://github.com/sebastienrousseau/static-site-generator/pull/583) closed 15 issues:
  - **#556** profraw hygiene + `repo-hygiene` CI gate; `make coverage` pins `LLVM_PROFILE_FILE` to `target/coverage/`.
  - **#557** Six baseline ADRs in [`docs/adr/`](docs/adr/) + `lint-adr` CI gate enforcing the `adr: ADR-NNNN` citation graph.
  - **#558** No-shellout regression lint (`tools/lint-no-shellout.sh`).
  - **#559 + #494** `BENCHMARKS.md` expanded 83 → 245 lines; `tools/seed-bench-corpus.sh` generates deterministic 10/100/1K/10K corpora.
  - **#560** Miri workflow — nightly schedule + `run-miri`-labelled PR trigger, 180-min timeout.
  - **#561** `cargo-vet` supply-chain attestation (Mozilla / Bytecode Alliance / Google trust sets).
  - **#562** SARIF v2.1.0 emitter for `ssg audit` + GitHub Code Scanning upload step.
  - **#563** Dead `openssl` direct dep removed (7 transitive crates dropped from `Cargo.lock`).
  - **#584** `cargo check --no-default-features` dead-code errors fixed; `cargo-hack feature-powerset` CI step added.
  - **#466** Golden-file framework expanded 1 → 11 goldens (10 scaffold + 1 e2e sitemap).
  - **#495** A11y element-presence gate confirmed always-on.
  - **#21** 100% `rustdoc` coverage verified across all six workspace crates.
  - **#22** README v0.0.45 sync + workspace version bump 0.0.44 → 0.0.45.
  - **#23 / #24 / #25** [`docs/features-coverage.md`](docs/features-coverage.md) + `features_matrix_is_exhaustive` test gate.

- **Content-staging shim** ([`src/core/content_stager.rs`](src/core/content_stager.rs), [ADR-0007](docs/adr/0007-staticdatagen-staging-shim.md)) — works around five `staticdatagen 0.0.9` / `staticweaver 0.0.2` / `metadata-gen 0.0.4` brittleness points so 2,371-file real-world user sites build again. Upstream fixes filed and tracked in [#585](https://github.com/sebastienrousseau/static-site-generator/issues/585).

### Fixed

- **Site-build regression on user sites without `layout:` frontmatter**, missing `main.js`/`sw.js`, no `tags.md`, multi-line YAML scalars, or template references to keys content omits. Detailed root-cause in [ADR-0007](docs/adr/0007-staticdatagen-staging-shim.md). Validated against `sebastienrousseau/sebastienrousseau.github.io` — 102 root pages, 6.40s build, all 102 a11y-passing.

### Changed

- **CI coverage floors raised** from 95.0 → 95.5 / 96.5 / 95.5 (regions / lines / functions). Coverage gate uses `cargo llvm-cov --lib` to keep the heavy `example_outputs.rs` integration suite in its own job.
- **Miri trigger model**: nightly schedule + `run-miri` label-gated PR runs (decoupled from every push).
- **100-page build budget** in `tests/perf_budgets.rs` raised 500ms → 800ms to absorb the `content_stager` shim. Reverts in v0.0.46.

### Security

- **cargo-vet attestation** (#561) layered over `cargo-deny`.
- **SARIF feed into GitHub Code Scanning** (#562) surfaces audit findings in the Security tab.

### Performance

- **Content-stager pre-pass is Rayon-parallelised** (`copy_tree` and `inject_template_defaults_recursive`).

### Internal

- ~90 new unit tests, ~6 integration tests (`tests/regression_user_site.rs`). Total lib suite: **2,530 / 2,530 passing**.
- Workspace versions bumped 0.0.44 → 0.0.45 across `ssg`, `ssg-core`, `ssg-rpc`, `ssg-rpc-macro`, `ssg-search`, `ssg-wasm`.
- ADR-0007 documents the staging shim explicitly so future maintainers see the upstream debt at a glance.

## [0.0.40] - 2026-06-06

### ⚠ BREAKING CHANGES

- **Public API Error Type Swap**: `PathsBuilder::build` and `Paths::validate` now return `Result<T, SsgError>` instead of `anyhow::Result<T>`. Downstream users matching or handling errors from these endpoints must update their matches to `SsgError`.

### Added

- Native asset minification (JS/CSS) inside the asset pipeline.
- Localized switchers for language alternates using matched slugs.
- First-class Topic taxonomy type with hub/pillar page generation.
- Overlapping-tag similarity indexing for Related Posts.
- Word count and estimated reading time calculations in frontmatter metadata.
- Configurable CDN URL prefixing for markdown images.
- **Structured Error Handling**: Introduced `ssg_core::Error` for the core compilation module and a comprehensive library-wide `SsgError` wrapping all I/O, validation, template rendering, and path safety violations.
- **Contextual I/O Extensions**: Added `PathErrorExt` helper trait to cleanly propagate system directory and file paths alongside underlying I/O errors.

### Changed

- **Encapsulation Pass**: Module declarations in `src/lib.rs` for implementation groups (`core`, `plugins`, `server`) changed from `pub mod` to `pub(crate) mod` (renamed internally as `*_group` to avoid clashing with facade re-exports). Only clean, public facade APIs are exported.
- **Layout Restructuring**: Restructured parent crate codebase, organizing source files into `src/core/`, `src/plugins/`, and `src/server/` directories.

## [0.0.39] - 2026-05-10

### ⚠ BREAKING CHANGES

- **SRI hashes now use real SHA-256.** The previous in-house FNV-1a
  placeholder in `src/assets.rs::sha256_hex` was producing
  `integrity="sha256-..."` attributes that no browser would actually
  validate against. Real SHA-256 + canonical base64 means the hashes
  emitted by `FingerprintPlugin` are now genuine SRI per the W3C
  spec. **User impact:** any site that pre-built and checked-in SRI
  hashes (or fingerprinted filenames) needs a one-time rebuild;
  the short fingerprint suffix and the long SRI both change shape
  to canonical SHA-256-derived values.
- **`RunOptions` demoted to crate-internal.** Was nominally `pub` in
  `src/pipeline.rs`; now `pub(crate)`. The module is `pub(crate)`
  too, so the effective surface is unchanged for external consumers
  that imported via `ssg::pipeline::RunOptions`. No re-export was
  in place, so this should be a no-op for everyone but the curious.
- **Six public enums marked `#[non_exhaustive]`:** `DeployTarget`,
  `FieldType`, `UrlPrefixStrategy`, `ReadabilityFormula`,
  `ChangeKind`, `ProcessError`. Downstream code that pattern-matches
  on these without a wildcard arm needs `_ => …` added. This was
  staged in preparation for `1.0.0-rc.1`.

### Added

- **Build-time CycloneDX SBOM** (`src/sbom.rs`): every build emits a
  `sbom.cdx.json` at the site root + injects a per-page
  `<link rel="sbom" type="application/vnd.cyclonedx+json"
  href="/sbom.cdx.json">` discoverable via the IANA-registered link
  relation. Closes #457.
- **Typed content collection API** (`src/collections.rs`):
  `get_collection::<T>(dir)` / `get_entry::<T>(dir, slug)` for
  serde-typed Markdown loading, mirroring Astro's `getCollection`
  ergonomics with compile-time type safety. Closes #456.
- **WCAG 2.2 build-time checks** (`src/accessibility.rs`):
  `check_target_size` (2.5.8), `check_focus_appearance` (2.4.13)
  plus a `wcag-compliance.json` matrix artifact. README claim
  promoted from "WCAG 2.1 AA" to "WCAG 2.2 AA". Closes #421, #463.
- **JSON-LD schema.org validation** (`src/seo/jsonld.rs`):
  `validate_jsonld` checks required fields per `@type`; new CI
  step walks every example output. Closes #467.
- **CSS `url()` rewriting in `FingerprintPlugin`** (`src/assets.rs`):
  three-pass fingerprint pipeline so CSS-embedded image and font
  references stay valid after content-hashed renames. Closes #468.
- **Content-addressable assets widened to 14 extensions**: CSS, JS,
  MJS plus 7 image formats + 4 font formats. Per-platform
  `Cache-Control: immutable` rules emitted for Netlify, Vercel,
  Cloudflare Pages.
- **Reproducible-build verification job** in `scheduled.yml`:
  double-build with `--locked --offline` + SHA-256 hash diff.
  Closes #424.
- **SECURITY.md** (canonical security policy): disclosure SLA,
  threat model, security defaults, reproducible-build recipe,
  build-provenance verification.
- **WCAG 2.2 + EAA compliance guide**
  (`docs/guide/wcag-compliance.md`): full criterion mapping, EAA
  enforcement context (28 June 2025), member-state implementations,
  before/after migration metrics. Closes #470.
- **API stabilisation audit**
  (`docs/architecture/api-stability-audit.md`): 4-tier inventory of
  ~79 public types + Plugin trait. Closes #427.
- **Perf baseline doc** (`docs/perf/baseline-100p.md`): 18.7 ms
  100-page measurement + 6-subsystem hotspot inventory + profiling
  recipes. Closes #471.
- **Regression contract** (`docs/architecture/regression-contract.md`):
  canonical inventory of every CI gate and its user-facing promise.
- **`tests/chaos.rs`** — 9 chaos-engineering tests (corrupt
  frontmatter, symlink loops, concurrent builds). Closes #423.
- **`tests/element_presence.rs`** — universal HTML invariants gate
  (lang, title, main, charset) on every example page.
- **`tests/perf_budgets.rs`** — hard wall-clock budgets: 10-page <
  100 ms, 100-page < 500 ms, 500-page < 2 s.
- **`tests/jsonld_validation.rs`** — schema.org required-field
  validation across every example output, wired into CI.
- **`tests/golden_files.rs`** — golden-file regression framework
  with normalisation (timestamps, fingerprints, SRI) and
  `UPDATE_GOLDEN=1` workflow. Closes #466 (framework phase).
- **`tests/docs_accuracy.rs`** — verifies README claims match
  source-of-truth files (test count, WCAG version, coverage floors,
  MSRV, version sync).
- **`tests/doc_links.rs`** — every internal Markdown link in
  README/CHANGELOG/SECURITY/docs/* resolves to an existing file.
- **OpenTelemetry feature-gate scaffolding** (`src/otel.rs`):
  `otel` Cargo feature, `--trace` CLI flag, one demo span around
  `execute_build_pipeline`. Closes #422 phase A.
- **Multi-OS `example_outputs` portability job** in `scheduled.yml`:
  weekly macOS + Windows runs. Closes #473.
- **Per-criterion SEO comparison matrices** in
  `docs/compare/ssg-vs-{hugo,zola,astro}.md` covering 17–18 SEO
  criteria each. Closes #461.

### Changed

- **README test-count claim refreshed** from 1,640 → 1,685+ lib
  tests, plus mention of new `collections` + `sbom` + `otel`
  modules.
- **Cargo.toml keywords**: `["cli","generator","ssg","static-site",
  "wasm"]` → `["rust","markdown","jamstack","ssg","wasm"]` for
  better crates.io discoverability. Closes #428.
- **`FingerprintPlugin` rename suffix** is still the first 8 hex
  chars but now derived from real SHA-256 instead of FNV-1a.

### Fixed

- **Security:** `sha256_hex` was FNV-1a, not SHA-256.
  `integrity="sha256-..."` attributes were silently invalid. Now
  real SHA-256 via the `sha2` crate + canonical base64 via the
  `base64` crate. Verified against NIST test vectors.
- **CSS `url()` references broke** after image/font fingerprinting
  because the CSS file content wasn't patched before the CSS file
  was itself hashed. Three-pass pipeline fixes the ordering.
- **CSS parser false positives in WCAG checks** —
  `/* width: 10px */` no longer triggers 2.5.8; `@media print {
  button { width: 10px } }` no longer fires unconditionally;
  multiple `<style>` blocks now all scanned.
- **JSON-LD validator over-strictness** — `WebPage` requires only
  `name` (per Google rich-results docs), not `name + url +
  inLanguage`. The latter two are Recommended only.
- **`tests/chaos.rs::read_only_output_directory_returns_clean_error`**
  now uses a Drop guard for permission restoration so panics don't
  leave stale 0o555 tempdirs on CI disk.
- **Reproducible-build job** runs `cargo fetch --locked` then both
  build invocations as `--locked --offline` to eliminate transient
  registry state.

### Dependencies

- Cargo (`Cargo.toml` + `Cargo.lock`):
  - `rustls-webpki` 0.103.12 → 0.103.13 (resolves Dependabot #487)
  - `clap` 4.6.0 → 4.6.1 (#490)
  - `openssl` 0.10.77 → **0.10.79** (#490 + #492)
  - `uuid` 1.22.0 → 1.23.1 (#490)
  - **NEW** `sha2` = `"0.10"` (required for real SHA-256 SRI)
  - **NEW** `base64` = `"0.22"` (canonical base64 SRI encoding)
- GitHub Actions:
  - `actions/upload-artifact` v4 → **v7.0.1** (#489), 4 workflows
  - `crazy-max/ghaction-import-gpg` v6 → **v7.0.0** (#488),
    release.yml gpg-sign job
- npm (`tests/visual/`):
  - `@axe-core/playwright` 4.10.0 → **4.11.3** (#491)

### Security

- **Resolves 8 Dependabot security advisories on the default
  branch**: 6× openssl (alerts #29–#33, #35, #36; all `< 0.10.79`),
  1× rustls-webpki (#34; `< 0.103.13`). Severity mix: 6 high + 1
  moderate + 1 low.
- **Real SHA-256 for SRI** (above).
- **Reproducible-build verification** in CI.
- **`SECURITY.md`** canonical security policy.

## [0.0.38] - 2026-04-20

### Added

- **Agentic LLM pipeline**: `--ai-fix` CLI flag triggers audit, diagnose, fix, verify, and report cycle with configurable max refinement attempts and JSON output
- **Multilingual readability**: Kandel-Moles (FR), Wiener Sachtextformel (DE), Gulpease (IT), LIX (SV/NO/DA), Fernandez Huerta (ES) with BCP 47 language detection from frontmatter
- **OG image generation**: auto-generated SVG social cards from page title and site name, injected via `og:image` meta tag, zero new dependencies
- **Scalability benchmarks**: Criterion benchmarks at 100, 1K, and 10K page tiers with CI job on release tags
- **axe-core CI**: `@axe-core/playwright` integration for WCAG 2.1 AA audit with JSON report artifacts
- **CSP whitepaper**: `docs/whitepaper/csp-without-compromise.md` documenting build-time inline extraction and SRI hashing
- **237 new unit tests**: coverage raised from 94.24% to 95.06% regions (1,640 total)

### Changed

- CI coverage regions floor raised from 94% to 95%
- Version bumped from 0.0.37 to 0.0.38
- README rewritten with updated metrics, feature matrix, and architecture diagram

### Fixed

- `package-lock.json` synced with `@axe-core/playwright` dependency
- axe-core a11y audit restricted to desktop project (Chromium only) to avoid missing WebKit binary in CI

### Dependencies

- `actions/checkout` v4 to v6.0.2
- `actions/download-artifact` v4 to v8.0.1
- `actions/attest-build-provenance` v2 to v4.1.0
- `actions/upload-pages-artifact` v3 to v5.0.0
- `actions/deploy-pages` v4 to v5.0.0
- `actions/cache` v4 to v5.0.5
- `actions/setup-node` v4 to v6.4.0
- `docker/setup-buildx-action` v3 to v4.0.0
- `docker/build-push-action` v6 to v7.1.0
- `docker/login-action` v3 to v4.1.0

## [0.0.37] - 2026-04-19

### Added

- **WebAssembly**: `ssg-core` and `ssg-wasm` crates for browser/edge compilation
- **Interactive islands**: `<ssg-island>` Web Components with lazy hydration
- **Streaming compilation**: batch-based compiler for 100K+ page sites
- **Local LLM pipeline**: auto-generate alt text, meta descriptions, readability auditing
- **Dependency graph**: `DepGraph` for incremental rebuild tracking
- **Browser error overlay**: build errors rendered in-browser via WebSocket
- **CSS hot reload**: stylesheet changes without full page reload
- **Property-based testing**: proptest for frontmatter, markdown, shortcode fuzzing
- **WASM integration tests**: 12 wasm-bindgen-test cases in headless Chrome
- **llms.txt spec compliance**: section index, language field, disallow patterns
- **Performance gates**: 8 timed CI assertions (compilation, search, cache, streaming)
- **Enterprise regression suite**: 27 tests for cache resilience, licence, i18n, pipeline

### Changed

- Template engine: Tera → MiniJinja (10× smaller binary)
- Coverage floors raised to 95% (regions, lines, functions)
- All examples emit build timing and use unique ports (3001–3007)
- Plugin table descriptions shortened for readability audit compliance
- 100% API coverage: all 36 modules demonstrated in examples

### Fixed

- SPDX headers on all 97 source files (100% compliance)
- Duplicate "All rights reserved" in 5 bench/example files
- Duplicate server banners in 6 examples
- `run_fused_transforms` missing from 3 examples
- Readability audit threshold raised to grade 17 for technical docs

### Security

- CSP/SRI hardening: extract inline styles/scripts to external files
- GitHub Actions pinned to commit SHAs
- Dependabot configuration added
- `unsafe-inline` eliminated from Content-Security-Policy

## [0.0.36] - 2026-04-13

### Added

- **Post-processing pipeline** — new `postprocess` module with 5 plugins that
  repair `staticdatagen` output: `SitemapFixPlugin` (duplicate XML declarations,
  double-slash URLs, per-page lastmod), `NewsSitemapFixPlugin` (placeholder
  replacement, `<news:keywords>`), `RssAggregatePlugin` (feed aggregation with
  enclosures, categories, language, lastBuildDate, copyright),
  `ManifestFixPlugin` (word-boundary-safe truncation), `HtmlFixPlugin` (JSON-LD
  date conversion, HTTPS context, broken img repair).
- **Content schema validation** — new `content` module with `ContentSchema`,
  `FieldDef`, TOML schema loader, compile-time frontmatter validation, and
  `--validate` CLI flag for schema-only checks. 62 tests.
- **Responsive image pipeline** — `ImageOptimizationPlugin` now emits
  `<picture>` elements with AVIF/WebP `<source>` tags, responsive `srcset` at
  320/640/1024/1440, `loading="lazy" decoding="async"` by default,
  `fetchpriority="high"` → `loading="eager"`, width/height from source metadata.
- **i18n routing** — new `i18n` module with `I18nPlugin`, automatic hreflang
  injection for multi-locale pages, `x-default` support, per-locale sitemaps
  with `xhtml:link` alternates, `generate_lang_switcher_html()` helper.
- **Parallel plugin pipeline** — `MinifyPlugin` and `SearchIndex::build`
  converted to `par_iter()`. New `--jobs N` CLI flag for Rayon thread count.
- **Benchmark suite** — Criterion benchmarks for 10–10K synthetic pages,
  `benchmarks/README.md` with cross-SSG comparison instructions, `BENCHMARKS.md`
  template.
- **Accessibility CI** — `.github/workflows/a11y.yml` with pa11y WCAG 2.1 AA
  scanning, `make a11y` target.
- **SBOM + CI hardening** — `.github/workflows/sbom.yml` with CycloneDX
  generation and Sigstore build provenance attestation.
- **Multi-platform release workflow** — `.github/workflows/release.yml` builds 5
  targets on `v*` tags: Linux glibc, Linux musl (static), macOS ARM64, macOS
  Intel, Windows. SHA256 checksums, GitHub Release, crates.io publish.
- **Install script** — `scripts/install.sh` auto-detects OS/arch, downloads
  correct binary, verifies checksum, installs to `~/.local/bin`.
- **Homebrew formula** — `packaging/homebrew/ssg.rb` for `brew install`.
- **SPDX license headers** — added to all 60+ source files.
- **Deploy security headers** — `Content-Security-Policy` and
  `Strict-Transport-Security` (HSTS) added to Netlify/Vercel/Cloudflare configs.
- **Enhanced SEO plugin** — full OG suite (og:url, og:image, og:image:width/
  height, og:locale), full Twitter Card suite (summary_large_image for
  articles), JSON-LD Article/WebPage with datePublished, dateModified, author
  as Person entity, image as ImageObject, inLanguage.
- **Canonical URL replacement** — `CanonicalPlugin` now replaces template
  placeholders with correct `base_url + path` instead of skipping existing tags.

### Changed

- **Renamed** all references from "Shokunin" to "Static Site Generator".
- **Dependencies reduced** from 25 → 21 direct deps: `once_cell` → `OnceLock`,
  `dtt` → `chrono`, `colored` → ANSI codes, `uuid` moved to dev-deps.
- **Tokio features trimmed** from `["full"]` to `["fs", "rt-multi-thread",
  "macros", "time"]` — removes 8 unused subsystems.
- **MSRV** synced between `build.rs` (was 1.74) and `Cargo.toml` (1.88).
- **Dev server** only starts when `--serve` is explicitly requested (was
  blocking unconditionally after every build, breaking CI).
- **Accessibility checker** recognises `alt=""` with `role="presentation"` and
  bare `alt` attribute (minified) as valid decorative images.
- **Template contrast** — WCAG AAA colours: `--vp-t3` → `#545458`/`#a1a1aa`,
  `--vp-br` → `#1a3a8a`, links underlined for colour-blind distinguishability.
- **Musl static binary** — added to CI portability matrix (weekly + release).
- **`deny.toml`** — removed stale `CC0-1.0` and `Unicode-DFS-2016` entries.

### Fixed

- **Sitemap** — duplicate XML declarations, double-slash URLs, stale lastmod.
- **News sitemap** — "Unnamed Publication" / "Untitled Article" placeholders
  replaced with real frontmatter data.
- **RSS feed** — root feed now aggregates all article items (was single
  self-referencing entry).
- **OG/Twitter tags** — empty on non-index pages due to comment-marker
  detection instead of actual `<meta>` tag checks.
- **JSON-LD dates** — RFC 2822 → ISO 8601 conversion.
- **JSON-LD @context** — `http://schema.org/` → `https://schema.org`.
- **Manifest.json** — description truncated mid-word at 120 chars.
- **Markdown .class= syntax** — `<p src=` injected into `<img>` tags.
- **Lighthouse scores** — A11y 91→100, SEO 85→100 on generated output.
- **CI** — a11y workflow cancellation, Chrome sandbox flags, mold linker
  config incompatibility with CI runners.

### Added (continued — 2026-04-16)

- **8 polished examples with distinct brand identities** — every example now
  ships as a real-feeling clone-and-edit template:
  - `basic` — *Aria Studio* (independent design studio, single-page layout)
  - `blog` — *Threshold* (accessibility journal, 3 substantive posts on EAA /
    WCAG / typography, working tags + posts aggregation)
  - `quickstart` — *Heron Coffee* (small London roastery + 3 journal posts
    demonstrating the full 16-plugin pipeline against realistic content)
  - `docs` — *Polaris* (generic developer-tool docs template — Welcome /
    Getting Started / Configuration / API reference / Release notes / Support)
  - `landing` — *Meridian Systems* (compliance-grade software for regulated
    industries; rich body copy, real client list, zero-JS verification)
  - `portfolio` — *Maya Okafor* (independent UX researcher, 3 detailed case
    studies: Field Notes Collective, Linden Editions, Polaris Maps)
  - `multilingual` — 6 priority locales (EN/FR/ES/DE/JA/AR) rewritten with a
    real i18n product narrative ("Write once, ship in 28 languages")
  - `plugins` — annotated lifecycle walkthrough, own dirs, root templates
- **Comprehensive regression test suite** — `+140 tests` across 3 new files:
  - `tests/example_outputs.rs` (19 tests) — runs every example end-to-end +
    11 negative validator tests proving the validators catch what they claim
  - `tests/plugin_contracts.rs` (8 tests) — lifecycle ordering, plugin
    idempotency (HtmlFix + ManifestFix), HtmlFix→Minify ordering, SVG data-URL
    preservation
  - `tests/schema_validation.rs` (8 tests) — `validate_with_schema` contract:
    valid pages pass, missing fields fail, unknown enum values fail, missing
    schema file tolerated, multiple errors aggregated, legacy `validate_only`
    path still works
- **Coverage gate** — `.github/workflows/ci.yml` enforces region ≥95.0%, line
  ≥97.0%, function ≥95.0%. Lib coverage measured at 95.22% / 97.46% / 95.79%.
- **`validate_with_schema(content_dir, schema_path)` API** — schema can now
  live outside `content_dir`, avoiding `staticdatagen::compile`'s read-every-
  file behaviour that previously blocked the docs example schema validation.
- **Browser-compat fixes in `HtmlFixPlugin`** — removes empty `<link
  rel="preload" href>` tags; injects modern `mobile-web-app-capable` meta
  alongside the deprecated apple variant.
- **`ManifestFixPlugin` empty-icon filtering** — drops icon entries whose `src`
  is empty (Chrome would otherwise log a manifest icon download error).
- **Mobile-menu desktop fix** — added `.mobile-menu{display:none}` to base CSS
  in all 6 shared templates; previously the rule lived only inside
  `@media(max-width:768px)` so the menu rendered as a duplicate nav on desktop.
- **Mobile nav alignment fix** — added `.nav-controls{margin-left:auto}` to the
  `@media(max-width:768px)` block so theme switch + hamburger sit flush right
  when `.nav-search` is hidden.

### Changed (continued — 2026-04-16)

- **Folder hierarchy consolidated**:
  - `Formula/` + `pkg/{arch,deb,scoop,winget,PUBLISHING.md}` →
    `packaging/{homebrew,arch,deb,scoop,winget,PUBLISHING.md}`
  - `template/tera` → `templates/tera` (singular `template/` removed)
  - `benchmarks/README.md` → `benches/README.md` (benchmarks/ removed)
  - Empty root `content/`, `templates/`, `public/`, `build/` removed
- **CI workflows consolidated 7 → 3**:
  - `ci.yml` (PR gate; lint → test ×3 OS · examples · coverage · audit
    in parallel; <5 min wall time target)
  - `scheduled.yml` (weekly + tag; portability matrix, musl static, pa11y,
    SBOM)
  - `release.yml` (tag; build × 5 platforms + GHCR + GPG + AUR + crates.io)
- **Release pipeline expanded** — adds `.rpm` (cargo-generate-rpm), macOS
  `.pkg` (pkgbuild), Windows `.msi` (cargo-wix), multi-arch GHCR container
  (`ghcr.io/sebastienrousseau/static-site-generator:vX.Y.Z` + `:latest`),
  AUR push (gated on `AUR_SSH_KEY` secret), GPG detached signatures (gated
  on `GPG_PRIVATE_KEY` secret).
- **Cache files relocated** — `.ssg-cache.json` + `.ssg-plugins-cache.json`
  moved from repo root → `target/.ssg-cache/{ssg,plugins}.json`.
- **Clippy re-enabled** — `cargo clippy --lib -- -D warnings` is now CI-gated;
  tests/examples allow `unwrap_used` + `expect_used` via documented
  workspace-wide `[lints.clippy]` allowance list. Lib has 0 warnings.
- **`Dockerfile` added** — two-stage build (cargo + debian-slim runtime) for
  the GHCR multi-arch image.
- **`Cargo.toml` packaging metadata** — `[package.metadata.generate-rpm]` for
  RPM asset list, `[package.metadata.wix]` for MSI installer config.

### Fixed (continued — 2026-04-16)

- **A11y false positive** — `check_img_alt` previously truncated `<img>` tags
  at the first `>` character inside an SVG `data:` URL in `src=`, causing
  spurious `<img> missing alt text: (no src)` reports. New quote-aware
  `find_tag_end()` respects attribute quoting.
- **Schema validation silently passing** — docs example reported "all pages
  valid" without actually validating because schema was outside `content_dir`
  (where the legacy `validate_only` looked). New API + relocated schema fix it.
- **Nav clutter on single-page templates** — `basic` example trims Posts/Tags
  nav items + footer Resources column + hero CTAs via `:has()` CSS injection.
- **Stray repo artifacts removed** — `*.log`, `fixes.txt`, `.DS_Store`,
  `public.build-tmp/` purged from working tree (already gitignored).

## [0.0.35] - 2026-04-11

### Added

- **Localized search widget** — `SearchLabels` struct with 28 bundled locale
  translations; `LocalizedSearchPlugin` injects per-locale search modal
  strings (button, placeholder, footer hints, no-results message).
- **GFM Markdown extensions** — new `MarkdownExtPlugin` adds tables,
  ~~strikethrough~~, and task-list checkboxes on top of staticdatagen's
  renderer.
- **WCAG AAA green palette** — brand colours switched from blue to green
  (matching the Kaishi logo) with solid-hex text tokens: 7.05:1–16.5:1
  contrast ratios in both light and dark modes.
- **28-locale multilingual example** — full content + template trees for
  en, fr, ar, bn, cs, de, es, ha, he, hi, id, it, ja, ko, nl, pl, pt,
  ro, ru, sv, th, tl, tr, uk, vi, yo, zh, zh-tw.
- **`cmd::resolve_host()` / `resolve_port()`** — `$SSG_HOST` / `$SSG_PORT`
  env-var overrides for WSL2, Codespaces, and dev-container users.
- **`make init`** — one-command bootstrap (detects platform, installs
  rustfmt + clippy + cargo-deny, wires up git hooks, runs first build).
- **`make hooks`** — installs `.githooks/pre-commit` signed-commit guard.
- **`make clean`** — removes build artefacts and stray log files.
- **`.devcontainer/devcontainer.json`** — one-click VS Code / Codespaces
  environment.
- **`.githooks/pre-commit`** — cross-platform (bash) hook that refuses
  unsigned commits.
- **`.github/workflows/portability.yml`** — cost-optimised 3-OS CI matrix
  (fast Linux gate per push; full matrix weekly + on release tags).
- **`<h1>` on all pages** — content templates now emit
  `<h1 class="page-title">{{title}}</h1>`.
- **`<meta name="mobile-web-app-capable">`** added alongside the deprecated
  apple-prefixed variant.
- **`prefers-reduced-motion`** global CSS override.
- **44 px tap targets** for `.lang-btn` and `.menu-toggle`; `.theme-switch`
  uses a transparent `::after` hit-area extension.
- **`docs/README.md`** — explains the gitignored `docs/` build-target
  directory.
- **Criterion benchmark suite** — `benches/bench_site_generation.rs`
  measures end-to-end compile throughput at 10, 50, and 100 pages.
  `make bench` target added to Makefile.
- **`CHANGELOG.md`** — Keep a Changelog format with full release notes.
- **README Table of Contents** — 11-item jump index at the top.
- **Code of Conduct** linked from README.
- **`make doc`** — generates API documentation with `-D warnings` and
  opens in browser.
- **Mermaid plugin lifecycle diagram** in CONTRIBUTING.md.

### Changed

- **Rayon-parallelised plugin pipeline** — `SearchPlugin`,
  `SeoPlugin`, `CanonicalPlugin`, and `JsonLdPlugin` now use
  `par_iter().try_for_each()` instead of sequential `for` loops for
  HTML file injection. `AtomicUsize` replaces mutable counters.
- **`warp` dependency removed** — `handle_server()` now uses
  `http_handle::Server` via `tokio::task::spawn_blocking`. Cargo.lock
  shrank by 292 lines. Direct deps: 25 → 24.

- **CI pipelines pinned to SHA** — all shared workflow refs and GitHub
  Actions pinned to immutable commit SHAs instead of mutable `@main` /
  `@v4` / `@stable` tags. Eliminates supply-chain risk.
- **`.editorconfig`** expanded with `[*.{json,toml}]` and `[*.html]`
  rules at indent 2.
- **MSRV** bumped from 1.74.0 to **1.88.0** (deps had silently escalated).
- **README** rewritten: test count (342→741), CLI reference (10→14 flags),
  cross-platform prerequisites table, library example uses `ssg::run()`,
  CI claim corrected (stable only, not nightly), module list expanded to
  all 30 src modules.
- **CONTRIBUTING.md** architecture tree synced to all 30 modules; signed-tag
  enforcement; per-platform setup instructions.
- **`Cargo.toml`** `documentation` URL → `https://docs.rs/ssg` (was dead
  `static-site-generator.one`); `homepage` → GitHub repository URL.
- **`ssg --help`** no longer leaks `[INFO]` log lines (logger init moved
  below `Cli::build().get_matches()`).
- **Portability CI** split into fast gate (1 job/push) + full matrix
  (weekly/tags) — ~6× cost reduction.
- **`src/process.rs`** gained `//!` module-level documentation.
- **`src/lib.rs`** `ServeTransport` doc fixed (broken `[NoopTransport]`
  intra-doc link).
- Hardcoded `/tmp/` paths in tests replaced with `std::env::temp_dir()`.

### Fixed

- **RTL dropdown positioning** — `right:0` → `inset-inline-end:0` so the
  language menu anchors correctly on Hebrew / Arabic pages.
- **English root link** (`/`) was being rewritten to `/<locale>/` by the
  inline JS — added `h !== '/'` guard.
- **Cross-locale navigation** — language switcher links now preserve the
  current sub-path (e.g. `/en/tags/` → `/fr/tags/`).
- **Banner URLs** corrected: `stock/images/banners/` → `stocks/images/`.
- **Logo URLs** migrated: `kaishi/images/logos/` → `kaishi/v1/logos/`.
- **Theme switch button** visual restored after tap-target rule blew up
  its 40×22 pill to 44×44 square.
- **Search widget dark mode** — greys were globally replaced with light-mode
  values, making text invisible; now context-aware (light: `#595960`,
  dark: `#cccccf`).
- **PR template** — added signed-commit checklist item.
- **Search locale isolation** — widget now fetches
  `/<lang>/search-index.json` per locale instead of always loading the
  English root index. Result URLs are prefixed with the locale path.
- **Search hero content indexed** — `extract_text()` no longer strips
  `<header>` blocks, so hero taglines and subtitles are searchable.
- **Search JS scoping crash** — `lm` and `lp` locale variables hoisted
  from `load()` to the outer IIFE scope; eliminates `ReferenceError`
  that silently crashed the search function on every keystroke.
- **`cargo deny check licenses`** — added Zlib to allow list (used by
  `foldhash`); removed stale RUSTSEC-2025-0068 ignore.
- **RUSTSEC-2026-0097** (rand 0.8.5 unsound) acknowledged in both
  `.cargo/audit.toml` and `deny.toml` — transitive via `phf_generator`,
  SSG never calls `rand::rng()` directly.
- **Unused import** in `quickstart_example.rs` removed.

### Removed

- **Inline JS nav sort** — was comparing translated `textContent` against
  an English `order` array, scrambling the menu. Source-HTML order now
  persists directly.
- **Language selector page** at `/` — root now serves English content
  directly; language switcher is embedded in the nav bar.

## [0.0.34] - 2025-04-04

See [release notes](https://github.com/sebastienrousseau/static-site-generator/releases/tag/v0.0.34).

## [0.0.33] - 2025-02-04

See [release notes](https://github.com/sebastienrousseau/static-site-generator/releases/tag/v0.0.33).

[0.0.37]: https://github.com/sebastienrousseau/static-site-generator/compare/v0.0.36...v0.0.37
[0.0.36]: https://github.com/sebastienrousseau/static-site-generator/compare/v0.0.35...v0.0.36
[0.0.35]: https://github.com/sebastienrousseau/static-site-generator/compare/v0.0.34...v0.0.35
[0.0.34]: https://github.com/sebastienrousseau/static-site-generator/compare/v0.0.33...v0.0.34
[0.0.33]: https://github.com/sebastienrousseau/static-site-generator/releases/tag/v0.0.33
