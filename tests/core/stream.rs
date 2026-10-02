// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::stream`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;

use ssg::stream::{process_batch, stream_copy, stream_hash, stream_lines};
use tempfile::tempdir;

#[test]
fn stream_copy_copies_full_file_byte_for_byte() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("dst.bin");
    let payload = vec![0xABu8; 8192];
    fs::write(&src, &payload).unwrap();
    let copied = stream_copy(&src, &dst).unwrap();
    assert_eq!(copied as usize, payload.len());
    assert_eq!(fs::read(&dst).unwrap(), payload);
}

#[test]
fn process_batch_copies_files_and_records_counts() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), "one").unwrap();
    fs::write(src.join("b.txt"), "two").unwrap();

    let result = process_batch(&src, &dst, |s, d| {
        fs::copy(s, d).map_err(anyhow::Error::from)
    })
    .unwrap();
    assert_eq!(result.files_processed, 2);
    assert!(result.bytes_read >= 6);
}

#[test]
fn stream_lines_invokes_callback_for_each_line() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lines.txt");
    fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();
    let mut seen = Vec::new();
    let count = stream_lines(&path, |_n, line| {
        seen.push(line.to_string());
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 3);
    assert_eq!(seen, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn stream_hash_returns_a_non_empty_hex_string() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("h.txt");
    fs::write(&path, "hello world").unwrap();
    let h = stream_hash(&path).unwrap();
    assert!(!h.is_empty(), "should not be empty");
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
}
