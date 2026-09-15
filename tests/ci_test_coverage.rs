//! Asserts that every test target and every advertised capability is actually
//! exercised by CI.
//!
//! # Why this exists
//!
//! CI used to run integration tests by naming targets:
//!
//! ```yaml
//! cargo test --test regression --test fault_injection --test schema_validation …
//! ```
//!
//! That list named 13 of the 50 files in `tests/`. The other 29 ran nowhere —
//! among them `release_versions`, whose entire purpose is to catch a crate
//! being published without a version gate, and which failed the moment it was
//! finally executed. A hardcoded target list is a second inventory that has to
//! be maintained in lockstep with the first, and it was not.
//!
//! The same shape of problem applies to the library's capabilities: the README
//! can claim a plugin exists, and the plugin can be registered, without
//! anything proving it still behaves. So this file gates two inventories,
//! both derived from the code rather than restated:
//!
//! 1. every file in `tests/` is reachable from a workflow
//! 2. every registered plugin and every audit gate is named by some test
//!
//! Neither is a proof of correctness. They are proof that the thing is
//! *reached* — which is the part that silently stops being true.

// `clippy.toml` sets `allow-expect-in-tests`, but that only reaches code
// clippy recognises as test code: `#[test]` functions and `#[cfg(test)]`
// modules. Integration tests are separate crates compiled *without*
// `--cfg test`, so module-level helpers like `all_workflows` below are
// not covered. Every other suite in `tests/` carries this same header.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ssg::audit::{AuditConfig, AuditRunner};
use ssg::cmd::SsgConfig;
use ssg::plugin::PluginManager;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.yml` under `.github/workflows/`, concatenated.
fn all_workflows() -> String {
    let dir = root().join(".github/workflows");
    let mut out = String::new();
    for entry in fs::read_dir(&dir).expect("workflows directory is readable") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
            out.push_str(&fs::read_to_string(&path).unwrap_or_default());
            out.push('\n');
        }
    }
    assert!(!out.is_empty(), "no workflows found");
    out
}

/// Test targets Cargo will build: `tests/*.rs` plus the `[[test]]` entries
/// that point at a directory's `main.rs`.
fn test_targets() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for entry in fs::read_dir(root().join("tests")).expect("tests/ is readable")
    {
        let path = entry.expect("readable entry").path();
        if path.extension().is_some_and(|e| e == "rs") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let _ = out.insert(stem.to_string());
            }
        }
    }

    // Directory suites are declared in Cargo.toml rather than discovered.
    let manifest = fs::read_to_string(root().join("Cargo.toml"))
        .expect("Cargo.toml is readable");
    let mut in_test_block = false;
    for line in manifest.lines() {
        if line.trim() == "[[test]]" {
            in_test_block = true;
            continue;
        }
        if line.starts_with('[') {
            in_test_block = false;
        }
        if in_test_block {
            if let Some(name) = line
                .strip_prefix("name = \"")
                .and_then(|r| r.strip_suffix('"'))
            {
                let _ = out.insert(name.to_string());
            }
        }
    }
    out
}

#[test]
fn every_test_target_is_reachable_from_ci() {
    let workflows = all_workflows();

    // A blanket `--tests` run covers every target Cargo knows about, which is
    // the whole point of preferring it to an enumerated list.
    if workflows.contains("cargo test --tests") {
        return;
    }

    let unreachable: Vec<String> = test_targets()
        .into_iter()
        .filter(|t| !workflows.contains(&format!("--test {t}")))
        .collect();

    assert!(
        unreachable.is_empty(),
        "these test targets are never run by any workflow:\n  {}\n\
         Either add them to a workflow or use `cargo test --tests`.",
        unreachable.join("\n  ")
    );
}

#[test]
fn every_registered_plugin_is_named_by_a_test() {
    let config = SsgConfig::default();
    let mut plugins = PluginManager::new();
    ssg::pipeline::register_default_plugins(&mut plugins, &config, false, None);

    let corpus = all_test_sources();
    let unmentioned: Vec<&str> = plugins
        .inventory()
        .into_iter()
        .map(|p| p.name)
        .filter(|name| !corpus.contains(*name))
        .collect();

    assert!(
        unmentioned.is_empty(),
        "these plugins are registered in the pipeline but named by no test:\n  {}\n\
         A plugin nothing references can be broken without any suite noticing.",
        unmentioned.join("\n  ")
    );
}

#[test]
fn every_audit_gate_is_named_by_a_test() {
    let gates = AuditRunner::new(AuditConfig::new()).gate_names();
    let corpus = all_test_sources();

    let unmentioned: Vec<&str> =
        gates.into_iter().filter(|g| !corpus.contains(*g)).collect();

    assert!(
        unmentioned.is_empty(),
        "these audit gates are registered but named by no test:\n  {}",
        unmentioned.join("\n  ")
    );
}

#[test]
fn every_cli_subcommand_is_named_by_a_test() {
    let corpus = all_test_sources();
    let unmentioned: Vec<&str> = ssg::cmd::SUBCOMMANDS
        .iter()
        .copied()
        // `help` is clap's own; there is nothing of ours to regress.
        .filter(|s| *s != "help")
        .filter(|s| !corpus.contains(*s))
        .collect();

    assert!(
        unmentioned.is_empty(),
        "these CLI subcommands are accepted but named by no test:\n  {}",
        unmentioned.join("\n  ")
    );
}

/// Every test source in the repository, concatenated.
///
/// Used for "is this capability mentioned anywhere in the suite" checks. A
/// name appearing in a test is weak evidence — it proves reach, not
/// correctness — but its *absence* is strong evidence that nothing exercises
/// the capability at all, which is what these tests are for.
fn all_test_sources() -> String {
    let mut out = String::new();
    collect_rs(&root().join("tests"), &mut out);
    collect_rs(&root().join("src"), &mut out);
    assert!(!out.is_empty(), "no test sources found");
    out
}

fn collect_rs(dir: &Path, out: &mut String) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push_str(&fs::read_to_string(&path).unwrap_or_default());
            out.push('\n');
        }
    }
}
