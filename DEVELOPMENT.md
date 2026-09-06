<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Developing SSG

The single entry point for working on the Static Site Generator: toolchain
setup, how to run **every CI gate locally**, where the tests live, and how
releases are cut.

If you read one section, make it [Reproducing CI
locally](#reproducing-ci-locally). Most red pipelines on this project have
not been real regressions — they have been a local command that differed
from the one CI runs, most often `cargo clippy --lib` where CI runs
`--lib --tests --examples --all-features`.

## Contents

- [Toolchain](#toolchain)
- [First build](#first-build)
- [Reproducing CI locally](#reproducing-ci-locally)
- [Test layout](#test-layout)
- [Packaging and the install contract](#packaging-and-the-install-contract)
- [Supply chain](#supply-chain)
- [Release model](#release-model)
- [Common failure modes](#common-failure-modes)

Architecture — how the pipeline actually fits together — is in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Toolchain

| Requirement | Value | Enforced by |
|---|---|---|
| Rust channel | `stable` | `rust-toolchain.toml` |
| MSRV | **1.88.0** | `rust-version` in `Cargo.toml`; `cargo-semver-checks` in CI |
| Components | `rustfmt`, `clippy` | `rust-toolchain.toml` |

The MSRV floor is set by transitive dependencies (`time-macros`,
`staticdatagen`, `oxc_*`), not by this crate's own source. Raising it is a
dependency decision; record the reason in an ADR under `docs/adr/` when
it moves.

Some gates need tools beyond the Rust toolchain:

```sh
cargo install --locked cargo-deny cargo-vet cargo-llvm-cov \
  cargo-semver-checks cargo-hack
```

Plus, for the docs and packaging gates: `ripgrep` (ADR and shellout
lints), and `mandoc`, `zsh` and `fish` (install smoke test). On macOS all
three ship with the system or are one `brew install` away; the CI job
installs them explicitly rather than assuming.

## First build

```sh
make init     # toolchain components, cargo-deny, git hooks, first build
make test     # the suite
```

`make init` is idempotent — re-running it on an existing clone is a no-op
for anything already in place.

Note that `GNUmakefile` and `Makefile` both exist and do different jobs.
GNU make reads `GNUmakefile` and ignores `Makefile`, so `GNUmakefile`
forwards anything it does not define. Packaging targets live in
`GNUmakefile`; developer targets live in `Makefile`. `make help` lists
both.

## Reproducing CI locally

Every job in `.github/workflows/ci.yml` and the command that reproduces
it. These are the exact strings CI runs; `tests/development_docs.rs`
asserts that, so this table cannot quietly drift from the workflow.

| CI job | Run locally |
|---|---|
| repo hygiene | `./scripts/repo-hygiene.sh` |
| fmt | `cargo fmt --all -- --check` |
| clippy (lib — strict) | `cargo clippy --lib --all-features -- -D warnings` |
| clippy (tests + examples) | `cargo clippy --lib --tests --examples --all-features -- -D warnings -A clippy::unwrap_used -A clippy::expect_used` |
| no-shellout lint | `./tools/lint-no-shellout.sh` |
| ADR citation graph | `./tools/lint-adr.sh` |
| feature powerset | `cargo hack check --feature-powerset --depth 2 --no-dev-deps` |
| unit and integration tests (3 OS on stable; MSRV `1.88` and beta on Linux) | `cargo test --tests --features test-fault-injection` |
| example outputs | `cargo build --examples --quiet` then `cargo test --test example_outputs -- --test-threads=1` |
| coverage gate | `make coverage` |
| docs lint (text) | `typos` (v1.50.1 — CI pins the matching action), `npx markdownlint-cli2`, `./scripts/check-docs-tracked.sh`, `./scripts/check-typos-allowlist.sh`, `reuse lint` |
| docs lint | `cargo test --test doc_links`, `cargo test --test readme_sync`, `cargo test --test docs_accuracy`, `cargo test --test development_docs`, `cargo test --test man_page`, `cargo test --test completions` |
| user manual | `./scripts/build-manual.sh` |
| rustdoc | `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p ssg` |
| cargo-deny | `cargo deny check` |
| install contract | `./scripts/install-smoke.sh` |
| fuzz regression corpus | `cargo +nightly fuzz build <target>` then `./fuzz/target/*/release/<target> fuzz/corpus/<target> -runs=0` |
| cargo-vet | `cargo vet --locked` |
| semver checks | `cargo semver-checks --package ssg --package ssg-core --package ssg-rpc --package ssg-search` |

Two things to know about the table above.

**Clippy is two passes, not one.** The strict pass forbids `unwrap` and
`expect` in library code; the second pass allows them in tests and
examples, where they are the idiomatic way to fail a test. Running only
the first misses lints in `tests/` and `examples/`, which is precisely
how several rounds of green-locally / red-in-CI happened.

**The test matrix is 5 jobs, not 9.** Every OS runs on stable; the MSRV
floor (`1.88`) and beta run on Linux only. Those two toolchains answer a
compiler question rather than a platform one — "does this still build on
the floor we claim, and is it about to break on the next release?" —
so paying for them three times buys nothing. beta is
`continue-on-error`: an early warning, not a gate.

**One command covers unit and integration tests.** `--tests` builds and
runs `unittests src/lib.rs` as well as every `tests/*.rs` target — check
it with `cargo test --tests --no-run`, which lists the lib executable. A
separate `--lib` step therefore runs the same 3722 tests a second time,
which is why there is no longer one.

**The example-dependent suites are gated on `SSG_REQUIRE_EXAMPLES`.**
`tests/element_presence.rs` and `tests/jsonld_validation.rs` scan
`examples/*/public`, which only exists after `cargo build --examples` and
an example run. Rather than skipping silently, they check that variable:
CI's `examples` job sets it, so a gate finding nothing to scan there is a
real failure. Locally they skip unless you set it too:

```sh
SSG_REQUIRE_EXAMPLES=1 cargo test --test element_presence
```

The same variable gates the two suites that *build* examples rather than
just read them — `tests/example_outputs.rs` and
`tests/json_feed_compliance.rs`. Those run in the `examples` job and skip
elsewhere, because each builds and runs shipped examples that bind
`127.0.0.1:3000`, serialised behind a mutex, for about thirteen minutes.
That is what the `examples` job budgets for; it does not fit in the
per-OS `test` job, which is what made `test · ubuntu`, `test · macos` and
`test · windows` all hit their 20-minute timeout once the job moved to
`cargo test --tests`. Note that those mutexes are process-local statics,
so they do **not** serialise across the two test binaries — another
reason the pair must not run concurrently.

To run them locally:

```sh
cargo build --examples
SSG_REQUIRE_EXAMPLES=1 cargo test --test example_outputs -- --test-threads=1
SSG_REQUIRE_EXAMPLES=1 cargo test --test json_feed_compliance
```

One row above deserves a note. `cargo test --tests` includes
`tests/heap_frontmatter.rs`, which builds and runs the unpublished
`ssg-heap-probe` workspace member in release mode — about a minute — and
asserts the frontmatter path's peak heap on a 10,000-page corpus against
the baseline recorded before #578 was fixed. It lives in its own crate
because a counting allocator is `unsafe` by trait definition and the root
crate is `#![forbid(unsafe_code)]`. If it fails, something is holding
per-page state across the whole pass again; the assertion message says
what it measured.

## Test layout

| Location | Contains |
|---|---|
| `src/**` `#[cfg(test)]` | Unit tests, next to the code they cover |
| `tests/*.rs` | Integration and gate suites (58 files) |
| `tests/golden/` | Golden-file fixtures |
| `benches/bench.rs` | Criterion umbrella harness (`make bench`) |
| `fuzz/fuzz_targets/` | libFuzzer targets, replayed per push by ClusterFuzzLite |
| `crates/*/` | Workspace members, each with their own tests |

The goldens under `tests/golden/` are byte-for-byte snapshots compared by
`tests/golden_files.rs`. To reseed them after an intentional output change,
run the suite with `UPDATE_GOLDEN=1` in the environment, once per feature
set (`--features minify` writes the `.minify` variants); libtest rejects
unknown flags, so there is no `--update-golden` switch. Seed on one platform
and re-run the suite on the other before committing — the suite has caught
ordering that differed between APFS and ext4 — for example inside
`docker run --rm -v "$PWD":/work -w /work rust:1.90` on macOS. A run that
modifies no golden is the proof the snapshots are portable.

Beyond the usual unit and integration suites, several files exist purely
to stop documentation and inventory drifting from code. They are worth
knowing about, because when one fails the fix is almost never in the test:

| Gate | Asserts |
|---|---|
| `tests/readme_sync.rs` | README counts, versions and module table match the code |
| `tests/docs_accuracy.rs` | Documented behaviour matches actual behaviour |
| `tests/doc_links.rs` | Every internal Markdown link resolves |
| `tests/man_page.rs` | Every parser flag and subcommand reaches `ssg.1`, and no phantom flags |
| `tests/completions.rs` | The same for all four shells, plus each script parses in its own shell |
| `tests/development_docs.rs` | This file's CI table matches `ci.yml` |
| `tests/ci_test_coverage.rs` | Every `tests/*.rs` file is actually run by some CI job |

`tests/ci_test_coverage.rs` deserves a note: a test file that no workflow
runs is worse than no test, because it is counted as coverage while
asserting nothing. It caught 29 such files.

## The user manual

`docs/` doubles as the source for a rendered mdBook manual —
`book.toml` points mdBook's `src` at it, so every chapter is a file that
already exists rather than a second copy that can drift.
`docs/SUMMARY.md` is the index.

```sh
mdbook serve --open        # live preview
./scripts/build-manual.sh  # what CI runs
```

Adding a chapter means adding a file under `docs/` **and** a line in
`docs/SUMMARY.md` **and** an allowlist entry in `.gitignore` if it is
outside an already-admitted directory. `create-missing = false` makes
mdBook fail on a SUMMARY entry with no file, and
`scripts/check-docs-tracked.sh` catches the ignore case.

## Packaging and the install contract

```sh
make man                                # target/dist/man/ssg.1
make completions                        # target/dist/completions/*
make DESTDIR=/tmp/stage install         # staged install
./scripts/install-smoke.sh              # what CI checks
```

The man page and all four completion scripts are **generated from the
clap definition** (`src/cmd/man.rs`, `src/cmd/completions.rs`) and are not
committed. Never hand-edit a `.1` or a completion script: the generator is
the source, and the drift gates will reject the edit anyway.

`install` honours `PREFIX` (default `/usr/local`) and `DESTDIR`, and
`uninstall` is its exact inverse — the smoke test asserts that nothing is
left behind. PowerShell completions are generated but installed only when
a packager sets `PWSHCOMPDIR`, because PowerShell has no FHS convention
on Unix.

## Packaging

Distribution maintainers have their own document:
[`docs/packaging.md`](docs/packaging.md). It covers the licence grant,
the toolchain floor, vendored offline builds, which test suites to skip
in a package build and why, the exact install layout, and signature
verification.

## Supply chain

`cargo vet` gates every transitive dependency, and the exemption count in
`supply-chain/config.toml` is a **ratchet: it may only go down**. CI
compares it against `supply-chain/exemptions-baseline.txt`.

This has a practical consequence when adding a dependency. Run

```sh
cargo add <crate> && cargo vet --locked
```

*before* writing code against it. If the result is `Vetting Failed` with
an unvetted crate, the options are to audit it properly
(`cargo vet certify`) or to not add it — never to raise the baseline. Two
dependencies were declined this way while building the packaging
artefacts (`roff` and `clap_complete`); both were replaced with a narrow
emitter in-tree, which is why `src/cmd/man.rs` and
`src/cmd/completions.rs` exist.

## Release model

Versioning follows [ADR-0009](docs/adr/0009-versioning-policy-0.0.x-until-0.0.999.md):
`0.0.x` until `0.0.999`. Within that scheme the public API is still
checked by `cargo-semver-checks` against the crates.io baseline, so an
accidental breaking change fails CI rather than shipping.

Commits are signed (SSH format). Verify a range with:

```sh
git log --format='%h %G? %s' <range>
```

`G` means a good signature; anything else — `N`, `B`, `U` — is a problem.
Rebasing rewrites commits and re-signs only when `commit.gpgsign` is set,
so always re-check the range after one.

## Common failure modes

**Green locally, red in CI.** Almost always a narrower local command. Use
the table above verbatim.

**A drift gate fails after a legitimate change.** The gate is usually
right: update the code the documentation describes, or the documentation,
so the two agree. Do not relax the assertion — a gate that asserts less
than it appears to is the failure mode these were written against.

**`cargo test --tests` passes but the `examples` job fails.** The
example-dependent suites need built examples; see the
`SSG_REQUIRE_EXAMPLES` note above.

**Coverage dips below the floor.** Floors are 98.0% for regions, lines
and functions. `make coverage` reproduces the CI numbers exactly,
including the `--ignore-filename-regex` exclusions.

**A fuzz seed is not picked up in CI.** `fuzz/.gitignore` tracks only
`corpus/*/seed-*`, so a seed file must be named with that prefix or git
will not track it and CI will not see it.
