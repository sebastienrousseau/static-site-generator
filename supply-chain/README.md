<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Supply-chain attestation

This directory holds the `cargo-vet` attestation state for the `ssg`
workspace. `cargo-vet` complements `cargo-deny` (license + CVE
checking) with **per-crate audit attestation**: every transitive
dependency must either be (1) audited by us, (2) imported from a
trust set we recognise, or (3) explicitly exempted with rationale.

## Files

| File | Purpose | Edited by |
|---|---|---|
| `audits.toml` | Our own audit certificates. Add an entry here when *we* have read the source of a crate version and certify it `safe-to-deploy` or `safe-to-run`. | `cargo vet certify` |
| `config.toml` | Workspace policy: which trust sets we import, per-crate policies, and the exemption list (crates we have not yet audited but accept for now). | `cargo vet init` / `cargo vet exemption add` |
| `imports.lock` | Pinned hashes of imported trust-set audit files, so we detect tampering on `cargo vet` runs. | `cargo vet` |

## Trust sets imported

We trust the following organisations' published audit decisions:

- **Mozilla Firefox** — `https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml`
- **Bytecode Alliance (Wasmtime)** — `https://raw.githubusercontent.com/bytecodealliance/wasmtime/main/supply-chain/audits.toml`
- **Google** — `https://raw.githubusercontent.com/google/supply-chain/main/audits.toml`

Rationale: these organisations audit Rust crates as part of shipping
production code (Firefox, Wasmtime, ChromeOS) — their certificates
are reviewed by paid security teams. Adopting the union of their
trust sets transfers 90+ direct attestations to us at v0.0.45
bootstrap.

To refresh imported certificates: `cargo vet`. To rotate trust sets:
edit the `[imports.*]` block in `config.toml` and re-run `cargo vet`.

## Exemption policy

v0.0.45 bootstrap inherits **544 exemptions** — every transitive
crate not covered by an imported trust set is auto-exempted at
`safe-to-deploy` so `cargo vet` is green from day one. This is the
*starting line*, not the finish line.

Reduction path:
1. **Quarterly trust-set refresh.** Run `cargo vet` and review the
   delta — Mozilla/Google/Bytecodealliance add new audits monthly,
   so passive refresh drops our count steadily.
2. **Targeted audits for high-blast-radius crates.** When a crate
   appears in the build pipeline's critical path (parsers,
   network, FFI), we should audit it ourselves via
   `cargo vet certify <crate> <version>`.
3. **Drop entries on minor-bump churn.** Each crate version is its
   own exemption entry; bumping a dep often *adds* an entry rather
   than replacing one. Pruning is a chore: `cargo vet prune`.

Target trajectory: 544 → 350 by v0.1.0, → 100 by v1.0.0. Tracked as
a recurring v0.0.* milestone task (no per-release issue — the bar
moves down quarterly).

**Progress: 544 → 533 exemption entries (v0.0.47).** The first 13
first-party audit certificates landed in `audits.toml`, burning 11
exemption entries (two of the audited versions — `base64` 0.13.1 and
0.22.1 — had no exemption entry to burn). `cargo vet` now reports
100 fully-audited crates, up from 89 at the v0.0.45 bootstrap. This
data feeds the planned "544 to Zero" whitepaper.

## First-party audits (v0.0.47, plan §3 item 2.2)

All 13 certificates were written against the **exact published
source at the locked version**, taken from the local
`~/.cargo/registry/src` cache — not from same-author development
checkouts, which had drifted ahead of the pinned versions (e.g. the
`staticdatagen` working tree was at 0.0.10 while the lockfile pins
0.0.9).

Methodology per crate (`safe-to-deploy` bar):

1. Full read of `build.rs` (every same-author crate carries the
   same `version_check` rustc-version gate — no codegen, no
   network, no file writes).
2. Pattern sweep over `src/`: `unsafe` blocks, `proc-macro = true`,
   network access (`std::net`, `TcpStream`, `reqwest`, `ureq`,
   `hyper`), process spawning (`Command::new`, `process::Command`),
   filesystem deletion (`remove_dir_all` / `remove_file`),
   `include!`, and env/credential reads (`std::env::var`).
3. Manual inspection of **every** hit in context (e.g.
   `http-handle`'s `libc::sendfile` FFI, `staticdatagen`'s opt-in
   `sh -c` `CommandExecutor`, `staticweaver`'s feature-gated
   remote-template fetch).
4. Findings that fall short of disqualifying are disclosed verbatim
   in the certificate's `notes` field — an audit is only as
   trustworthy as its caveats.

Audited (crate@version — rough review time, seeding the "544 to
Zero" per-audit cost data):

| Certificate | Time | Notable finding disclosed |
|---|---|---|
| `staticdatagen@0.0.9` | ~40 min | opt-in `sh -c` `CommandExecutor` API (unused by ssg; no-shellout lint) |
| `frontmatter-gen@0.0.6` | ~20 min | env-var config knobs, parse-or-default |
| `html-generator@0.0.3` | ~20 min | comrak raw-HTML passthrough always on |
| `html-generator@0.0.6` | ~25 min | raw HTML opt-in + ammonia; emoji-data env override |
| `mdx-gen@0.0.1`, `@0.0.2` | ~10 min each | comrak raw-HTML passthrough |
| `http-handle@0.0.4`, `@0.0.5` | ~35 min each | `libc::sendfile` FFI; test-only env mutation; AGPL, dev-server-only |
| `metadata-gen@0.0.4` | ~10 min | clean |
| `staticweaver@0.0.2` | ~25 min | remote-template fetch behind non-default feature (not compiled here) |
| `rss-gen@0.0.5` | ~15 min | clean |
| `base64@0.13.1`, `@0.22.1` | ~10 min each | clean (`forbid(unsafe_code)`) |

Declined (kept as exemption, with reason):

- **`sha2@0.11.0`** — 59 `unsafe` sites (SIMD intrinsics /
  arch-specific digest backends). A grep-plus-inspection review
  cannot honestly certify hand-written crypto SIMD; this needs a
  dedicated review session or a trust-set audit of the 0.11 line.
  Its exemption stays until then.

## Exemption ratchet (CI)

`supply-chain/exemptions-baseline.txt` records the current exemption
count. The `vet` job in `ci.yml` fails any PR whose
`[[exemptions.*]]` entry count in `config.toml` differs from that
baseline: above it is a new exemption, below it is a burn-down that
must lower the baseline file in the same PR. The ratchet only moves
downward; never raise it.

## CI integration

`.github/workflows/ci.yml` runs `cargo vet --locked` in a dedicated
`vet` job (parallel with `cargo-deny`). The build fails if:

- An imported trust set returns a tampered audit file (detected via
  `imports.lock`).
- A new transitive dep appears that is neither audited, trust-set
  vouched, nor exempted.
- A pinned audit file URL is unreachable.

Adding a new dep that triggers a vet failure: run
`cargo vet suggest` for guidance, then `cargo vet certify` or
`cargo vet trust`. An exemption is not an option: the ratchet above
fails the PR, as `AGENTS.md` says.

## See also

- `deny.toml` — license + CVE gating (`cargo deny check`)
- `docs/adr/0006-cyclonedx-over-spdx.md` — SBOM format decision
- `SECURITY.md` — overall threat model and security defaults
