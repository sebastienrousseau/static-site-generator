// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Documentation accuracy regression gate.
//!
//! Every claim the README, CHANGELOG, or docs/ makes about the
//! codebase that can be cross-referenced from `Cargo.toml`, source
//! files, or CI configuration is verified here.
//!
//! ## What this catches
//!
//! - README test-count claims drifting from `cargo test --lib` reality.
//! - Version mismatches between `Cargo.toml` and the README install
//!   instruction / SECURITY.md.
//! - Coverage-floor claims in README vs `ci.yml` `COVERAGE_*_FLOOR`
//!   env vars.
//! - WCAG version claims in README vs `src/accessibility.rs`.
//! - MSRV claims in README vs `Cargo.toml` `rust-version`.
//!
//! These are the load-bearing claims procurement, security, and SEO
//! reviewers cite when evaluating SSG. A drift between marketing
//! copy and source-of-truth files would silently undermine those
//! claims; this test fails CI before that happens.
//!
//! ## Methodology
//!
//! Read source-of-truth files at test time and grep the README /
//! CHANGELOG / SECURITY.md for the corresponding claim. If the
//! source-of-truth changed without the docs being updated, the test
//! prints a directly-actionable diff and panics. The test is
//! tolerant of "soft" claims (`approximately`, `1,000+`, etc.) by
//! design — it only fails on **exact-number** assertions that have
//! gone stale.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(workspace().join(rel))
        .unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Extracts the `name = ...` and `version = ...` values from the
/// `[package]` table of the root `Cargo.toml`. Cheap regex-free
/// parse so we don't pull a TOML crate just for this test.
fn cargo_package_field(field: &str) -> String {
    let toml = read("Cargo.toml");
    let mut in_package = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(&format!("{field} = ")) {
            return rest
                .trim()
                .trim_start_matches('"')
                .split('"')
                .next()
                .unwrap_or_default()
                .to_string();
        }
    }
    panic!("Cargo.toml [package] missing field `{field}`");
}

/// Counts `#[test]`, `#[tokio::test]`, etc. attributes across the
/// `src/` tree. This approximates the lib test count without
/// running cargo, which would loop into us.
fn count_src_lib_tests() -> usize {
    let mut count = 0;
    walk_for_count(&workspace().join("src"), &mut count);
    count
}

fn walk_for_count(dir: &Path, count: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_for_count(&p, count);
        } else if p.extension().is_some_and(|e| e == "rs") {
            if let Ok(content) = fs::read_to_string(&p) {
                // Count `#[test]` and `#[tokio::test]` annotations
                // appearing as the immediate decorator of a function.
                for line in content.lines() {
                    let t = line.trim();
                    if t == "#[test]" || t.starts_with("#[tokio::test") {
                        *count += 1;
                    }
                }
            }
        }
    }
}

// =====================================================================
// Version sync
// =====================================================================

#[test]
fn readme_version_matches_cargo_toml() {
    let cargo_version = cargo_package_field("version");
    let readme = read("README.md");

    // README often cites the version in `cargo install ssg --version
    // X.Y.Z` or the docs.rs link `docs.rs/ssg/X.Y.Z`. We assert at
    // least one full-form occurrence.
    let badge = format!("v{cargo_version}");
    assert!(
        readme.contains(&cargo_version) || readme.contains(&badge),
        "Cargo.toml version `{cargo_version}` does not appear in README. \
         Either bump the README claim or revert the Cargo.toml change. \
         Soft-mention forms (~X.Y, latest, etc.) don't satisfy this gate."
    );
}

// =====================================================================
// Lib test count
// =====================================================================

#[test]
fn readme_lib_test_count_is_in_the_right_ballpark() {
    // The README has historically cited a specific test count (e.g.
    // "1,640 unit tests"). When source-tree test annotations grow
    // far past the cited number, the claim becomes stale.
    //
    // This test extracts the README's lib test count (any
    // X-or-X,XXX form followed by "unit tests" or "lib tests") and
    // asserts it's within ±15% of the source-tree count. The 15%
    // band tolerates "round numbers" (1,640 staying stable while
    // 1,685 ticks past) without demanding constant README churn,
    // but a 200+-test drift will fail this gate and force an update.
    let readme = read("README.md");
    let actual = count_src_lib_tests();

    let mut claim: Option<usize> = None;
    for line in readme.lines() {
        let lower = line.to_lowercase();
        if !lower.contains("unit test") && !lower.contains("lib test") {
            continue;
        }
        // Capture the first digit run in the line (handles 1,640 by
        // stripping commas).
        let mut buf = String::new();
        for ch in line.chars() {
            if ch.is_ascii_digit() {
                buf.push(ch);
            } else if !buf.is_empty() {
                if ch == ',' {
                    continue;
                }
                break;
            }
        }
        if let Ok(n) = buf.parse::<usize>() {
            if n >= 100 {
                claim = Some(n);
                break;
            }
        }
    }

    let Some(claim) = claim else {
        eprintln!(
            "[docs_accuracy] README has no `N unit tests` / `N lib tests` \
             claim — skipping. To enforce, add a claim like \
             `1,685 unit tests`."
        );
        return;
    };

    let lower = claim.saturating_sub(claim * 15 / 100);
    let upper = claim + claim * 15 / 100;
    assert!(
        actual >= lower && actual <= upper,
        "README cites {claim} unit/lib tests but `src/` has {actual} \
         `#[test]` annotations (±15% band: {lower}..={upper}). \
         Update the README claim or investigate the test-count drift."
    );
}

// =====================================================================
// Coverage floors
// =====================================================================

#[test]
fn readme_coverage_claims_match_ci_floors() {
    let ci = read(".github/workflows/ci.yml");
    let readme = read("README.md");

    // Extract the COVERAGE_REGIONS_FLOOR / COVERAGE_LINES_FLOOR /
    // COVERAGE_FUNCTIONS_FLOOR env values from ci.yml.
    let floor = |name: &str| -> Option<u32> {
        let needle = format!("{name}: \"");
        let idx = ci.find(&needle)?;
        let after = &ci[idx + needle.len()..];
        let close = after.find('"')?;
        after[..close].parse::<f32>().ok().map(|f| f as u32)
    };

    let regions = floor("COVERAGE_REGIONS_FLOOR")
        .expect("ci.yml missing COVERAGE_REGIONS_FLOOR");

    // The README cites "95% region", "97% line", etc. Verify the
    // numeric region claim matches.
    let readme_lower = readme.to_lowercase();
    let region_pat = format!("{regions}% region");
    assert!(
        readme_lower.contains(&region_pat),
        "ci.yml COVERAGE_REGIONS_FLOOR is {regions}% but README's \
         region-coverage claim doesn't match. Search: `{region_pat}`."
    );
}

// =====================================================================
// WCAG version sync
// =====================================================================

#[test]
fn readme_wcag_version_matches_accessibility_module() {
    // src/plugins/accessibility.rs declares the WCAG version in its module
    // docstring and uses it as the `wcag_version` field default.
    let accessibility = read("src/plugins/accessibility.rs");
    let readme = read("README.md");

    // Pull "WCAG 2.X" from the accessibility module (first match).
    let wcag_version = ["2.2", "2.1", "2.0"]
        .iter()
        .find(|v| accessibility.contains(&format!("WCAG {v}")))
        .copied()
        .unwrap_or("2.1");

    let claim = format!("WCAG {wcag_version}");
    assert!(
        readme.contains(&claim),
        "src/accessibility.rs targets `{claim}` but README doesn't \
         mention it. Update README or fix the module-level docstring."
    );
}

// =====================================================================
// MSRV sync
// =====================================================================

#[test]
fn readme_msrv_matches_cargo_toml() {
    let msrv = cargo_package_field("rust-version");
    let readme = read("README.md");

    // README either cites the MSRV explicitly (`Rust 1.X+`) or has
    // a "Rust version" / "MSRV" line. We accept any of the common
    // forms.
    let forms = [
        format!("Rust {msrv}"),
        format!("rust {msrv}"),
        format!("MSRV: {msrv}"),
        format!("MSRV {msrv}"),
        format!("rust-version = \"{msrv}\""),
    ];
    let found = forms.iter().any(|f| readme.contains(f));
    if !found {
        eprintln!(
            "[docs_accuracy] README has no MSRV claim — soft gate. \
             Cargo.toml MSRV is `{msrv}`. Consider adding to README \
             so consumers see it without reading Cargo.toml."
        );
    }
}

/// Every statement of the MSRV names `rust-version` from the root
/// `Cargo.toml`. A raise that misses one fails here, because the
/// expected text (built from the current floor) is then absent. The
/// short form (`1.89`) is what the CI matrices and `build.rs` use.
#[test]
fn every_msrv_statement_names_rust_version() {
    let msrv = cargo_package_field("rust-version");
    let short = msrv
        .rsplit_once('.')
        .map_or(msrv.as_str(), |(head, _)| head);
    let expected: Vec<(&str, String)> = vec![
        ("README.md", format!("badge/MSRV-{msrv}-")),
        ("README.md", format!("alt=\"MSRV {msrv}\"")),
        ("README.md", format!("Requires **Rust {msrv}+**")),
        ("README.md", format!("| **MSRV** | Rust {msrv} |")),
        ("README.md", format!("**Rust {msrv}.**")),
        ("README.md", format!("`[stable, {short}]`")),
        (
            "CONTRIBUTING.md",
            format!("(https://rustup.rs/) {msrv} or later"),
        ),
        ("DEVELOPMENT.md", format!("| MSRV | **{msrv}** |")),
        ("DEVELOPMENT.md", format!("binaries on the MSRV `{short}`")),
        ("DEVELOPMENT.md", format!("floor (`{short}`)")),
        ("build.rs", format!("let min_version = \"{short}\";")),
        ("Dockerfile", format!("FROM rust:{short}-slim@")),
        (
            ".github/workflows/ci.yml",
            format!("toolchain: \"{short}\" # MSRV floor"),
        ),
        (
            ".github/workflows/scheduled.yml",
            format!("toolchain: [stable, \"{short}\"]"),
        ),
        ("docs/guide/installation.md", format!("**Rust {msrv}+**")),
        ("docs/packaging.md", format!("**Rust {msrv}**, declared")),
        (
            "docs/architecture/regression-contract.md",
            format!("MSRV ({short})"),
        ),
    ];
    let mut stale: Vec<String> = expected
        .iter()
        .filter(|(file, text)| !read(file).contains(text.as_str()))
        .map(|(file, text)| format!("{file}: no `{text}`"))
        .collect();
    let members =
        fs::read_dir(workspace().join("crates")).expect("read crates/");
    for entry in members.flatten() {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        let declares = text.lines().any(|l| l.starts_with("rust-version = \""));
        if declares && !text.contains(&format!("rust-version = \"{msrv}\"")) {
            stale.push(format!(
                "{}: rust-version is not {msrv}",
                manifest.display()
            ));
        }
    }
    assert!(
        stale.is_empty(),
        "MSRV statements disagree with rust-version {msrv}:\n{}",
        stale.join("\n")
    );
}

// =====================================================================
// SECURITY.md cross-check
// =====================================================================

#[test]
fn security_md_exists_and_mentions_disclosure() {
    let security = read("SECURITY.md");
    assert!(
        security
            .to_lowercase()
            .contains("reporting a vulnerability")
            || security.to_lowercase().contains("disclos"),
        "SECURITY.md must describe a vulnerability disclosure process"
    );
}

// =====================================================================
// SBOM emission verified at the source level
// =====================================================================

#[test]
fn sbom_cyclondx_version_matches_module_claim() {
    // src/plugins/sbom.rs commits to "1.5" in both the rustdoc and the
    // emitted JSON. If those drift, downstream tooling that expects
    // the docstring version will validate against the wrong schema.
    let sbom = read("src/plugins/sbom.rs");
    let docstring_v =
        sbom.contains("CycloneDX 1.5") || sbom.contains("`CycloneDX` 1.5");
    let emit_v = sbom.contains("\"specVersion\": \"1.5\"");
    assert!(
        docstring_v && emit_v,
        "src/sbom.rs CycloneDX version drift: docstring says 1.5 = {docstring_v}, \
         emit says 1.5 = {emit_v}"
    );
}

#[test]
fn features_matrix_is_exhaustive() {
    // docs/features-coverage.md must list every plugin module under
    // src/plugins/<name>.rs in its first table. Adding a new plugin
    // without updating the matrix fires this assertion with the
    // offending module name — keeping examples/benches/regression
    // coverage discoverable as the surface grows.
    let matrix = read("docs/features-coverage.md");

    // The plugin module names to check are derived from src/plugins/*.rs
    // (excluding mod.rs, plugin.rs, plugins.rs which are infrastructure,
    // and the postprocess + builtin_templates submodules).
    let plugins_dir = workspace().join("src/plugins");
    let mut module_names = Vec::new();
    for entry in fs::read_dir(&plugins_dir).unwrap().flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            // Skip infrastructure modules and the registry shim;
            // those are themselves covered by the "Plugin trait +
            // lifecycle" + "Plugin registry" rows.
            let skip = matches!(stem, "mod" | "plugins"); // `plugin` IS listed
            if !skip {
                module_names.push(stem.to_string());
            }
        }
    }

    let mut missing = Vec::new();
    for name in &module_names {
        // The matrix lists module names in `<code>` backticks in the
        // second column. A bare substring is precise enough because
        // the names are unique (e.g. `accessibility`, `ai`, `csp`).
        let needle = format!("`{name}`");
        if !matrix.contains(&needle) {
            missing.push(name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "docs/features-coverage.md is missing rows for these plugin modules:\n  {}\n\nAdd one row per module to the first table.",
        missing.join("\n  ")
    );
}

// =====================================================================
// Claims reconciliation wording gates (v0.0.47 plan §3 item 2.5)
// =====================================================================

#[test]
fn compilation_claims_use_bounded_memory_batch_wording() {
    // `src/core/streaming.rs` is a *batch* compiler with a memory
    // budget — content is compiled batch-by-batch to disk, not
    // incrementally streamed. v0.0.47 renamed the user-facing claim
    // from "streaming compilation" to "bounded-memory batch
    // compilation"; this gate pins the new wording so it can't
    // drift back. The word "streaming" remains legitimate for the
    // lol_html rewriting path and chunked file I/O — only the exact
    // phrase "streaming compilation" is banned from these docs.
    for rel in ["README.md", "BENCHMARKS.md", "docs/guide/streaming.md"] {
        let doc = read(rel).to_lowercase();
        assert!(
            !doc.contains("streaming compilation"),
            "{rel} still says `streaming compilation` — the honest \
             wording is `bounded-memory batch compilation` \
             (core/streaming.rs is a batch compiler; see v0.0.47 \
             plan §3 item 2.5)."
        );
        assert!(
            doc.contains("bounded-memory batch"),
            "{rel} lost the `bounded-memory batch` wording that \
             describes core/streaming.rs. Restore it (v0.0.47 plan \
             §3 item 2.5)."
        );
    }
}

#[test]
fn pqc_claims_are_posture_guidance_not_capability() {
    // SSG's PQC surface is configuration *guidance* (TLS/PQC notes
    // in edge-header emitters + an HSTS/TLS posture audit gate) —
    // not post-quantum cryptography. ML-DSA content-provenance
    // signing is roadmap (#579 / SECURITY.md Hardening Roadmap).
    // The README must describe this as "PQC posture guidance" and
    // never as a "PQC-aware" / "post-quantum" capability.
    let readme = read("README.md");
    assert!(
        readme.contains("PQC posture guidance"),
        "README lost the `PQC posture guidance` wording for the \
         edge-headers feature (v0.0.47 plan §3 item 2.5)."
    );
    assert!(
        !readme.contains("PQC-aware"),
        "README says `PQC-aware` — that overstates the feature. Use \
         `PQC posture guidance` (v0.0.47 plan §3 item 2.5)."
    );
    assert!(
        !readme.to_lowercase().contains("post-quantum"),
        "README claims `post-quantum` as a capability. SSG emits \
         PQC *posture guidance*; ML-DSA provenance signing is \
         roadmap (#579)."
    );
}

#[cfg(test)]
mod helpers {
    use super::*;

    #[test]
    fn count_src_lib_tests_is_nonzero() {
        let n = count_src_lib_tests();
        assert!(n > 100, "expected >100 lib tests in src/, got {n}");
    }

    #[test]
    fn cargo_package_name_is_ssg() {
        assert_eq!(cargo_package_field("name"), "ssg");
    }
}

/// The README's integration-target count must match `tests/`.
///
/// Its sibling claim — the lib test count on the same README line — has
/// been gated with a ±15% band since it was first written. The
/// integration-target half never was, and it drifted: the line read 51
/// while `tests/` held 57. Unlike the lib count, this one is exact and
/// free to derive, so it is asserted exactly rather than banded.
#[test]
fn readme_integration_target_count_matches_tests_dir() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let actual = fs::read_dir(root.join("tests"))
        .expect("read tests/")
        .filter_map(Result::ok)
        .filter(|e| {
            e.path().extension().is_some_and(|x| x == "rs")
                && e.file_type().is_ok_and(|t| t.is_file())
        })
        .count();

    let readme = read("README.md");
    let claim = readme
        .lines()
        .find(|l| l.contains("integration test target"))
        .and_then(|line| {
            let idx = line.find("integration test target")?;
            let head = line[..idx].trim_end();
            let digits: String = head
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit() || *c == ',')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            digits.replace(',', "").parse::<usize>().ok()
        });

    let Some(claim) = claim else {
        panic!(
            "README no longer states an integration test target count. \
             The claim is what this gate guards; if it was removed on \
             purpose, remove this test in the same commit."
        );
    };

    assert_eq!(
        claim, actual,
        "README claims {claim} integration test targets; `tests/` holds \
         {actual} `.rs` files."
    );
}
