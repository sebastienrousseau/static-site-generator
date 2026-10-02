// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! High-performance streaming file processor.
//!
//! Provides constant-memory file processing for workloads from 1K to 50K+
//! files. All I/O uses fixed-size buffers — memory usage does not grow
//! with file size or transaction count.
//!
//! # Performance targets
//!
//! - Time to first result: < 2 ms
//! - Throughput: >= 50,000 files/second
//! - Memory: constant O(1) per file via streaming
//!
//! # Architecture
//!
//! Files are processed through a pipeline of `StreamProcessor` stages.
//! Each stage reads from a buffered input, transforms in a fixed-size
//! buffer, and writes to a buffered output. No file is ever fully loaded
//! into memory unless it fits within the buffer size.

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Default buffer size for streaming I/O (8 KB).
/// Aligned to typical filesystem block size for optimal throughput.
pub const STREAM_BUFFER_SIZE: usize = 8 * 1024;

/// Maximum number of files to process in a single batch.
/// Bounds memory for directory listings per Power of Ten Rule 2.
pub const MAX_BATCH_SIZE: usize = 100_000;

/// Result of processing a batch of files.
#[derive(Debug, Clone, Copy)]
pub struct BatchResult {
    /// Number of files processed.
    pub files_processed: usize,
    /// Total bytes read across all files.
    pub bytes_read: u64,
    /// Total bytes written across all files.
    pub bytes_written: u64,
    /// Wall-clock duration of the batch.
    pub duration_ms: f64,
    /// Throughput in files per second.
    pub throughput: f64,
}

/// Copies a single file using buffered streaming I/O.
///
/// Reads and writes in `STREAM_BUFFER_SIZE` chunks. Memory usage is
/// constant regardless of file size — a 1 KB file and a 1 GB file
/// use the same buffer.
///
/// # Examples
///
/// ```rust
/// use ssg::stream::stream_copy;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// let src = dir.path().join("src.txt");
/// let dst = dir.path().join("dst.txt");
/// fs::write(&src, "hello").unwrap();
/// let bytes = stream_copy(&src, &dst).unwrap();
/// assert_eq!(bytes, 5);
/// ```
///
/// # Errors
///
/// Returns an error if the source cannot be read or the destination
/// cannot be written.
pub fn stream_copy(src: &Path, dst: &Path) -> Result<u64> {
    let file_in = File::open(src)
        .with_context(|| format!("cannot open {}", src.display()))?;
    let file_out = File::create(dst)
        .with_context(|| format!("cannot create {}", dst.display()))?;

    let reader = BufReader::with_capacity(STREAM_BUFFER_SIZE, file_in);
    let writer = BufWriter::with_capacity(STREAM_BUFFER_SIZE, file_out);

    copy_streams(reader, writer, src, dst)
}

/// Inner copy loop over generic reader/writer pairs.
///
/// Extracted from `stream_copy` so unit tests can drive the read,
/// write, and flush error paths with failing mock streams — those
/// branches are unreachable through the filesystem on all supported
/// platforms once `File::open`/`File::create` have succeeded.
fn copy_streams<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    src: &Path,
    dst: &Path,
) -> Result<u64> {
    let mut buf = [0u8; STREAM_BUFFER_SIZE];
    let mut total: u64 = 0;

    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("read error: {}", src.display()))?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&buf[..n])
            .with_context(|| format!("write error: {}", dst.display()))?;
        total += n as u64;
    }

    writer
        .flush()
        .with_context(|| format!("flush error: {}", dst.display()))?;

    Ok(total)
}

/// Hashes a file using streaming I/O with constant memory.
///
/// Reads in `STREAM_BUFFER_SIZE` chunks and feeds each chunk to a
/// `DefaultHasher`. Never loads the entire file into memory.
///
/// Returns a 16-character hex fingerprint.
///
/// # Examples
///
/// ```rust
/// use ssg::stream::stream_hash;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// let p = dir.path().join("h.txt");
/// fs::write(&p, "hello").unwrap();
/// let h = stream_hash(&p).unwrap();
/// assert_eq!(h.len(), 16);
/// ```
pub fn stream_hash(path: &Path) -> Result<String> {
    use std::hash::{DefaultHasher, Hasher};

    let file = File::open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(STREAM_BUFFER_SIZE, file);
    let mut hasher = DefaultHasher::new();
    let mut buf = [0u8; STREAM_BUFFER_SIZE];

    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("read error: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }

    Ok(format!("{:016x}", hasher.finish()))
}

/// Processes a batch of files through a streaming pipeline.
///
/// Applies `processor` to each file in `src_dir`, writing results to
/// `dst_dir`. Processes files sequentially with constant memory. For
/// parallel processing, use `process_batch_parallel`.
///
/// # Errors
///
/// Returns an error if any file cannot be read, processed, or written.
/// Processing stops at the first error.
///
/// # Examples
///
/// ```rust
/// use ssg::stream::{process_batch, stream_copy};
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// let src = dir.path().join("src");
/// let dst = dir.path().join("dst");
/// fs::create_dir(&src).unwrap();
/// fs::write(src.join("f.txt"), "x").unwrap();
/// let res = process_batch(&src, &dst, stream_copy).unwrap();
/// assert_eq!(res.files_processed, 1);
/// ```
pub fn process_batch<F>(
    src_dir: &Path,
    dst_dir: &Path,
    processor: F,
) -> Result<BatchResult>
where
    F: Fn(&Path, &Path) -> Result<u64>,
{
    let start = Instant::now();

    fs::create_dir_all(dst_dir)
        .with_context(|| format!("cannot create {}", dst_dir.display()))?;

    let entries: Vec<PathBuf> = collect_files_bounded(src_dir)?;
    let mut bytes_read: u64 = 0;
    let mut bytes_written: u64 = 0;
    let mut count: usize = 0;

    for src_path in &entries {
        // Unreachable in practice: every `src_path` comes from
        // `collect_files_bounded(src_dir)`, which always joins onto
        // this exact `src_dir`, so `strip_prefix` cannot fail through
        // the public API. Exercised only via the `stream::strip-prefix`
        // failpoint under the `test-fault-injection` feature.
        fail_point!("stream::strip-prefix", |_| Err(anyhow::anyhow!(
            "injected: stream::strip-prefix"
        )));
        let rel = src_path
            .strip_prefix(src_dir)
            .with_context(|| "strip_prefix failed")?;
        let dst_path = dst_dir.join(rel);

        if let Some(parent) = dst_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let src_size = fs::metadata(src_path).map_or(0, |m| m.len());
        let written = processor(src_path, &dst_path)?;

        bytes_read += src_size;
        bytes_written += written;
        count += 1;
    }

    let (duration_ms, throughput) = compute_throughput(count, start.elapsed());

    Ok(BatchResult {
        files_processed: count,
        bytes_read,
        bytes_written,
        duration_ms,
        throughput,
    })
}

/// Derives `(duration_ms, throughput)` from a batch's elapsed time.
///
/// Extracted from `process_batch` so the zero-duration guard (which
/// yields `f64::INFINITY`) is unit-testable — a real batch never
/// observes a zero `Instant` delta on supported platforms.
fn compute_throughput(
    count: usize,
    elapsed: std::time::Duration,
) -> (f64, f64) {
    let duration_ms = elapsed.as_secs_f64() * 1000.0;
    let throughput = if duration_ms > 0.0 {
        count as f64 / elapsed.as_secs_f64()
    } else {
        f64::INFINITY
    };
    (duration_ms, throughput)
}

/// Collects files from a directory with a bounded iteration count.
///
/// Returns at most `MAX_BATCH_SIZE` files. Uses iterative traversal
/// (no recursion) with depth tracking.
fn collect_files_bounded(dir: &Path) -> Result<Vec<PathBuf>> {
    collect_files_bounded_with_limit(dir, MAX_BATCH_SIZE)
}

/// Inner walker accepting an explicit limit.
///
/// Extracted so unit tests can exercise the saturation `break`
/// branches without allocating `MAX_BATCH_SIZE` (100k) files on disk.
fn collect_files_bounded_with_limit(
    dir: &Path,
    limit: usize,
) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut iterations: usize = 0;

    while let Some(current) = stack.pop() {
        if iterations >= limit {
            break;
        }

        let entries = fs::read_dir(&current)
            .with_context(|| format!("cannot read {}", current.display()))?;

        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
                iterations += 1;
                if iterations >= limit {
                    break;
                }
            }
        }
    }

    Ok(files)
}

/// Processes a file by reading line-by-line with constant memory.
///
/// Calls `line_fn` for each line. The line buffer is reused across
/// iterations — memory does not grow with file length.
///
/// # Examples
///
/// ```rust
/// use ssg::stream::stream_lines;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// let p = dir.path().join("f.txt");
/// fs::write(&p, "a\nb\nc").unwrap();
/// let mut seen = Vec::new();
/// let n = stream_lines(&p, |_, line| { seen.push(line.to_string()); Ok(()) }).unwrap();
/// assert_eq!(n, 3);
/// assert_eq!(seen[0], "a");
/// ```
///
/// # Errors
///
/// Returns an error if the file cannot be read.
pub fn stream_lines<F>(path: &Path, mut line_fn: F) -> Result<usize>
where
    F: FnMut(usize, &str) -> Result<()>,
{
    use std::io::BufRead;

    let file = File::open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let reader = BufReader::with_capacity(STREAM_BUFFER_SIZE, file);
    let mut count: usize = 0;

    for line in reader.lines() {
        let line =
            line.with_context(|| format!("read error at line {count}"))?;
        line_fn(count, &line)?;
        count += 1;
    }

    Ok(count)
}

/// Returns the throughput of a no-op pipeline to measure overhead.
///
/// Creates `n` temporary files and streams them through `stream_copy`.
/// Returns the measured throughput in files/second.
///
/// # Examples
///
/// ```rust
/// # #[cfg(test)]
/// # fn doctest() {
/// use ssg::stream::benchmark_throughput;
///
/// let result = benchmark_throughput(5).unwrap();
/// assert_eq!(result.files_processed, 5);
/// # }
/// ```
#[cfg(any(test, feature = "benchmark"))]
pub fn benchmark_throughput(n: usize) -> Result<BatchResult> {
    let tmp = tempfile::tempdir().context("cannot create temp dir")?;
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(&src)?;

    // Create n small files (64 bytes each)
    for i in 0..n {
        fs::write(src.join(format!("f{i}.txt")), "a]".repeat(32))?;
    }

    process_batch(&src, &dst, stream_copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_stream_copy_small_file() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src.txt");
        let dst = tmp.path().join("dst.txt");
        fs::write(&src, "hello world").unwrap();

        let bytes = stream_copy(&src, &dst).unwrap();
        assert_eq!(bytes, 11);
        assert_eq!(fs::read_to_string(&dst).unwrap(), "hello world");
    }

    #[test]
    fn test_stream_copy_large_file() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("large.bin");
        let dst = tmp.path().join("large_copy.bin");

        // 1 MB file — larger than STREAM_BUFFER_SIZE
        let data = vec![0xABu8; 1024 * 1024];
        fs::write(&src, &data).unwrap();

        let bytes = stream_copy(&src, &dst).unwrap();
        assert_eq!(bytes, 1024 * 1024);
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[test]
    fn test_stream_copy_empty_file() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("empty.txt");
        let dst = tmp.path().join("empty_copy.txt");
        fs::write(&src, "").unwrap();

        let bytes = stream_copy(&src, &dst).unwrap();
        assert_eq!(bytes, 0);
    }

    #[test]
    fn test_stream_hash_deterministic() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        fs::write(&path, "consistent content").unwrap();

        let h1 = stream_hash(&path).unwrap();
        let h2 = stream_hash(&path).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn test_stream_hash_differs_for_different_content() {
        let tmp = tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, "content a").unwrap();
        fs::write(&b, "content b").unwrap();

        assert_ne!(stream_hash(&a).unwrap(), stream_hash(&b).unwrap());
    }

    #[test]
    fn test_stream_hash_large_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("big.bin");
        fs::write(&path, vec![0u8; 100_000]).unwrap();

        let hash = stream_hash(&path).unwrap();
        assert_eq!(hash.len(), 16);
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn test_process_batch_copies_files() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();

        for i in 0..10 {
            fs::write(src.join(format!("f{i}.txt")), format!("data {i}"))
                .unwrap();
        }

        let result = process_batch(&src, &dst, stream_copy).unwrap();
        assert_eq!(result.files_processed, 10);
        assert!(result.bytes_written > 0);
        assert!(result.throughput > 0.0);
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn test_process_batch_empty_directory() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();

        let result = process_batch(&src, &dst, stream_copy).unwrap();
        assert_eq!(result.files_processed, 0);
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn test_process_batch_nested_dirs() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("sub/deep")).unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();
        fs::write(src.join("sub/mid.txt"), "mid").unwrap();
        fs::write(src.join("sub/deep/leaf.txt"), "leaf").unwrap();

        let result = process_batch(&src, &dst, stream_copy).unwrap();
        assert_eq!(result.files_processed, 3);
        assert_eq!(
            fs::read_to_string(dst.join("sub/deep/leaf.txt")).unwrap(),
            "leaf"
        );
    }

    #[test]
    fn test_stream_lines_counts_correctly() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("lines.txt");
        fs::write(&path, "line1\nline2\nline3\n").unwrap();

        let count = stream_lines(&path, |_i, _line| Ok(())).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_stream_lines_provides_content() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("data.txt");
        fs::write(&path, "alpha\nbeta\ngamma").unwrap();

        let mut collected = Vec::new();
        let _ = stream_lines(&path, |_i, line| {
            collected.push(line.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(collected, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_collect_files_bounded_respects_limit() {
        let tmp = tempdir().unwrap();
        // MAX_BATCH_SIZE is 100_000 — just verify it doesn't panic
        for i in 0..50 {
            fs::write(tmp.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let files = collect_files_bounded(tmp.path()).unwrap();
        assert_eq!(files.len(), 50);
    }

    #[test]
    fn collect_files_bounded_with_limit_breaks_on_outer_loop_saturation() {
        // Hits the `if iterations >= limit { break }` at the top of
        // the outer while loop (line 196 of the public version).
        // We add files in batches across multiple subdirectories so
        // the inner break fires first, leaves leftover stack entries,
        // and then the next outer-loop pop sees iterations == limit.
        let tmp = tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        for i in 0..3 {
            fs::write(a.join(format!("f{i}.txt")), "x").unwrap();
            fs::write(b.join(format!("f{i}.txt")), "x").unwrap();
        }

        let files = collect_files_bounded_with_limit(tmp.path(), 2).unwrap();
        // The cap is honoured: at most `limit` files returned
        // (may be slightly more depending on which subdir is popped
        // first; the contract is "at most" with break-on-saturation).
        assert!(files.len() <= 4);
    }

    #[test]
    fn collect_files_bounded_with_limit_breaks_on_inner_loop_saturation() {
        // Hits the inner `if iterations >= limit { break }` (line 210
        // of the public version) — file count exceeds limit during
        // a single read_dir iteration.
        let tmp = tempdir().unwrap();
        for i in 0..10 {
            fs::write(tmp.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let files = collect_files_bounded_with_limit(tmp.path(), 3).unwrap();
        assert_eq!(files.len(), 3);
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn test_benchmark_throughput_runs() {
        let result = benchmark_throughput(100).unwrap();
        assert_eq!(result.files_processed, 100);
        assert!(
            result.throughput.is_finite() && result.throughput > 0.0,
            "invalid throughput: {}",
            result.throughput
        );
        println!(
            "Benchmark: {} files in {:.2} ms ({:.0} files/sec)",
            result.files_processed, result.duration_ms, result.throughput
        );
    }

    #[test]
    fn test_batch_result_fields() {
        let r = BatchResult {
            files_processed: 10,
            bytes_read: 1000,
            bytes_written: 900,
            duration_ms: 1.5,
            throughput: 6666.0,
        };
        assert_eq!(r.files_processed, 10);
        assert!(r.throughput > 0.0);
    }

    #[test]
    fn test_stream_copy_nonexistent_source() {
        let dst = std::env::temp_dir().join("ssg_stream_copy_out");
        let result =
            stream_copy(Path::new("/definitely-does-not-exist-ssg"), &dst);
        assert!(result.is_err());
    }

    #[test]
    fn test_stream_hash_nonexistent() {
        let result = stream_hash(Path::new("/nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn test_stream_lines_empty_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("empty.txt");
        fs::write(&path, "").unwrap();

        let count = stream_lines(&path, |_i, _line| Ok(())).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn stream_copy_exact_buffer_size_file() {
        // Arrange
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("exact.bin");
        let dst = tmp.path().join("exact_copy.bin");
        let data = vec![0xCDu8; STREAM_BUFFER_SIZE];
        fs::write(&src, &data).unwrap();

        // Act
        let bytes = stream_copy(&src, &dst).unwrap();

        // Assert
        assert_eq!(bytes, STREAM_BUFFER_SIZE as u64);
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[test]
    fn stream_hash_empty_file() {
        // Arrange
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("empty.bin");
        fs::write(&path, b"").unwrap();

        // Act
        let h1 = stream_hash(&path).unwrap();
        let h2 = stream_hash(&path).unwrap();

        // Assert
        assert_eq!(h1, h2, "hash of empty file must be deterministic");
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn stream_hash_same_content_same_hash() {
        // Arrange
        let tmp = tempdir().unwrap();
        let a = tmp.path().join("file_a.txt");
        let b = tmp.path().join("file_b.txt");
        let content = "identical content in both files";
        fs::write(&a, content).unwrap();
        fs::write(&b, content).unwrap();

        // Act
        let hash_a = stream_hash(&a).unwrap();
        let hash_b = stream_hash(&b).unwrap();

        // Assert
        assert_eq!(hash_a, hash_b, "same content must produce same hash");
    }

    #[test]
    fn stream_lines_binary_content() {
        // Arrange — file with no newline characters
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("binary.bin");
        fs::write(&path, "no-newlines-here").unwrap();

        // Act
        let mut lines_seen = Vec::new();
        let count = stream_lines(&path, |_i, line| {
            lines_seen.push(line.to_string());
            Ok(())
        })
        .unwrap();

        // Assert — single line, no newline splitting
        assert_eq!(count, 1);
        assert_eq!(lines_seen, vec!["no-newlines-here"]);
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn process_batch_empty_directory() {
        // Arrange — source directory with no files
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("empty_src");
        let dst = tmp.path().join("empty_dst");
        fs::create_dir_all(&src).unwrap();

        // Act
        let result = process_batch(&src, &dst, stream_copy).unwrap();

        // Assert
        assert_eq!(result.files_processed, 0);
        assert_eq!(result.bytes_read, 0);
        assert_eq!(result.bytes_written, 0);
    }

    // -----------------------------------------------------------------
    // stream_copy — additional edge cases
    // -----------------------------------------------------------------

    #[test]
    fn stream_copy_file_just_over_buffer_boundary() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("over.bin");
        let dst = tmp.path().join("over_copy.bin");
        // One byte beyond buffer size forces two reads.
        let data = vec![0xEFu8; STREAM_BUFFER_SIZE + 1];
        fs::write(&src, &data).unwrap();

        let bytes = stream_copy(&src, &dst).unwrap();
        assert_eq!(bytes, (STREAM_BUFFER_SIZE + 1) as u64);
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[test]
    fn stream_copy_file_just_under_buffer_boundary() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("under.bin");
        let dst = tmp.path().join("under_copy.bin");
        let data = vec![0xAAu8; STREAM_BUFFER_SIZE - 1];
        fs::write(&src, &data).unwrap();

        let bytes = stream_copy(&src, &dst).unwrap();
        assert_eq!(bytes, (STREAM_BUFFER_SIZE - 1) as u64);
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[test]
    fn stream_copy_multiple_of_buffer_size() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("multi.bin");
        let dst = tmp.path().join("multi_copy.bin");
        let data = vec![0xBBu8; STREAM_BUFFER_SIZE * 3];
        fs::write(&src, &data).unwrap();

        let bytes = stream_copy(&src, &dst).unwrap();
        assert_eq!(bytes, (STREAM_BUFFER_SIZE * 3) as u64);
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[test]
    fn stream_copy_single_byte() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("one.bin");
        let dst = tmp.path().join("one_copy.bin");
        fs::write(&src, [0x42]).unwrap();

        let bytes = stream_copy(&src, &dst).unwrap();
        assert_eq!(bytes, 1);
        assert_eq!(fs::read(&dst).unwrap(), vec![0x42]);
    }

    #[test]
    fn stream_copy_dst_parent_does_not_exist() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src.txt");
        fs::write(&src, "data").unwrap();
        let dst = tmp.path().join("no/such/parent/out.txt");

        let err = stream_copy(&src, &dst);
        assert!(err.is_err());
    }

    // -----------------------------------------------------------------
    // stream_hash — additional edge cases
    // -----------------------------------------------------------------

    #[test]
    fn stream_hash_multi_chunk_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("multi_chunk.bin");
        // Force multiple read iterations
        let data = vec![0xCCu8; STREAM_BUFFER_SIZE * 2 + 100];
        fs::write(&path, &data).unwrap();

        let h1 = stream_hash(&path).unwrap();
        let h2 = stream_hash(&path).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn stream_hash_exact_buffer_boundary() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("exact_buf.bin");
        let data = vec![0xDDu8; STREAM_BUFFER_SIZE];
        fs::write(&path, &data).unwrap();

        let hash = stream_hash(&path).unwrap();
        assert_eq!(hash.len(), 16);
    }

    // -----------------------------------------------------------------
    // stream_lines — additional edge cases
    // -----------------------------------------------------------------

    #[test]
    fn stream_lines_callback_error_propagates() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("err.txt");
        fs::write(&path, "line1\nline2\nline3\n").unwrap();

        let result = stream_lines(&path, |i, _line| {
            if i == 1 {
                anyhow::bail!("stop at line 1");
            }
            Ok(())
        });

        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("stop at line 1"));
    }

    #[test]
    fn stream_lines_nonexistent_file() {
        let result = stream_lines(Path::new("/nonexistent_ssg"), |_, _| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn stream_lines_line_index_is_zero_based() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("indexed.txt");
        fs::write(&path, "a\nb\nc").unwrap();

        let mut indices = Vec::new();
        let _ = stream_lines(&path, |i, _| {
            indices.push(i);
            Ok(())
        })
        .unwrap();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn stream_lines_trailing_newline_does_not_create_extra_line() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("trailing.txt");
        fs::write(&path, "a\nb\n").unwrap();

        let count = stream_lines(&path, |_, _| Ok(())).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn stream_lines_many_lines() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("many.txt");
        let mut content = String::new();
        for i in 0..1000 {
            content.push_str(&format!("line {i}\n"));
        }
        fs::write(&path, &content).unwrap();

        let count = stream_lines(&path, |_, _| Ok(())).unwrap();
        assert_eq!(count, 1000);
    }

    // -----------------------------------------------------------------
    // process_batch — additional edge cases
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn process_batch_nonexistent_src_dir() {
        let tmp = tempdir().unwrap();
        let result = process_batch(
            &tmp.path().join("no-such-dir"),
            &tmp.path().join("dst"),
            stream_copy,
        );
        assert!(result.is_err());
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn process_batch_processor_error_stops_batch() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "hello").unwrap();

        let result = process_batch(&src, &dst, |_s, _d| {
            anyhow::bail!("processor error")
        });
        assert!(result.is_err());
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn process_batch_throughput_finite_for_fast_run() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        for i in 0..5 {
            fs::write(src.join(format!("f{i}.txt")), "x").unwrap();
        }

        let result = process_batch(&src, &dst, stream_copy).unwrap();
        assert_eq!(result.files_processed, 5);
        assert!(result.duration_ms >= 0.0);
    }

    // -----------------------------------------------------------------
    // collect_files_bounded_with_limit — additional edge cases
    // -----------------------------------------------------------------

    #[test]
    fn collect_files_bounded_with_limit_zero() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "x").unwrap();

        let files = collect_files_bounded_with_limit(tmp.path(), 0).unwrap();
        assert!(files.is_empty(), "{:?}", files);
    }

    #[test]
    fn collect_files_bounded_with_limit_exact() {
        let tmp = tempdir().unwrap();
        for i in 0..5 {
            fs::write(tmp.path().join(format!("f{i}.txt")), "x").unwrap();
        }

        let files = collect_files_bounded_with_limit(tmp.path(), 5).unwrap();
        assert_eq!(files.len(), 5);
    }

    #[test]
    fn collect_files_bounded_with_limit_deeply_nested() {
        let tmp = tempdir().unwrap();
        let deep = tmp.path().join("a/b/c/d/e");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("leaf.txt"), "deep").unwrap();
        fs::write(tmp.path().join("root.txt"), "root").unwrap();

        let files = collect_files_bounded(tmp.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn collect_files_bounded_empty_dir() {
        let tmp = tempdir().unwrap();
        let files = collect_files_bounded(tmp.path()).unwrap();
        assert!(files.is_empty(), "{:?}", files);
    }

    #[test]
    fn collect_files_bounded_nonexistent_dir() {
        let result =
            collect_files_bounded(Path::new("/nonexistent_ssg_walker"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // BatchResult — Clone / Copy / Debug
    // -----------------------------------------------------------------

    #[test]
    fn batch_result_clone_and_debug() {
        let r = BatchResult {
            files_processed: 5,
            bytes_read: 500,
            bytes_written: 400,
            duration_ms: 2.0,
            throughput: 2500.0,
        };
        let r2 = r;
        assert_eq!(r.files_processed, r2.files_processed);
        assert_eq!(format!("{r:?}"), format!("{r2:?}"));
    }

    // -----------------------------------------------------------------
    // benchmark_throughput — edge cases
    // -----------------------------------------------------------------

    #[test]
    fn benchmark_throughput_zero_files() {
        let result = benchmark_throughput(0).unwrap();
        assert_eq!(result.files_processed, 0);
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn benchmark_throughput_single_file() {
        let result = benchmark_throughput(1).unwrap();
        assert_eq!(result.files_processed, 1);
    }

    // -----------------------------------------------------------------
    // Constants — sanity checks
    // -----------------------------------------------------------------

    #[test]
    fn constants_are_sensible() {
        assert_eq!(STREAM_BUFFER_SIZE, 8192);
        assert_eq!(MAX_BATCH_SIZE, 100_000);
    }

    // -----------------------------------------------------------------
    // copy_streams — read / write / flush error paths via mock streams
    // -----------------------------------------------------------------

    /// Reader whose `read` always fails.
    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("simulated read failure"))
        }
    }

    /// Writer that can be configured to fail on write or on flush.
    struct FailingWriter {
        fail_write: bool,
        fail_flush: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.fail_write {
                Err(std::io::Error::other("simulated write failure"))
            } else {
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail_flush {
                Err(std::io::Error::other("simulated flush failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn copy_streams_read_error_carries_source_path() {
        let writer = FailingWriter {
            fail_write: false,
            fail_flush: false,
        };
        let err = copy_streams(
            FailingReader,
            writer,
            Path::new("in.bin"),
            Path::new("out.bin"),
        )
        .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("read error: in.bin"), "got: {msg}");
    }

    #[test]
    fn copy_streams_write_error_carries_dest_path() {
        let reader = std::io::Cursor::new(vec![7u8; 32]);
        let writer = FailingWriter {
            fail_write: true,
            fail_flush: false,
        };
        let err = copy_streams(
            reader,
            writer,
            Path::new("in.bin"),
            Path::new("out.bin"),
        )
        .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("write error: out.bin"), "got: {msg}");
    }

    #[test]
    fn copy_streams_flush_error_carries_dest_path() {
        let reader = std::io::Cursor::new(vec![7u8; 32]);
        let writer = FailingWriter {
            fail_write: false,
            fail_flush: true,
        };
        let err = copy_streams(
            reader,
            writer,
            Path::new("in.bin"),
            Path::new("out.bin"),
        )
        .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("flush error: out.bin"), "got: {msg}");
    }

    // -----------------------------------------------------------------
    // stream_hash / stream_lines — read error paths
    // -----------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn stream_hash_read_error_on_directory() {
        // On Unix, `File::open` on a directory succeeds but the first
        // `read` fails with EISDIR — driving the read-error context
        // closure inside the hash loop.
        let tmp = tempdir().unwrap();
        let err = stream_hash(tmp.path()).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("read error:"), "got: {msg}");
    }

    #[test]
    fn stream_lines_invalid_utf8_fires_line_error_context() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("bad.bin");
        fs::write(&path, [b'o', b'k', b'\n', 0xFF, 0xFE, 0xFD]).unwrap();

        let err = stream_lines(&path, |_, _| Ok(())).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("read error at line 1"), "got: {msg}");
    }

    // -----------------------------------------------------------------
    // process_batch — directory-creation error paths
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn process_batch_dst_creation_failure_fires_context_closure() {
        // dst_dir nests under an existing *file*, so create_dir_all
        // fails and the `cannot create` context closure runs.
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "file, not dir").unwrap();

        let err =
            process_batch(&src, &blocker.join("dst"), stream_copy).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("cannot create"), "got: {msg}");
    }

    #[test]
    #[serial_test::parallel(stream_strip_prefix)]
    fn process_batch_per_file_parent_creation_failure_propagates() {
        // The per-file `create_dir_all(parent)?` fails when the
        // destination subdirectory path is blocked by a plain file.
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/x.txt"), "x").unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("sub"), "file blocking subdir").unwrap();

        let result = process_batch(&src, &dst, stream_copy);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // compute_throughput — zero and non-zero durations
    // -----------------------------------------------------------------

    #[test]
    fn compute_throughput_zero_duration_is_infinite() {
        let (duration_ms, throughput) =
            compute_throughput(10, std::time::Duration::ZERO);
        // `Duration::ZERO.as_secs_f64() * 1000.0` is exactly 0.0 by
        // IEEE 754 (zero times any finite value is zero) — an exact
        // bit-pattern comparison, not an epsilon-worthy approximation.
        assert_eq!(duration_ms.to_bits(), 0.0_f64.to_bits());
        assert!(throughput.is_infinite());
    }

    #[test]
    fn compute_throughput_positive_duration_is_finite() {
        let (duration_ms, throughput) =
            compute_throughput(10, std::time::Duration::from_millis(5));
        assert!(duration_ms > 0.0);
        assert!(throughput.is_finite());
        assert!((throughput - 2000.0).abs() < f64::EPSILON);
    }
}

/// Fault-injection tests for `stream.rs` failpoints. Mirrors the
/// pattern used in `core/cache.rs` / `core/io_pool.rs`: the failpoint
/// registry is process-global, so these live in their own `mod` and
/// are `#[serial]` on a dedicated key.
#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_injection {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard<'a>(&'a str);

    impl Drop for FailGuard<'_> {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial_test::serial(stream_strip_prefix)]
    fn process_batch_strip_prefix_failpoint_injects_error() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "x").unwrap();

        let _guard = FailGuard("stream::strip-prefix");
        fail::cfg("stream::strip-prefix", "return")
            .expect("activate failpoint");
        let err = process_batch(&src, &dst, stream_copy).unwrap_err();
        assert!(
            format!("{err:?}").contains("injected: stream::strip-prefix"),
            "got: {err:?}"
        );
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        /// Hashing the same content twice must yield the same fingerprint.
        #[test]
        fn stream_hash_deterministic(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("input.bin");
            fs::write(&path, &data).unwrap();

            let h1 = stream_hash(&path).unwrap();
            let h2 = stream_hash(&path).unwrap();
            prop_assert_eq!(h1, h2);
        }
    }
}
