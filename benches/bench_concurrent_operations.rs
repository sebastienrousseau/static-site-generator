#![allow(
    clippy::unwrap_used,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned,
    clippy::expect_used
)]
// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![allow(missing_docs)]

//! Benchmark suite for concurrent operations in the Static Site Generator.
//!
//! This module contains performance benchmarks for concurrent file operations,
//! including copying, verification, and directory traversal.

use criterion::{criterion_group, criterion_main, Criterion};
use ssg::{copy_dir_all, verify_and_copy_files, verify_file_safety};
use std::hint::black_box;
use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
};
use tempfile::tempdir;

/// Benchmarks concurrent file copy operations with various file sizes and counts.
///
/// # Arguments
///
/// * `c` - A reference to a `Criterion` instance for benchmark configuration
///
/// # Example Output
///
/// ```text
/// concurrent copy (small files)    time: [15.234 ms 15.345 ms 15.456 ms]
/// concurrent copy (medium files)   time: [25.345 ms 25.456 ms 25.567 ms]
/// concurrent copy (large files)    time: [35.456 ms 35.567 ms 35.678 ms]
/// ```
pub fn bench_concurrent_copy(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_copy");

    // Benchmark small files (1KB each)
    let _ = group.bench_function("small_files", |b| {
        b.iter_with_setup(
            || setup_test_files(100, 1024),
            |(_tmp, src, dst)| {
                copy_dir_all(&src, &dst).unwrap();
                black_box(());
            },
        );
    });

    // Benchmark medium files (1MB each)
    let _ = group.bench_function("medium_files", |b| {
        b.iter_with_setup(
            || setup_test_files(10, 1024 * 1024),
            |(_tmp, src, dst)| {
                copy_dir_all(&src, &dst).unwrap();
                black_box(());
            },
        );
    });

    // Benchmark large files (10MB each)
    let _ = group.bench_function("large_files", |b| {
        b.iter_with_setup(
            || setup_test_files(5, 10 * 1024 * 1024),
            |(_tmp, src, dst)| {
                copy_dir_all(&src, &dst).unwrap();
                black_box(());
            },
        );
    });

    group.finish();
}

/// Benchmarks file verification operations under different scenarios.
///
/// # Arguments
///
/// * `c` - A reference to a `Criterion` instance
///
/// # Example Output
///
/// ```text
/// verify files (regular)     time: [5.123 µs 5.234 µs 5.345 µs]
/// verify files (nested)      time: [8.234 µs 8.345 µs 8.456 µs]
/// verify files (mixed)       time: [12.345 µs 12.456 µs 12.567 µs]
/// ```
pub fn bench_verify_files(c: &mut Criterion) {
    let mut group = c.benchmark_group("verify_files");

    // Benchmark regular file verification
    let _ = group.bench_function("regular_files", |b| {
        b.iter_with_setup(setup_regular_files, |(_tmp, path)| {
            verify_file_safety(&path).unwrap();
            black_box(());
        });
    });

    // Benchmark nested directory verification
    let _ = group.bench_function("nested_directories", |b| {
        b.iter_with_setup(setup_nested_directories, |(_tmp, src, dst)| {
            verify_and_copy_files(&src, &dst).unwrap();
            black_box(());
        });
    });

    // Benchmark mixed content verification
    let _ = group.bench_function("mixed_content", |b| {
        b.iter_with_setup(setup_mixed_content, |(_tmp, src, dst)| {
            verify_and_copy_files(&src, &dst).unwrap();
            black_box(());
        });
    });

    group.finish();
}

/// Helper function to set up test files for benchmarking.
///
/// Creates a specified number of files with given size in bytes.
///
/// # Arguments
///
/// * `count` - Number of files to create
/// * `size` - Size of each file in bytes
///
/// # Returns
///
/// * Tuple of (temp_dir, source `PathBuf`, destination `PathBuf`)
fn setup_test_files(
    count: u32,
    size: u64,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp_dir = tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    let dst_dir = temp_dir.path().join("dst");

    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&dst_dir).unwrap();

    // Create test files
    for i in 0..count {
        let file_path = src_dir.join(format!("file_{i}.txt"));
        let mut file = File::create(&file_path).unwrap();

        // Create file of specified size
        let data = vec![b'x'; size as usize];
        file.write_all(&data).unwrap();
    }

    // Postconditions: a fixture that silently built the wrong shape would
    // make every measurement below meaningless.
    assert!(src_dir.exists() && dst_dir.exists());
    let built: Vec<_> = fs::read_dir(&src_dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(built.len(), count as usize);
    assert_eq!(fs::metadata(built[0].path()).unwrap().len(), size);

    (temp_dir, src_dir, dst_dir)
}

/// Sets up regular files for verification benchmarking.
///
/// # Returns
///
/// * Tuple of (temp_dir, `PathBuf` to the test file)
fn setup_regular_files() -> (tempfile::TempDir, PathBuf) {
    let temp_dir = tempdir().unwrap();
    let file_path = temp_dir.path().join("test.txt");
    fs::write(&file_path, "test content").unwrap();
    (temp_dir, file_path)
}

/// Sets up nested directory structure for verification benchmarking.
///
/// # Returns
///
/// * Tuple of (temp_dir, source `PathBuf`, destination `PathBuf`)
fn setup_nested_directories() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp_dir = tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    let dst_dir = temp_dir.path().join("dst");

    // Create nested directory structure
    for i in 0..5 {
        let nested_dir = src_dir.join(format!("level_{i}"));
        fs::create_dir_all(&nested_dir).unwrap();

        for j in 0..3 {
            let file_path = nested_dir.join(format!("file_{j}.txt"));
            fs::write(file_path, format!("content {i}_{j}")).unwrap();
        }
    }

    // Postconditions, as above: five levels, three files each.
    for i in 0..5 {
        let dir = src_dir.join(format!("level_{i}"));
        assert!(dir.is_dir());
        for j in 0..3 {
            assert!(dir.join(format!("file_{j}.txt")).is_file());
        }
    }

    (temp_dir, src_dir, dst_dir)
}

/// Sets up mixed content (files and directories) for verification benchmarking.
///
/// # Returns
///
/// * Tuple of (temp_dir, source `PathBuf`, destination `PathBuf`)
fn setup_mixed_content() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp_dir = tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    let dst_dir = temp_dir.path().join("dst");

    // Create root files
    fs::create_dir_all(&src_dir).unwrap();
    for i in 0..5 {
        fs::write(
            src_dir.join(format!("root_{i}.txt")),
            format!("root content {i}"),
        )
        .unwrap();
    }

    // Create mixed nested structure
    for i in 0..3 {
        let sub_dir = src_dir.join(format!("dir_{i}"));
        fs::create_dir_all(&sub_dir).unwrap();

        // Mixed files and empty directories
        fs::write(sub_dir.join("content.txt"), format!("dir content {i}"))
            .unwrap();
        fs::create_dir_all(sub_dir.join("empty")).unwrap();
    }

    (temp_dir, src_dir, dst_dir)
}

criterion_group!(benches, bench_concurrent_copy, bench_verify_files);
criterion_main!(benches);
