//! Validates the fuzzing assets that CI and OSS-Fuzz depend on.
//!
//! Every fuzz target ships a dictionary and a seed corpus. Both are consumed
//! by libFuzzer at runtime, neither is compiled, and a malformed one fails
//! *quietly enough to miss*: libFuzzer prints `ParseDictionaryFile: error in
//! line N` and exits 0. A nightly job would go green having fuzzed nothing.
//!
//! That is not hypothetical. Two of the four dictionaries were written with
//! `\n` escapes, which the dictionary format does not accept — only `\\`,
//! `\"`, and `\xAB`. `fuzz_frontmatter` and `fuzz_markdown` therefore ran zero
//! iterations, while the other two reported healthy coverage, so the run
//! looked fine in aggregate. These tests exist so the next such mistake fails
//! in `cargo test` rather than being discovered by hand.

// `clippy.toml`'s `allow-expect-in-tests` only reaches `#[test]` functions
// and `#[cfg(test)]` modules. This is an integration test crate, built
// without `--cfg test`, so its module-level helpers need their own
// header — as the other suites in `tests/` already carry.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// Repository root, independent of the test's working directory.
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The fuzz targets declared in `fuzz/Cargo.toml`.
///
/// Parsed from the manifest rather than hardcoded: a target added there but
/// missing from this list would otherwise go unchecked, which is the same
/// drift the OSS-Fuzz build script avoids by enumerating the same way.
fn declared_targets() -> BTreeSet<String> {
    let manifest = fs::read_to_string(root().join("fuzz/Cargo.toml"))
        .expect("fuzz/Cargo.toml is readable");
    manifest
        .lines()
        .filter_map(|line| {
            let name = line.strip_prefix("name = \"")?.strip_suffix('"')?;
            name.starts_with("fuzz_").then(|| name.to_string())
        })
        .collect()
}

#[test]
fn every_declared_target_has_a_source_file() {
    for target in declared_targets() {
        let path = root().join(format!("fuzz/fuzz_targets/{target}.rs"));
        assert!(
            path.is_file(),
            "{target} is declared in fuzz/Cargo.toml but {} does not exist",
            path.display()
        );
    }
}

#[test]
fn dictionaries_use_only_supported_escapes() {
    // libFuzzer's dictionary grammar accepts `\\`, `\"` and `\xAB`. Anything
    // else — `\n`, `\t`, `\0` — aborts parsing of the whole file.
    for entry in dictionary_files() {
        let text = fs::read_to_string(&entry).expect("dictionary is readable");
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    continue;
                }
                match chars.peek() {
                    Some('\\' | '"') => {
                        let _ = chars.next();
                    }
                    Some('x') => {
                        let _ = chars.next();
                        for _ in 0..2 {
                            let d = chars.next();
                            assert!(
                                d.is_some_and(|d| d.is_ascii_hexdigit()),
                                "{}:{}: `\\x` needs two hex digits: {line}",
                                entry.display(),
                                n + 1
                            );
                        }
                    }
                    other => panic!(
                        "{}:{}: unsupported escape `\\{}` — libFuzzer accepts \
                         only \\\\, \\\" and \\xAB: {line}",
                        entry.display(),
                        n + 1,
                        other.map_or_else(
                            || "<eol>".to_string(),
                            |c| c.to_string()
                        ),
                    ),
                }
            }
        }
    }
}

#[test]
fn dictionary_entries_are_quoted() {
    for entry in dictionary_files() {
        let text = fs::read_to_string(&entry).expect("dictionary is readable");
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            assert!(
                line.starts_with('"') && line.ends_with('"') && line.len() >= 2,
                "{}:{}: entry must be a quoted string: {line}",
                entry.display(),
                n + 1
            );
        }
    }
}

#[test]
fn every_target_has_a_dictionary_and_seed_corpus() {
    // Not strictly required by libFuzzer, but a structured-input fuzzer
    // without either spends its budget rediscovering that `<` starts a tag.
    for target in declared_targets() {
        let dict = root().join(format!("fuzz/dictionaries/{target}.dict"));
        assert!(
            dict.is_file(),
            "{target} has no dictionary at {}",
            dict.display()
        );

        let corpus = root().join(format!("fuzz/corpus/{target}"));
        assert!(corpus.is_dir(), "{target} has no seed corpus directory");
        let seeds = fs::read_dir(&corpus)
            .expect("corpus dir is readable")
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .count();
        assert!(seeds > 0, "{target}'s seed corpus is empty");
    }
}

#[test]
fn oss_fuzz_build_script_is_executable_and_sets_shell_flags() {
    let script = root().join("fuzz/oss-fuzz-build.sh");
    assert!(script.is_file(), "fuzz/oss-fuzz-build.sh is missing");

    let text = fs::read_to_string(&script).expect("script is readable");
    // Shebang flags are dropped when the file is run as `bash script.sh`,
    // which is how both OSS-Fuzz's wrapper and a developer invoke it. The
    // first version relied on `#!/bin/bash -eu` and reported success after
    // every target failed to build.
    assert!(
        text.contains("set -euo pipefail"),
        "the script must set its shell flags in the body, not on the shebang"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&script).expect("stat").permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "fuzz/oss-fuzz-build.sh is not executable"
        );
    }
}

#[test]
fn clusterfuzzlite_config_is_present_and_consistent() {
    let dir = root().join(".clusterfuzzlite");
    for name in ["project.yaml", "Dockerfile", "build.sh"] {
        assert!(
            dir.join(name).is_file(),
            ".clusterfuzzlite/{name} is missing"
        );
    }

    let project = fs::read_to_string(dir.join("project.yaml"))
        .expect("project.yaml is readable");
    assert!(
        project.contains("language: rust"),
        "project.yaml must declare the Rust language for the base image to match"
    );
}

/// Every `.dict` under `fuzz/dictionaries/`.
fn dictionary_files() -> Vec<PathBuf> {
    let dir = root().join("fuzz/dictionaries");
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "dict"))
        .collect();
    out.sort();
    assert!(
        !out.is_empty(),
        "no dictionaries found in {}",
        dir.display()
    );
    out
}

/// Guards the assumption the other tests rest on.
#[test]
fn fuzz_manifest_declares_targets() {
    let targets = declared_targets();
    assert!(
        !targets.is_empty(),
        "no fuzz targets parsed from fuzz/Cargo.toml — the parser or the \
         manifest format changed"
    );
}

/// Every step in `fuzz.yml` that reads `$OUT` must declare it.
///
/// `OUT` is set per step in that workflow rather than at job level, so a
/// new step that uses it and forgets the `env:` block dies with
/// `OUT: unbound variable` — after the expensive build has already run.
///
/// This is written against a real failure: the corpus-replay step added
/// for issue #566 shipped without it. The local test passed because it
/// exported `OUT` by hand, which verified the replay logic and not the
/// wiring the replay depended on.
#[test]
fn every_fuzz_step_using_out_declares_it() {
    let workflow = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/fuzz.yml"
    ))
    .expect("read fuzz.yml");

    // Split into steps on the `- name:` boundary at step indentation.
    let mut offenders = Vec::new();
    let mut current = String::new();
    let mut name = String::new();

    let check = |name: &str, body: &str, offenders: &mut Vec<String>| {
        if name.is_empty() {
            return;
        }
        // `$OUT` used outside a comment line.
        let uses_out = body.lines().any(|l| {
            let t = l.trim();
            !t.starts_with('#') && (t.contains("$OUT") || t.contains("${OUT"))
        });
        // A declaration is a non-comment line whose key is `OUT`.
        // `body.contains("OUT: ")` was the first version and it passed
        // the mutation that removed the env block — because the step's
        // own comment mentions `OUT: unbound variable`. Prose about a
        // variable is not a declaration of it.
        let declares = body.lines().any(|l| {
            let t = l.trim();
            !t.starts_with('#') && t.starts_with("OUT:")
        });
        if uses_out && !declares {
            offenders.push(name.to_owned());
        }
    };

    for line in workflow.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("- name: ") {
            check(&name, &current, &mut offenders);
            name = rest.trim().to_owned();
            current.clear();
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    check(&name, &current, &mut offenders);

    assert!(
        offenders.is_empty(),
        "these fuzz.yml steps read $OUT without declaring it, and will \
         fail with `OUT: unbound variable` after the build has already \
         run: {offenders:?}"
    );
}
