<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Security Policy

## Reporting a Vulnerability

If you have discovered a security vulnerability in SSG, **do not open a
public issue**. Email the maintainer at
[sebastian.rousseau@gmail.com](mailto:sebastian.rousseau@gmail.com)
with:

- A description of the vulnerability and its impact
- Steps to reproduce
- The version of SSG affected
- Any proof-of-concept code (gist or attachment, never inline)

You should receive an acknowledgement within 72 hours. Disclosure
follows the coordinated disclosure principle — a fix is developed and
released before details are made public, and you receive credit in
the advisory unless you prefer otherwise.

## Supported Versions

| Version | Supported |
|---|---|
| 0.0.61 (latest) | ✅ Security fixes |
| < 0.0.61 | ❌ Upgrade required |

`0.0.x` is pre-1.0; only the latest release receives security patches,
so the table above names the current version rather than a fixed floor.
Once `1.0.0` ships, the supported range will widen per the SemVer-aligned
policy in `CHANGELOG.md`.

## Security Posture

SSG is a build-time tool that processes untrusted input (Markdown,
YAML frontmatter, templates) into static HTML. The threat model
focuses on:

- **Local privilege elevation** — a malicious site source must not
  exfiltrate secrets from the build environment or escape the output
  directory via path traversal.
- **Supply-chain integrity** — every dependency is pinned and audited
  via `cargo deny check` on every CI run; GitHub Actions are pinned
  to commit SHAs (not tags).
- **Output integrity** — generated HTML must not echo unsanitised
  user content into HTML attributes or JavaScript contexts.

Out of scope:

- Runtime attacks against deployed static sites (those depend on the
  hosting provider's controls).
- Cryptographic identity of the operator (key management is delegated
  to git signing and GPG-signed releases).

## Security-Relevant Defaults

| Control | Default | Configuration |
|---|---|---|
| `unsafe` Rust | `#![forbid(unsafe_code)]` at every crate root | not configurable |
| Path traversal | `is_safe_path()` checks every output path against site root | not configurable |
| CSP | Inline `<style>`/`<script>` extracted to external files with SRI hashes; `unsafe-inline` not emitted | `CspPlugin` |
| HSTS | `Strict-Transport-Security: max-age=31536000` in deploy configs | `DeployPlugin` |
| Dependency advisories | `cargo deny check` runs on every CI build; allow-list is documented in `deny.toml` with rationale | `deny.toml` |
| Action pinning | All GitHub Actions pinned by 40-char commit SHA, not tag/branch | workflow files |
| Shell-out guard | `tools/lint-no-shellout.sh` (CI gate) refuses `Command::new("<shell-binary>")` in `src/` or `crates/`. Regression guard for the v0.0.44 port of `LlmPlugin` from `curl` to `ureq` (#520) | `tools/lint-no-shellout.sh` |

## Reproducible Builds

CI verifies that `cargo build --release --locked -p ssg` produces a
byte-identical binary across two consecutive runs on the same commit.
The job is in
[`.github/workflows/scheduled.yml`](.github/workflows/scheduled.yml)
under the `reproducible` job and runs weekly + on tag push.

The verification process:

1. Both builds set `SOURCE_DATE_EPOCH=1700000000` to pin
   timestamp-derived metadata.
2. `RUSTFLAGS="--remap-path-prefix=${GITHUB_WORKSPACE}=/build"` strips
   absolute checkout paths from the binary.
3. `cargo build --release --locked -p ssg` runs twice, with
   `cargo clean -p ssg` between runs.
4. SHA-256 of `target/release/ssg` is captured after each build.
5. The job fails if the two hashes differ.

To verify locally:

```sh
SOURCE_DATE_EPOCH=1700000000 \
  RUSTFLAGS="--remap-path-prefix=$(pwd)=/build" \
  cargo build --release --locked -p ssg
sha256sum target/release/ssg
cargo clean -p ssg
SOURCE_DATE_EPOCH=1700000000 \
  RUSTFLAGS="--remap-path-prefix=$(pwd)=/build" \
  cargo build --release --locked -p ssg
sha256sum target/release/ssg
```

The two sums must match.

### Non-determinism sources investigated

If the CI job fails after a change to the build, candidate causes:

- **Embedded paths** — anything that reads `file!()` or `env!("CARGO_MANIFEST_DIR")`
  into the binary; mitigation is `--remap-path-prefix`.
- **`build.rs` non-determinism** — generators that read clock, hostname,
  or randomness; check `build.rs` for `SystemTime::now()` or
  `gethostname` use.
- **Parallel codegen ordering** — `codegen-units > 1` with non-frozen
  symbol ordering. Release profile already pins `codegen-units = 1`.
- **Dependency churn** — `cargo build --locked` enforces the lockfile;
  if a transitive crate changed between the two runs, the cache was
  poisoned.
- **`include_bytes!` of mutable files** — anything generated outside
  the lockfile's reach.

The reproducible-build hash is **not** currently part of the release
attestation — that step is tracked in #424's deferred follow-up. Once
the verification has been green for ≥ 4 consecutive weekly runs, the
hash will be embedded in the SLSA provenance attestation alongside
the existing CycloneDX SBOM.

## Build Provenance

Every release tag triggers
[`.github/workflows/release.yml`](.github/workflows/release.yml),
which produces:

- Multi-platform binaries (Linux glibc + musl, macOS arm64 + x86_64,
  Windows MSVC) with SHA-256 checksums.
- Detached GPG signatures (when `GPG_PRIVATE_KEY` secret is configured).
- A GitHub-native build provenance attestation via
  `actions/attest-build-provenance` (signed via Sigstore).
- A **SLSA v1.1 Level 3** build provenance attestation via the
  [`slsa-framework/slsa-github-generator`](https://github.com/slsa-framework/slsa-github-generator)
  reusable workflow — emits `ssg-<tag>.intoto.jsonl` covering every
  release artefact, with the runner identity attested by GitHub OIDC.
  A `verify-provenance` job re-runs `slsa-verifier verify-artifact`
  against the just-published release to catch silent breakage.
- A CycloneDX 1.5 SBOM and an SPDX 2.3 SBOM
  (`scheduled.yml` `sbom` job).

To verify a release binary:

```sh
# Verify checksum
sha256sum -c ssg-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz.sha256

# Verify GPG signature (import the release-signing key first — see below)
gpg --import KEYS.asc
gpg --verify ssg-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz.asc \
             ssg-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz

# Verify GitHub-native Sigstore attestation
gh attestation verify ssg-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz \
                      --owner sebastienrousseau

# Verify SLSA v1.1 Level 3 provenance
slsa-verifier verify-artifact ssg-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz \
  --provenance-path ssg-vX.Y.Z.intoto.jsonl \
  --source-uri github.com/sebastienrousseau/static-site-generator \
  --source-tag vX.Y.Z
```

### Release-signing key

The `gpg --verify` step above needs the public key. It is committed to
this repository as [`KEYS.asc`](KEYS.asc), and the fingerprint is:

```text
4B7F16C909C7A8EE9BED338A4F047EDF5F90F638
```

`Sebastien Rousseau <sebastian.rousseau@gmail.com>`, ed25519,
signing-only, expires 2028-08-16.

Verify the fingerprint out of band before trusting it. A key fetched
over the same channel as the artefact it signs proves nothing on its
own — which is why the Sigstore attestation and SLSA provenance above
remain the stronger checks. The detached signature exists for consumers
who need an offline check with the `gpg` their distribution already
ships.

Releases before v0.0.51 carry no `.asc`: the signing job was gated on a
secret that had not been configured, and a job-ordering bug meant the
signatures it did produce never reached the release
(fixed in #678).

The full downstream verification guide — including the regulatory
cross-reference to EO 14028, the EU CRA, and FedRAMP — lives at
[`docs/security/sbom-provenance.md`](docs/security/sbom-provenance.md).

## SBOM (Software Bill of Materials)

SSG ships **two** CycloneDX SBOMs that procurement and security
reviewers can consume without privileged repository access:

### Per-site embedded SBOM (`SbomPlugin`, build time)

Every site built with SSG contains `sbom.cdx.json` at the site root
and a per-page `<link>` element pointing at it:

```html
<link rel="sbom"
      type="application/vnd.cyclonedx+json"
      href="/sbom.cdx.json">
```

`rel="sbom"` is the [IANA-registered link
relation](https://www.iana.org/assignments/link-relations/) for SBOM
discovery (registered 2023). The file is CycloneDX 1.5 JSON
covering the SSG generator (purl, licences, externalReferences) and
the site itself as a top-level `metadata.component`. Transitive
Cargo dependencies are **not** in this file — they live in the
CI-generated SBOM (next section).

To fetch and validate against the spec:

```sh
# Fetch from any deployed site
curl -sL https://example.com/sbom.cdx.json | jq '.metadata.timestamp'

# Validate against the CycloneDX 1.5 JSON Schema
cyclonedx validate --input-file sbom.cdx.json \
                   --input-version v1_5 \
                   --input-format json
```

### Release-artifact SBOMs (CI, with transitive deps)

The `scheduled.yml` `sbom` job emits **two** SBOM formats covering
every transitive Cargo dependency, both attested by Sigstore via
`actions/attest-build-provenance`:

- `cargo cyclonedx --format json --spec-version 1.5`
  → uploaded as the `sbom-cyclonedx` build artefact (`*.cdx.json`).
- `cargo sbom --output-format spdx_json_2_3`
  → uploaded as the `sbom-spdx` build artefact (`ssg.spdx.json`).

SPDX 2.3 is the format that NTIA "minimum elements" guidance,
US EO 14028, and the EU Cyber Resilience Act all currently accept.
SPDX 3.0 emission is pending upstream tool support; the rationale
is documented in [`docs/security/sbom-provenance.md`](docs/security/sbom-provenance.md).

To fetch the CI-generated SBOMs for a specific release:

```sh
gh run download <run-id> --repo sebastienrousseau/static-site-generator \
                          --name sbom-cyclonedx
gh run download <run-id> --repo sebastienrousseau/static-site-generator \
                          --name sbom-spdx
```

### Verification of the per-page link

The build-time link injection is idempotent: pages already
containing `rel="sbom"` are left unchanged. Source: `src/sbom.rs`.

## Cryptographic Material

SSG itself does not generate or store cryptographic keys at runtime.
The release pipeline uses:

- **GPG** — for detached signatures over release artifacts. The public
  key is published at the project URL and rotated per the standard
  GPG hygiene cycle.
- **Sigstore** — for keyless build provenance attestation; uses the
  GitHub OIDC token at workflow runtime.
- **SHA-256** — for content-integrity checksums.
- **SHA-384** — for Subresource Integrity hashes embedded in HTML
  by `CspPlugin`.

No long-term secrets are stored in the repository.

## Hardening Roadmap

Tracked in the milestone "Compliance, Security & Observability":

- **Reproducible builds across all release platforms** — currently
  Linux-only; macOS and Windows reproducibility is platform-dependent
  and tracked separately.
- **Post-quantum content provenance** — embedding ML-DSA (FIPS 204)
  signatures in `<meta name="content-signature">` tags. Tracked in
  issue #420; needs a design RFC before implementation.
- **Transitive dependency rotation** — six unmaintained crates remain
  in the allow-list (`yaml-rust`, `paste`, `fxhash`, `number_prefix`,
  `bincode`, `rand 0.8.5`). Tracked in issue #464.
- **OpenTelemetry build traces** — feature-gated observability for
  the plugin pipeline. Tracked in issue #422.

---

*Last reviewed: 2026-05-10. This file lives at repository root and
is the canonical security policy for the SSG project.*
