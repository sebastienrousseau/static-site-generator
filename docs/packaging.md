<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Packaging SSG

Written for distribution maintainers. If you are packaging SSG for a
distro, everything you need should be here; if something is missing,
that is a bug — please open an issue rather than guessing.

## Contents

- [Licence grant](#licence-grant)
- [Minimum toolchain](#minimum-toolchain)
- [Dependency pin model](#dependency-pin-model)
- [Building offline](#building-offline)
- [Running the tests offline](#running-the-tests-offline)
- [Installing](#installing)
- [What gets installed](#what-gets-installed)
- [Verifying signatures](#verifying-signatures)
- [Reproducibility](#reproducibility)
- [Existing packaging](#existing-packaging)

## Licence grant

Dual-licensed **MIT OR Apache-2.0**, at your option. Full texts are in
[`LICENSES/`](../LICENSES/) as well as `LICENSE-MIT` and
`LICENSE-APACHE` at the repository root.

The repository is [REUSE](https://reuse.software) 3.3 compliant and this
is checked in CI, so every file's licensing is machine-readable. To
generate a manifest for your packaging metadata:

```sh
reuse lint
reuse spdx > ssg.spdx
```

Bulk declarations for content, fixtures and generated files live in
[`REUSE.toml`](../REUSE.toml); source files carry inline SPDX headers.

## Minimum toolchain

**Rust 1.89.0**, declared as `rust-version` in `Cargo.toml` and enforced
in CI by a build against that exact toolchain.

Read the [MSRV policy](../README.md#minimum-supported-rust-version)
before pinning: the floor is set by dependencies rather than chosen, and
it may rise in any release because `0.0.z` carries no compatibility
promise.

**This project makes no claim about distro-packaged Rust versions.** An
unverified compatibility table is worse than none, so we do not publish
one. Check `rust-version` against your toolchain directly.

## Dependency pin model

`Cargo.lock` is committed and CI builds with `--locked`. Package against
the lockfile: it is the dependency set every gate in this repository ran
against.

Dependencies are gated by [`cargo vet`](../supply-chain/README.md) with
an exemption ratchet — the exemption count may only decrease, so a
release cannot quietly gain an unaudited dependency.

If your distro requires unbundling or version-relaxing crates, be aware
that `cargo update` moves you off the tested set. `cargo deny check`
(config in `deny.toml`) will tell you whether a substitution keeps the
licence and advisory posture intact.

## Building offline

```sh
cargo vendor vendor/                # once, with network
mkdir -p .cargo
cat >> .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
cargo build --release --locked --offline
```

`build.rs` does no network access. Nothing in the build downloads at
compile time.

## Running the tests offline

```sh
cargo test --offline --locked --tests
```

Two notes on what that does and does not cover:

- The example-building suites (`tests/example_outputs.rs`,
  `tests/json_feed_compliance.rs`) **skip** unless
  `SSG_REQUIRE_EXAMPLES=1` is set. They take about thirteen minutes and
  bind `127.0.0.1:3000`. Leave them skipped in a package build; they
  exercise the shipped examples rather than the binary you are shipping.
- Wall-clock performance budgets assert only on Linux, and even there
  they measure the machine as much as the code. A failure in
  `tests/perf_budgets.rs` on a loaded build host is not a defect in the
  release.

No test requires network access.

## Installing

The GNU install contract, honouring `PREFIX` and `DESTDIR`:

```sh
make PREFIX=/usr DESTDIR="$pkgdir" install
```

Every directory is overridable independently — `BINDIR`, `MAN1DIR`,
`DOCDIR`, `BASHCOMPDIR`, `ZSHCOMPDIR`, `FISHCOMPDIR` — so you can match
your distro's layout without patching the makefile. `make uninstall`
is an exact inverse, and CI asserts that on every push.

`make install-strip` strips the installed binary.

## What gets installed

With `PREFIX=/usr`:

| Path | Contents |
|---|---|
| `/usr/bin/ssg` | The binary |
| `/usr/share/man/man1/ssg.1` | Man page |
| `/usr/share/bash-completion/completions/ssg` | bash completions |
| `/usr/share/zsh/site-functions/_ssg` | zsh completions |
| `/usr/share/fish/vendor_completions.d/ssg.fish` | fish completions |
| `/usr/share/doc/ssg/` | README, CHANGELOG, both licences |

The man page and all completions are **generated from the CLI
definition** at build time, not committed, so they cannot drift from
`--help`. CI asserts that every parser flag appears in both.

PowerShell completions are generated but installed only if you set
`PWSHCOMPDIR`, since there is no FHS location for them.

## Verifying signatures

Release artefacts carry, per [`SECURITY.md`](../SECURITY.md):

- `SHA256SUMS` for every archive
- Detached GPG signatures — the signing key is
  [`KEYS.asc`](../KEYS.asc) in the repository
- A SLSA v1.1 Level 3 provenance attestation
- A CycloneDX SBOM

```sh
gpg --import KEYS.asc
gpg --verify ssg-x86_64-unknown-linux-gnu.tar.gz.asc
sha256sum --check SHA256SUMS --ignore-missing
```

Tags and commits are signed. Verify a tag with `git verify-tag v0.0.58`.

## Reproducibility

Builds are byte-reproducible for a given toolchain and target, and CI
enforces it: `determinism.yml` builds the same input twice and compares
hashes, and compares output across operating systems.

`SOURCE_DATE_EPOCH` is honoured when generating the man page, so a
rebuild produces an identical `ssg.1`.

This is a statement about *this project's* output. Whether your build
environment is reproducible end to end also depends on your toolchain
and packaging, which we cannot assert for you.

## Prebuilt targets

Every release publishes archives for:

| Target | Notes |
|---|---|
| `x86_64-unknown-linux-gnu` | also `.deb` (amd64) and `.rpm` |
| `x86_64-unknown-linux-musl` | static, runs anywhere |
| `aarch64-unknown-linux-gnu` | also `.deb` (arm64) and `.rpm` |
| `aarch64-unknown-linux-musl` | static, runs anywhere |
| `aarch64-apple-darwin` | also `.pkg` |
| `x86_64-apple-darwin` | also `.pkg` |
| `x86_64-pc-windows-msvc` | `.zip`, also `.msi` |

Linux builds run natively on their own architecture rather than being
cross-compiled, so there is no cross-linker toolchain between the
release and the binary you receive.

## Existing packaging

[`packaging/`](../packaging/) has working definitions for Homebrew, Arch
(PKGBUILD), Debian, Scoop and WinGet, and a container image is published
to GHCR. Reuse or adapt them freely.

If you package SSG for a distribution not listed there, please open an
issue — we would like to track it, and once two distributions carry it
the README gets a Repology badge.
