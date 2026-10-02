// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::walk`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;

use ssg::walk::{
    walk_files, walk_files_bounded_count, walk_files_bounded_depth,
    walk_files_multi,
};
use tempfile::tempdir;

fn seed_tree(dir: &std::path::Path) {
    fs::create_dir_all(dir.join("a/b/c")).unwrap();
    fs::write(dir.join("root.md"), "").unwrap();
    fs::write(dir.join("root.txt"), "").unwrap();
    fs::write(dir.join("a/mid.md"), "").unwrap();
    fs::write(dir.join("a/b/deep.md"), "").unwrap();
    fs::write(dir.join("a/b/c/deeper.md"), "").unwrap();
    fs::write(dir.join("photo.JPG"), "").unwrap();
}

#[test]
fn walk_files_collects_all_matching_extensions_recursively() {
    let dir = tempdir().unwrap();
    seed_tree(dir.path());
    let mut found = walk_files(dir.path(), "md").unwrap();
    found.sort();
    assert_eq!(found.len(), 4, "4 .md files exist in the seeded tree");
}

#[test]
fn walk_files_returns_empty_for_missing_dir() {
    let dir = tempdir().unwrap();
    let result = walk_files(&dir.path().join("nope"), "md").unwrap();
    assert!(result.is_empty(), "{:?}", result);
}

#[test]
fn walk_files_multi_is_case_insensitive_on_extensions() {
    let dir = tempdir().unwrap();
    seed_tree(dir.path());
    let found = walk_files_multi(dir.path(), &["jpg"]).unwrap();
    assert_eq!(
        found.len(),
        1,
        "photo.JPG must match jpg case-insensitively"
    );
}

#[test]
fn walk_files_bounded_depth_skips_subdirs_past_limit() {
    let dir = tempdir().unwrap();
    seed_tree(dir.path());
    let depth_1 = walk_files_bounded_depth(dir.path(), "md", 1).unwrap();
    assert_eq!(depth_1.len(), 2, "depth 1 sees root.md + a/mid.md");
}

#[test]
fn walk_files_bounded_count_caps_results() {
    let dir = tempdir().unwrap();
    seed_tree(dir.path());
    let capped = walk_files_bounded_count(dir.path(), "md", 2).unwrap();
    assert_eq!(capped.len(), 2);
}
