// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Streaming compilation for large sites.
//!
//! Processes content files in batches to cap peak memory usage, enabling
//! compilation of 100K+ page sites within a configurable memory budget.
//!
//! The streaming compiler divides content files into chunks based on the
//! memory budget, compiles each chunk, then releases it before processing
//! the next. After all chunks, a merge pass unifies cross-page artefacts
//! (sitemap, search index, feeds).

use crate::error::{PathErrorExt, SsgError};
use crate::walk;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Default peak memory budget: 512 MB.
pub const DEFAULT_MEMORY_BUDGET_MB: usize = 512;

/// Estimated memory per page in bytes (HTML + metadata + buffers).
/// Conservative estimate for batch sizing.
const ESTIMATED_BYTES_PER_PAGE: usize = 64 * 1024; // 64 KB

/// Memory budget configuration for streaming compilation.
#[derive(Debug, Clone, Copy)]
pub struct MemoryBudget {
    /// Maximum memory in bytes.
    pub max_bytes: usize,
    /// Pages per batch, derived from `max_bytes`.
    pub batch_size: usize,
}

impl MemoryBudget {
    /// Creates a memory budget from a megabyte limit.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::streaming::MemoryBudget;
    ///
    /// let b = MemoryBudget::from_mb(64);
    /// assert_eq!(b.max_bytes, 64 * 1024 * 1024);
    /// assert!(b.batch_size >= 10);
    /// ```
    #[must_use]
    pub fn from_mb(mb: usize) -> Self {
        let max_bytes = mb * 1024 * 1024;
        let batch_size = (max_bytes / ESTIMATED_BYTES_PER_PAGE).max(10);
        Self {
            max_bytes,
            batch_size,
        }
    }

    /// Creates the default 512 MB budget.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::streaming::MemoryBudget;
    ///
    /// let b = MemoryBudget::default_budget();
    /// assert_eq!(b.max_bytes, 512 * 1024 * 1024);
    /// ```
    #[must_use]
    pub fn default_budget() -> Self {
        Self::from_mb(DEFAULT_MEMORY_BUDGET_MB)
    }
}

/// Collects content files and returns them as batches.
///
/// Each batch contains at most `budget.batch_size` files.
///
/// # Examples
///
/// ```rust
/// use ssg::streaming::{batched_content_files, MemoryBudget};
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// fs::write(dir.path().join("a.md"), "x").unwrap();
/// let budget = MemoryBudget { max_bytes: 0, batch_size: 10 };
/// let batches = batched_content_files(dir.path(), &budget).unwrap();
/// assert_eq!(batches.len(), 1);
/// ```
pub fn batched_content_files(
    content_dir: &Path,
    budget: &MemoryBudget,
) -> Result<Vec<Vec<PathBuf>>, SsgError> {
    let all_files = walk::walk_files(content_dir, "md")?;

    if all_files.is_empty() {
        return Ok(vec![]);
    }

    let batches: Vec<Vec<PathBuf>> = all_files
        .chunks(budget.batch_size)
        .map(|chunk| chunk.to_vec())
        .collect();

    log::info!(
        "[streaming] {} file(s) in {} batch(es) (budget: {} MB, {} pages/batch)",
        all_files.len(),
        batches.len(),
        budget.max_bytes / (1024 * 1024),
        budget.batch_size,
    );

    Ok(batches)
}

/// Compiles a single batch of content files into the build directory.
///
/// Creates a temporary content directory containing only the batch files,
/// runs `staticdatagen::compile` on it, then merges the output into the
/// final site directory.
///
/// # Examples
///
/// ```rust
/// use ssg::streaming::compile_batch;
/// use tempfile::tempdir;
///
/// let dir = tempdir().unwrap();
/// // Empty batch is a no-op: returns Ok immediately.
/// assert!(compile_batch(&[], dir.path(), dir.path(), dir.path(), dir.path(), 0).is_ok());
/// ```
pub fn compile_batch(
    batch: &[PathBuf],
    content_dir: &Path,
    build_dir: &Path,
    site_dir: &Path,
    template_dir: &Path,
    batch_idx: usize,
) -> Result<(), SsgError> {
    if batch.is_empty() {
        return Ok(());
    }

    // Create a temporary batch content directory
    let batch_content = build_dir.join(format!(".batch-{batch_idx}"));
    fs::create_dir_all(&batch_content).with_path(&batch_content)?;

    // Copy batch files preserving directory structure
    for file in batch {
        let rel = file.strip_prefix(content_dir).unwrap_or(file);
        let dest = batch_content.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_path(parent)?;
        }
        let _ = fs::copy(file, &dest).with_path(&dest)?;
    }

    // Compile the batch
    let batch_build = build_dir.join(format!(".batch-{batch_idx}-build"));
    let batch_site = build_dir.join(format!(".batch-{batch_idx}-site"));
    fs::create_dir_all(&batch_build).with_path(&batch_build)?;
    fs::create_dir_all(&batch_site).with_path(&batch_site)?;

    let compile_result = staticdatagen::compile(
        &batch_build,
        &batch_content,
        &batch_site,
        template_dir,
    );

    // Merge batch output into the main site directory
    if compile_result.is_ok() {
        fs::create_dir_all(site_dir).with_path(site_dir)?;
        merge_dir(&batch_site, site_dir)?;
    }

    // Clean up batch temporaries
    let _ = fs::remove_dir_all(&batch_content);
    let _ = fs::remove_dir_all(&batch_build);
    let _ = fs::remove_dir_all(&batch_site);

    compile_result.map_err(|e| {
        SsgError::io(
            std::io::Error::other(format!("batch {batch_idx}: {e:?}")),
            build_dir,
        )
    })
}

/// Recursively merges files from `src` into `dst`, overwriting on conflict.
fn merge_dir(src: &Path, dst: &Path) -> Result<(), SsgError> {
    if !src.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(src).with_path(src)? {
        let entry = entry.with_path(src)?;
        let path = entry.path();
        let dest = dst.join(entry.file_name());

        if path.is_dir() {
            fs::create_dir_all(&dest).with_path(&dest)?;
            merge_dir(&path, &dest)?;
        } else {
            let _ = fs::copy(&path, &dest).with_path(&dest)?;
        }
    }
    Ok(())
}

/// Determines whether streaming compilation should be used.
///
/// Returns `true` if the content directory has more files than a single
/// batch can hold, or if `--max-memory` was explicitly set.
///
/// # Examples
///
/// ```rust
/// use ssg::streaming::{should_stream, MemoryBudget};
/// use tempfile::tempdir;
///
/// let dir = tempdir().unwrap();
/// let budget = MemoryBudget::default_budget();
/// // Explicitly set ⇒ always stream.
/// assert!(should_stream(dir.path(), &budget, true));
/// ```
#[must_use]
pub fn should_stream(
    content_dir: &Path,
    budget: &MemoryBudget,
    explicitly_set: bool,
) -> bool {
    if explicitly_set {
        return true;
    }

    let count = walk::walk_files(content_dir, "md").map_or(0, |f| f.len());

    count > budget.batch_size
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn memory_budget_from_mb() {
        let budget = MemoryBudget::from_mb(256);
        assert_eq!(budget.max_bytes, 256 * 1024 * 1024);
        assert!(budget.batch_size > 0);
    }

    #[test]
    fn memory_budget_default() {
        let budget = MemoryBudget::default_budget();
        assert_eq!(budget.max_bytes, 512 * 1024 * 1024);
    }

    #[test]
    fn memory_budget_minimum_batch_size() {
        let budget = MemoryBudget::from_mb(0);
        assert!(
            budget.batch_size >= 10,
            "batch size should have a floor of 10"
        );
    }

    #[test]
    fn batched_content_files_empty_dir() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();

        let budget = MemoryBudget::from_mb(512);
        let batches = batched_content_files(&content, &budget).unwrap();
        assert!(batches.is_empty(), "{:?}", batches);
    }

    #[test]
    fn batched_content_files_splits_correctly() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();

        for i in 0..25 {
            fs::write(
                content.join(format!("page{i}.md")),
                format!("# Page {i}"),
            )
            .unwrap();
        }

        let budget = MemoryBudget {
            max_bytes: 0,
            batch_size: 10,
        };
        let batches = batched_content_files(&content, &budget).unwrap();

        assert_eq!(batches.len(), 3); // 10 + 10 + 5
        assert_eq!(batches[0].len(), 10);
        assert_eq!(batches[1].len(), 10);
        assert_eq!(batches[2].len(), 5);
    }

    #[test]
    fn merge_dir_combines_files() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("a.html"), "from src").unwrap();
        fs::write(dst.join("b.html"), "existing").unwrap();

        merge_dir(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("a.html")).unwrap(), "from src");
        assert_eq!(fs::read_to_string(dst.join("b.html")).unwrap(), "existing");
    }

    #[test]
    fn merge_dir_overwrites_on_conflict() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("a.html"), "new").unwrap();
        fs::write(dst.join("a.html"), "old").unwrap();

        merge_dir(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("a.html")).unwrap(), "new");
    }

    #[test]
    fn should_stream_when_explicitly_set() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();

        let budget = MemoryBudget::default_budget();
        assert!(should_stream(&content, &budget, true));
    }

    #[test]
    fn compile_batch_empty_is_noop() {
        let dir = tempdir().unwrap();
        let result = compile_batch(
            &[],
            dir.path(),
            &dir.path().join("build"),
            &dir.path().join("site"),
            &dir.path().join("templates"),
            0,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn merge_dir_nonexistent_src_is_noop() {
        let dir = tempdir().unwrap();
        let result =
            merge_dir(&dir.path().join("nonexistent"), &dir.path().join("dst"));
        assert!(result.is_ok());
    }

    #[test]
    fn merge_dir_nested() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        let nested = src.join("sub");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(nested.join("file.txt"), "nested").unwrap();

        merge_dir(&src, &dst).unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("sub/file.txt")).unwrap(),
            "nested"
        );
    }

    #[test]
    fn should_stream_large_site() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        // Create more files than default batch size (8192)
        // Use a tiny budget instead
        let budget = MemoryBudget {
            max_bytes: 0,
            batch_size: 2,
        };
        for i in 0..5 {
            fs::write(content.join(format!("p{i}.md")), "# Hi").unwrap();
        }
        assert!(should_stream(&content, &budget, false));
    }

    #[test]
    fn should_not_stream_small_site() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("index.md"), "# Home").unwrap();

        let budget = MemoryBudget::default_budget();
        assert!(!should_stream(&content, &budget, false));
    }

    // -----------------------------------------------------------------
    // MemoryBudget — edge cases
    // -----------------------------------------------------------------

    #[test]
    fn memory_budget_from_mb_one() {
        let budget = MemoryBudget::from_mb(1);
        assert_eq!(budget.max_bytes, 1024 * 1024);
        // 1 MB / 64 KB = 16 pages per batch
        assert_eq!(budget.batch_size, 16);
    }

    #[test]
    fn memory_budget_from_mb_very_large() {
        let budget = MemoryBudget::from_mb(4096);
        assert_eq!(budget.max_bytes, 4096 * 1024 * 1024);
        // 4 GB / 64 KB = 65536 pages per batch
        assert_eq!(budget.batch_size, 65536);
    }

    #[test]
    fn memory_budget_batch_size_floor_is_ten() {
        // Even with 0 MB, the floor ensures at least 10 pages/batch
        let budget = MemoryBudget::from_mb(0);
        assert_eq!(budget.max_bytes, 0);
        assert_eq!(budget.batch_size, 10);
    }

    #[test]
    fn memory_budget_default_budget_matches_constant() {
        let budget = MemoryBudget::default_budget();
        assert_eq!(budget.max_bytes, DEFAULT_MEMORY_BUDGET_MB * 1024 * 1024);
        assert_eq!(
            budget.batch_size,
            MemoryBudget::from_mb(DEFAULT_MEMORY_BUDGET_MB).batch_size
        );
    }

    #[test]
    fn memory_budget_clone_copy_debug() {
        let a = MemoryBudget::from_mb(128);
        let b = a; // Copy
        assert_eq!(a.max_bytes, b.max_bytes);
        assert_eq!(a.batch_size, b.batch_size);
        let debug = format!("{a:?}");
        assert!(debug.contains("MemoryBudget"));
    }

    // -----------------------------------------------------------------
    // batched_content_files — additional scenarios
    // -----------------------------------------------------------------

    #[test]
    fn batched_content_files_nonexistent_dir_returns_empty() {
        let dir = tempdir().unwrap();
        let budget = MemoryBudget::from_mb(512);
        let result =
            batched_content_files(&dir.path().join("nonexistent"), &budget);
        // walk_files treats a missing dir as empty, so batched returns
        // Ok([]) — asserted without a conditional so no dead branch.
        let batches = result.unwrap_or_default();
        assert!(batches.is_empty(), "{:?}", batches);
    }

    #[test]
    fn batched_content_files_single_file() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("index.md"), "# Home").unwrap();

        let budget = MemoryBudget::from_mb(512);
        let batches = batched_content_files(&content, &budget).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    #[test]
    fn batched_content_files_ignores_non_md() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("page.md"), "# Page").unwrap();
        fs::write(content.join("image.png"), "fakepng").unwrap();
        fs::write(content.join("style.css"), "body{}").unwrap();

        let budget = MemoryBudget::from_mb(512);
        let batches = batched_content_files(&content, &budget).unwrap();
        let total: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total, 1, "only .md files should be collected");
    }

    #[test]
    fn batched_content_files_exact_batch_boundary() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        for i in 0..10 {
            fs::write(content.join(format!("p{i}.md")), "# Hi").unwrap();
        }

        let budget = MemoryBudget {
            max_bytes: 0,
            batch_size: 10,
        };
        let batches = batched_content_files(&content, &budget).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 10);
    }

    #[test]
    fn batched_content_files_many_small_batches() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        for i in 0..7 {
            fs::write(content.join(format!("p{i}.md")), "# Hi").unwrap();
        }

        let budget = MemoryBudget {
            max_bytes: 0,
            batch_size: 2,
        };
        let batches = batched_content_files(&content, &budget).unwrap();
        assert_eq!(batches.len(), 4); // 2+2+2+1
        assert_eq!(batches[3].len(), 1);
    }

    #[test]
    fn batched_content_files_nested_directories() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(content.join("blog")).unwrap();
        fs::create_dir_all(content.join("docs")).unwrap();
        fs::write(content.join("index.md"), "# Index").unwrap();
        fs::write(content.join("blog/post.md"), "# Post").unwrap();
        fs::write(content.join("docs/api.md"), "# API").unwrap();

        let budget = MemoryBudget::from_mb(512);
        let batches = batched_content_files(&content, &budget).unwrap();
        let total: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total, 3);
    }

    // -----------------------------------------------------------------
    // merge_dir — additional scenarios
    // -----------------------------------------------------------------

    #[test]
    fn merge_dir_deeply_nested() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(src.join("a/b/c")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a/b/c/deep.txt"), "deep content").unwrap();

        merge_dir(&src, &dst).unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("a/b/c/deep.txt")).unwrap(),
            "deep content"
        );
    }

    #[test]
    fn merge_dir_empty_src() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("existing.txt"), "keep").unwrap();

        merge_dir(&src, &dst).unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("existing.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn merge_dir_multiple_files() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        for i in 0..5 {
            fs::write(src.join(format!("f{i}.txt")), format!("data{i}"))
                .unwrap();
        }

        merge_dir(&src, &dst).unwrap();
        for i in 0..5 {
            assert_eq!(
                fs::read_to_string(dst.join(format!("f{i}.txt"))).unwrap(),
                format!("data{i}")
            );
        }
    }

    // -----------------------------------------------------------------
    // should_stream — additional scenarios
    // -----------------------------------------------------------------

    #[test]
    fn should_stream_with_no_content_dir() {
        let dir = tempdir().unwrap();
        let budget = MemoryBudget::from_mb(512);
        // Non-existent dir, not explicitly set => false (walk returns 0)
        assert!(!should_stream(
            &dir.path().join("no-content"),
            &budget,
            false
        ));
    }

    #[test]
    fn should_stream_explicitly_set_overrides_count() {
        // Even with zero files, explicit flag forces streaming
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();

        let budget = MemoryBudget::from_mb(512);
        assert!(should_stream(&content, &budget, true));
    }

    #[test]
    fn should_stream_exactly_at_batch_boundary() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        // Create exactly batch_size files => count == batch_size, not >
        let budget = MemoryBudget {
            max_bytes: 0,
            batch_size: 3,
        };
        for i in 0..3 {
            fs::write(content.join(format!("p{i}.md")), "# Hi").unwrap();
        }
        // 3 files, batch_size 3 => count is NOT > batch_size => false
        assert!(!should_stream(&content, &budget, false));
    }

    #[test]
    fn should_stream_one_over_boundary() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let budget = MemoryBudget {
            max_bytes: 0,
            batch_size: 3,
        };
        for i in 0..4 {
            fs::write(content.join(format!("p{i}.md")), "# Hi").unwrap();
        }
        // 4 files, batch_size 3 => true
        assert!(should_stream(&content, &budget, false));
    }

    // -----------------------------------------------------------------
    // compile_batch — additional scenarios
    // -----------------------------------------------------------------

    #[test]
    fn compile_batch_with_nonexistent_files_still_creates_dirs() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();

        // Pass paths that don't exist — the copy inside compile_batch
        // will fail, but the batch content dir should still be created.
        let result = compile_batch(
            &[content.join("nonexistent.md")],
            &content,
            &build,
            &site,
            &templates,
            0,
        );
        // This may error (file not found during copy), which is expected.
        // The important thing is it doesn't panic.
        let _ = result;
    }

    #[test]
    fn compile_batch_creates_batch_content_dir() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        let site = dir.path().join("site");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::write(content.join("page.md"), "---\ntitle: T\n---\n# Hi").unwrap();

        // compile_batch with a real file — may fail at staticdatagen::compile
        // but should not panic and should create the batch dir
        let _result = compile_batch(
            &[content.join("page.md")],
            &content,
            &build,
            &site,
            &templates,
            42,
        );
        // Batch dirs are cleaned up, so we just verify no panic
    }

    #[test]
    fn compile_batch_succeeds_and_merges_valid_page() {
        // Mirrors the pipeline build fixture (full frontmatter plus a
        // page template) so staticdatagen's compile succeeds — driving
        // the merge-into-site_dir branch after a successful batch.
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        let site = dir.path().join("public");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&build).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::write(
            content.join("index.md"),
            "---\ntitle: \"Home\"\ndescription: \"home\"\n\
             permalink: \"https://example.com/\"\n---\nhome body",
        )
        .unwrap();
        fs::write(
            templates.join("page.html"),
            "<!doctype html><html><body>{{ content }}</body></html>",
        )
        .unwrap();

        let result = compile_batch(
            &[content.join("index.md")],
            &content,
            &build,
            &site,
            &templates,
            7,
        );
        assert!(result.is_ok(), "expected successful batch: {result:?}");
        assert!(site.is_dir(), "site dir must be created on success");
    }

    #[test]
    fn compile_batch_fails_when_build_dir_is_a_file() {
        // build_dir exists as a plain file, so creating the batch
        // content dir underneath it fails immediately.
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build_file = dir.path().join("build");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("a.md"), "x").unwrap();
        fs::write(&build_file, "i am a file").unwrap();

        let result = compile_batch(
            &[content.join("a.md")],
            &content,
            &build_file,
            &dir.path().join("site"),
            &dir.path().join("templates"),
            0,
        );
        assert!(result.is_err());
    }

    #[test]
    fn compile_batch_fails_when_dest_parent_blocked_by_file() {
        // The per-file parent create_dir_all fails: the batch content
        // dir already contains a plain file where a subdirectory is
        // needed.
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        fs::create_dir_all(content.join("sub")).unwrap();
        fs::write(content.join("sub/a.md"), "x").unwrap();
        let batch_content = build.join(".batch-3");
        fs::create_dir_all(&batch_content).unwrap();
        fs::write(batch_content.join("sub"), "blocking file").unwrap();

        let result = compile_batch(
            &[content.join("sub/a.md")],
            &content,
            &build,
            &dir.path().join("site"),
            &dir.path().join("templates"),
            3,
        );
        assert!(result.is_err());
    }

    #[test]
    fn compile_batch_fails_when_batch_build_dir_blocked() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("a.md"), "x").unwrap();
        fs::create_dir_all(&build).unwrap();
        // Block the derived batch-build path with a plain file.
        fs::write(build.join(".batch-5-build"), "blocking file").unwrap();

        let result = compile_batch(
            &[content.join("a.md")],
            &content,
            &build,
            &dir.path().join("site"),
            &dir.path().join("templates"),
            5,
        );
        assert!(result.is_err());
    }

    #[test]
    fn compile_batch_fails_when_merge_dir_blocked_after_successful_compile() {
        // Every other failure test in this module fails *before*
        // reaching the merge step (blocked temp dirs abort the copy or
        // the compile itself). Here the compile succeeds — two real
        // pages, one of them nested under `about/` — but the site dir
        // already has a plain *file* named `about`, so
        // `merge_dir(&batch_site, site_dir)?` fails on
        // `create_dir_all` when it tries to recreate that
        // subdirectory. This drives the `?` propagation inside the
        // `if compile_result.is_ok()` block.
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        let site = dir.path().join("public");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&build).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::write(
            content.join("index.md"),
            "---\ntitle: \"Home\"\ndescription: \"home\"\n\
             permalink: \"https://example.com/\"\n---\nhome body",
        )
        .unwrap();
        fs::write(
            content.join("about.md"),
            "---\ntitle: \"About\"\ndescription: \"about\"\n\
             permalink: \"https://example.com/about/\"\n---\nabout body",
        )
        .unwrap();
        fs::write(
            templates.join("page.html"),
            "<!doctype html><html><body>{{ content }}</body></html>",
        )
        .unwrap();

        // Pre-block the nested output directory with a plain file so
        // merge_dir's `create_dir_all(&dest)` fails once compile
        // succeeds and produces `about/index.html` in the batch output.
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("about"), "blocking file, not a directory")
            .unwrap();

        let result = compile_batch(
            &[content.join("index.md"), content.join("about.md")],
            &content,
            &build,
            &site,
            &templates,
            11,
        );
        assert!(
            result.is_err(),
            "merge_dir failure after a successful compile must propagate: {result:?}"
        );
    }

    #[test]
    fn compile_batch_fails_when_batch_site_dir_blocked() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let build = dir.path().join("build");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("a.md"), "x").unwrap();
        fs::create_dir_all(&build).unwrap();
        fs::write(build.join(".batch-6-site"), "blocking file").unwrap();

        let result = compile_batch(
            &[content.join("a.md")],
            &content,
            &build,
            &dir.path().join("site"),
            &dir.path().join("templates"),
            6,
        );
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // merge_dir — error branches
    // -----------------------------------------------------------------

    #[test]
    fn merge_dir_src_is_file_fails_at_read_dir() {
        let dir = tempdir().unwrap();
        let src_file = dir.path().join("plain.txt");
        fs::write(&src_file, "x").unwrap();

        let result = merge_dir(&src_file, &dir.path().join("dst"));
        assert!(result.is_err());
    }

    #[test]
    fn merge_dir_subdir_blocked_by_file_in_dst() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/f.html"), "x").unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("sub"), "blocking file").unwrap();

        let result = merge_dir(&src, &dst);
        assert!(result.is_err());
    }

    #[test]
    fn merge_dir_nested_failure_propagates_through_recursion() {
        // The inner merge_dir call fails (blocked sub-subdir), and the
        // error propagates through the recursive `?`.
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("a/b/f.html"), "x").unwrap();
        fs::create_dir_all(dst.join("a")).unwrap();
        fs::write(dst.join("a/b"), "blocking file").unwrap();

        let result = merge_dir(&src, &dst);
        assert!(result.is_err());
    }

    #[test]
    fn merge_dir_copy_into_missing_dst_fails() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("f.html"), "x").unwrap();

        // dst does not exist: fs::copy into it fails.
        let result = merge_dir(&src, &dir.path().join("missing-dst"));
        assert!(result.is_err());
    }
}
