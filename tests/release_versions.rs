// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Release version-coherence gate.
//!
//! Every publishable crate in the workspace must carry the same
//! version as the root `ssg` crate, and every internal dependency
//! pin must name that same version.
//!
//! ## What this catches
//!
//! Version drift between the root crate and the workspace members it
//! depends on. `release.yml` publishes `ssg-core`, `ssg-rpc-macro`,
//! `ssg-rpc`, `ssg-search` and `ssg-a11y` to crates.io before `ssg`
//! itself, because crates.io verifies path dependencies against the
//! registry. If a member's version was not bumped along with the
//! root, the first `cargo publish` of the release aborts with
//! `error: crate <name>@<version> already exists on crates.io index`
//! and nothing downstream of it publishes.
//!
//! That is not hypothetical. Between v0.0.47 and v0.0.50 the members
//! stayed pinned at `0.0.47` while the root advanced to `0.0.50`, so
//! the v0.0.48 release failed at the very first publish, `ssg` was
//! skipped, and crates.io fell two versions behind the README's own
//! install instruction.
//!
//! The failure surfaced only after a tag had been pushed — the point
//! at which it is most expensive, because a tag that produced no
//! release has to be deleted and re-cut. This test moves the same
//! signal to every `cargo test` run.
//!
//! ## Methodology
//!
//! Parse the root and member manifests directly. No TOML crate: the
//! fields involved are single-line `key = "value"` entries, and
//! `tests/docs_accuracy.rs` sets the precedent for keeping this kind
//! of gate dependency-free.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// Members that `release.yml` publishes to crates.io, in the
/// dependency order it uses. `ssg-wasm` is absent deliberately: it
/// carries `publish = false` and is not a dependency of `ssg`.
const PUBLISHED_MEMBERS: &[&str] = &[
    "ssg-core",
    "ssg-rpc-macro",
    "ssg-rpc",
    "ssg-search",
    "ssg-a11y",
    "ssg-i18n",
    // Added to release.yml's publish loop without being added here, so it
    // was published with no version gate: this test compares the two lists
    // precisely to catch that, and did.
    "ssg-mcp",
];

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn manifest_of(member: &str) -> PathBuf {
    workspace().join("crates").join(member).join("Cargo.toml")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Returns the `version = "…"` value from a manifest's `[package]`
/// table, ignoring `version` keys that appear in dependency tables.
fn package_version(path: &Path) -> String {
    let toml = read(path);
    let mut in_package = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("version = \"") {
                if let Some(v) = rest.split('"').next() {
                    return v.to_string();
                }
            }
        }
    }
    panic!("no [package] version in {}", path.display());
}

/// Returns every `version = "…"` pin attached to an internal
/// `ssg-*` path dependency in `path`, as `(crate_name, pinned)`.
///
/// Matches the single-line inline-table form the workspace uses
/// throughout, e.g.
/// `ssg-core = { path = "crates/ssg-core", version = "0.0.50" }`.
fn internal_pins(path: &Path) -> Vec<(String, String)> {
    let toml = read(path);
    let mut pins = Vec::new();
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some((name, rest)) = trimmed.split_once(" = ") else {
            continue;
        };
        if !name.starts_with("ssg-") || !rest.contains("path = ") {
            continue;
        }
        if let Some(after) = rest.split("version = \"").nth(1) {
            if let Some(v) = after.split('"').next() {
                pins.push((name.to_string(), v.to_string()));
            }
        }
    }
    pins
}

#[test]
fn published_members_match_the_root_version() {
    let root = package_version(&workspace().join("Cargo.toml"));
    let mut drift = Vec::new();

    for member in PUBLISHED_MEMBERS {
        let got = package_version(&manifest_of(member));
        if got != root {
            drift.push(format!(
                "  crates/{member}/Cargo.toml: {got}  (want {root})"
            ));
        }
    }

    assert!(
        drift.is_empty(),
        "workspace members have drifted from the root version {root}.\n\
         `cargo publish` will abort with \"already exists on crates.io \
         index\" on the first of these and the release will publish \
         nothing:\n{}\n\nBump each to {root}, refresh Cargo.lock, and \
         re-run.",
        drift.join("\n")
    );
}

#[test]
fn ssg_wasm_matches_the_root_version_even_though_it_is_unpublished() {
    // `publish = false` keeps it off crates.io, but leaving it behind
    // makes `cargo metadata` output and release notes inconsistent,
    // and it is the member most likely to be forgotten precisely
    // because the publish step never touches it.
    let root = package_version(&workspace().join("Cargo.toml"));
    let got = package_version(&manifest_of("ssg-wasm"));
    assert_eq!(
        got, root,
        "crates/ssg-wasm/Cargo.toml is at {got}, root is at {root}"
    );
}

#[test]
fn internal_dependency_pins_match_the_root_version() {
    let root = package_version(&workspace().join("Cargo.toml"));
    let mut stale = Vec::new();

    let mut manifests = vec![workspace().join("Cargo.toml")];
    manifests.extend(PUBLISHED_MEMBERS.iter().map(|m| manifest_of(m)));
    manifests.push(manifest_of("ssg-wasm"));

    for manifest in &manifests {
        for (dep, pinned) in internal_pins(manifest) {
            if pinned != root {
                stale.push(format!(
                    "  {}: {dep} pinned at {pinned}  (want {root})",
                    manifest
                        .strip_prefix(workspace())
                        .unwrap_or(manifest)
                        .display()
                ));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "internal dependency pins have drifted from the root version \
         {root}.\ncrates.io resolves these against the registry, so a \
         stale pin either publishes against an old sub-crate or fails \
         outright:\n{}",
        stale.join("\n")
    );
}

#[test]
fn published_members_list_matches_the_release_workflow() {
    // The list above only protects what `release.yml` actually
    // publishes. If a member is added to the workflow loop without
    // being added here, it silently loses its version gate.
    let workflow = read(&workspace().join(".github/workflows/release.yml"));
    let loop_line = workflow
        .lines()
        .find(|l| l.trim_start().starts_with("for c in ssg-"))
        .expect("release.yml no longer has the `for c in ssg-…` publish loop");

    let in_workflow: Vec<&str> = loop_line
        .trim()
        .trim_start_matches("for c in ")
        .trim_end_matches("; do")
        .split_whitespace()
        .collect();

    assert_eq!(
        in_workflow, PUBLISHED_MEMBERS,
        "release.yml publishes a different set of crates than this \
         test guards.\nrelease.yml: {in_workflow:?}\nthis test:  \
         {PUBLISHED_MEMBERS:?}"
    );
}

/// Packaging manifests that pin a version must match `Cargo.toml`.
///
/// Scoop, `WinGet` and the AUR `PKGBUILD` each hard-code the version.
/// All three sat at `0.0.37` while the crate was at `0.0.58` — twenty-one
/// releases of drift, discovered by reading them rather than by any
/// gate, because nothing compared them to anything.
///
/// The Homebrew formula is deliberately absent from this check: it
/// resolves `releases/latest` rather than pinning, so it cannot go
/// stale. A manifest that does not state a version has nothing to
/// verify.
#[test]
fn packaging_manifests_match_the_crate_version() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let version = env!("CARGO_PKG_VERSION");

    // (path, what a version line looks like there)
    let manifests: &[(&str, &str)] = &[
        ("packaging/scoop/ssg.json", "\"version\""),
        ("packaging/winget/ssg.yaml", "PackageVersion"),
        ("packaging/arch/PKGBUILD", "pkgver"),
    ];

    let mut checked = 0_usize;
    let mut stale = Vec::new();

    for (rel, key) in manifests {
        let path = root.join(rel);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {rel}: {e}"));

        let line = text
            .lines()
            .find(|l| l.contains(key))
            .unwrap_or_else(|| panic!("{rel} has no line containing {key}"));

        checked += 1;
        if !line.contains(version) {
            stale.push(format!("{rel}: {}", line.trim()));
        }
    }

    assert_eq!(checked, manifests.len(), "not every manifest was examined");
    assert!(
        stale.is_empty(),
        "these packaging manifests pin a version other than {version}:\n  \
         {}\n\nA stale manifest ships the wrong binary to whoever installs \
         through that channel.",
        stale.join("\n  ")
    );
}

/// `SECURITY.md`'s supported-version table must name the current
/// release.
///
/// The table named `< 0.0.30` as the unsupported floor while the crate
/// was at `0.0.58` — a floor that had not moved in twenty-eight
/// releases, and which contradicted the sentence directly beneath it
/// stating that only the latest release is supported. A security policy
/// that disagrees with itself is worse than a terse one: a reader
/// deciding whether to upgrade cannot tell which half to believe.
#[test]
fn security_policy_names_the_current_version() {
    let version = env!("CARGO_PKG_VERSION");
    let text =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/SECURITY.md"))
            .expect("read SECURITY.md");

    let table: String = text
        .lines()
        .skip_while(|l| !l.contains("Supported Versions"))
        .take_while(|l| !l.starts_with("`0.0.x`"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        table.contains('|'),
        "could not locate the supported-versions table in SECURITY.md"
    );
    assert!(
        table.contains(version),
        "SECURITY.md's supported-versions table does not name {version}. \
         It reads:\n{table}\n\nA reader deciding whether to upgrade needs \
         this to be current."
    );
}

/// Every `ssg = "0.0.x"` snippet in current docs must name this release.
///
/// `docs/guide/installation.md` told readers to depend on `0.0.37` while
/// the crate was at `0.0.58` — twenty-one releases stale, on the page
/// whose entire job is telling people what to install. The README's copy
/// of the same snippet was current, which is how it went unnoticed: the
/// version gates all pointed at manifests and the README.
///
/// `CHANGELOG.md` is excluded: its version strings are history.
#[test]
fn dependency_snippets_in_docs_name_the_current_version() {
    let version = env!("CARGO_PKG_VERSION");
    let root = workspace();

    let out = std::process::Command::new("git")
        .args(["ls-files", "*.md"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files failed");

    let mut stale = Vec::new();
    for rel in String::from_utf8_lossy(&out.stdout).lines() {
        if rel.ends_with("CHANGELOG.md") {
            continue;
        }
        let Ok(text) = fs::read_to_string(root.join(rel)) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("ssg = \"") else {
                continue;
            };
            let claimed = rest.trim_end_matches('"');
            if claimed != version {
                stale.push(format!(
                    "{rel}:{}: `ssg = \"{claimed}\"` (current: {version})",
                    n + 1
                ));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "documentation tells readers to depend on a stale version:\n  {}",
        stale.join("\n  ")
    );
}

/// crates.io rejects a publish whose keywords break its limits, and the
/// publish step is fail-fast: one bad keyword stops the whole release
/// after the earlier crates have already gone out, which cannot be undone.
///
/// `ssg-mcp` shipped `"static-site-generator"` (21 characters). crates.io
/// answered `400 Bad Request: "static-site-generator" is an invalid
/// keyword (keywords must have less than 20 characters)` during the
/// v0.0.60 bootstrap, after five sibling crates had already published.
///
/// The bound here is 19 rather than 20 because that is what the server's
/// own message states; every keyword in the workspace is well inside it.
#[test]
fn every_publishable_crate_has_keywords_crates_io_will_accept() {
    const MAX_KEYWORDS: usize = 5;
    const MAX_KEYWORD_LEN: usize = 19;

    let mut manifests = vec![workspace().join("Cargo.toml")];
    let crates_dir = workspace().join("crates");
    let mut members: Vec<_> = fs::read_dir(&crates_dir)
        .expect("crates/ is readable")
        .filter_map(Result::ok)
        .map(|e| e.path().join("Cargo.toml"))
        .filter(|p| p.is_file())
        .collect();
    members.sort();
    manifests.extend(members);

    let mut checked = 0usize;
    for manifest in manifests {
        let text = read(&manifest);
        // Unpublished members cannot fail a crates.io publish.
        if text.contains("publish = false") {
            continue;
        }
        let Some(line) = text.lines().find(|l| l.starts_with("keywords"))
        else {
            continue;
        };
        let keywords: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        assert!(
            !keywords.is_empty(),
            "{}: a `keywords` line with no keywords",
            manifest.display()
        );
        assert!(
            keywords.len() <= MAX_KEYWORDS,
            "{}: {} keywords; crates.io accepts at most {MAX_KEYWORDS}",
            manifest.display(),
            keywords.len()
        );
        for kw in &keywords {
            assert!(
                kw.len() <= MAX_KEYWORD_LEN,
                "{}: keyword {kw:?} is {} characters; crates.io rejects \
                 anything longer than {MAX_KEYWORD_LEN}, and that failure \
                 lands mid-publish after earlier crates are already live",
                manifest.display(),
                kw.len()
            );
        }
        checked += 1;
    }

    // A test that checked nothing would pass just as quietly as the bug.
    assert!(
        checked >= 6,
        "expected to check the root crate and its publishable members, \
         but only inspected {checked} manifest(s)"
    );
}
