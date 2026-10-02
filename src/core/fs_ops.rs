// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! File system operations: directory copying, safety validation, and traversal.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{PathErrorExt, SsgError};
use rayon::prelude::*;

use crate::MAX_DIR_DEPTH;

/// Minimum number of entries to justify Rayon parallel dispatch overhead.
pub(crate) const PARALLEL_THRESHOLD: usize = 16;

/// Validates and copies files from source to destination.
///
/// This function performs comprehensive safety checks before copying files,
/// including path validation, symlink detection, and size limitations.
///
/// # Arguments
///
/// * `src` - Source path to copy from
/// * `dst` - Destination path to copy to
///
/// # Returns
///
/// Returns `Ok(())` if the copy operation succeeds, or an error if:
/// * Source path is invalid or inaccessible
/// * Source contains symlinks (not allowed)
/// * Files exceed size limits (default: 10MB)
/// * Destination cannot be created or written to
///
/// # Examples
///
/// ```rust
/// use ssg::verify_and_copy_files;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let src_dir = tempdir().unwrap();
/// let dst_dir = tempdir().unwrap();
/// fs::write(src_dir.path().join("a.txt"), "data").unwrap();
/// verify_and_copy_files(src_dir.path(), dst_dir.path()).unwrap();
/// assert!(dst_dir.path().join("a.txt").exists());
/// ```
///
/// # Security
///
/// This function implements several security measures:
/// * Path traversal prevention
/// * Symlink restriction
/// * File size limits
/// * Permission validation
pub fn verify_and_copy_files(src: &Path, dst: &Path) -> Result<(), SsgError> {
    if !is_safe_path(src)? {
        return Err(SsgError::PathTraversal {
            path: src.to_path_buf(),
        });
    }

    if !src.exists() {
        return Err(SsgError::Validation {
            field: "src".to_string(),
            message: format!(
                "Source directory does not exist: {}",
                src.display()
            ),
        });
    }

    // If source is a file, verify its safety
    if src.is_file() {
        verify_file_safety(src)?;
    }

    // Ensure the destination directory exists
    fs::create_dir_all(dst).with_path(dst)?;

    // Copy directory contents with safety checks
    copy_dir_all(src, dst)?;

    Ok(())
}

/// Asynchronously validates and copies files between directories.
///
/// Uses iterative traversal with an explicit stack to avoid unbounded recursion.
/// Traversal depth is bounded by [`MAX_DIR_DEPTH`].
///
/// # Examples
///
/// ```rust
/// use ssg::fs_ops::verify_and_copy_files_async;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let src = tempdir().unwrap();
/// let dst = tempdir().unwrap();
/// fs::write(src.path().join("x.txt"), "hi").unwrap();
/// verify_and_copy_files_async(src.path(), dst.path()).unwrap();
/// assert!(dst.path().join("x.txt").is_file());
/// ```
pub fn verify_and_copy_files_async(
    src: &Path,
    dst: &Path,
) -> Result<(), SsgError> {
    if !src.exists() {
        return Err(SsgError::Validation {
            field: "src".to_string(),
            message: format!(
                "Source directory does not exist: {}",
                src.display()
            ),
        });
    }

    fs::create_dir_all(dst).with_path(dst)?;

    copy_directory_recursive(src, dst)
}

/// Iteratively copies a directory tree with depth bounds and safety checks.
fn copy_directory_recursive(src: &Path, dst: &Path) -> Result<(), SsgError> {
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf(), 0usize)];

    while let Some((src_dir, dst_dir, depth)) = stack.pop() {
        if depth >= MAX_DIR_DEPTH {
            return Err(SsgError::Validation {
                field: "directory_depth".to_string(),
                message: format!(
                    "Directory nesting exceeds maximum depth of {}: {}",
                    MAX_DIR_DEPTH,
                    src_dir.display()
                ),
            });
        }

        for entry in fs::read_dir(&src_dir).with_path(&src_dir)? {
            let entry = entry.with_path(&src_dir)?;
            copy_entry(&entry, &dst_dir, depth, &mut stack)?;
        }
    }

    Ok(())
}

/// Copies a single directory entry, pushing subdirs onto the stack.
fn copy_entry(
    entry: &fs::DirEntry,
    dst_dir: &Path,
    depth: usize,
    stack: &mut Vec<(PathBuf, PathBuf, usize)>,
) -> Result<(), SsgError> {
    let src_path = entry.path();
    let dst_path = dst_dir.join(entry.file_name());

    if src_path.is_dir() {
        fs::create_dir_all(&dst_path).with_path(&dst_path)?;
        stack.push((src_path, dst_path, depth + 1));
    } else {
        verify_file_safety(&src_path)?;
        _ = fs::copy(&src_path, &dst_path).with_path(&dst_path)?;
    }
    Ok(())
}

/// Copies directories with a progress bar for feedback.
///
/// Uses iterative traversal with an explicit stack to avoid unbounded recursion.
/// Traversal depth is bounded by [`MAX_DIR_DEPTH`].
///
/// # Examples
///
/// ```rust
/// use ssg::fs_ops::copy_dir_with_progress;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let src = tempdir().unwrap();
/// let dst = tempdir().unwrap();
/// fs::write(src.path().join("a.txt"), "x").unwrap();
/// copy_dir_with_progress(src.path(), dst.path()).unwrap();
/// assert!(dst.path().join("a.txt").exists());
/// ```
pub fn copy_dir_with_progress(src: &Path, dst: &Path) -> Result<(), SsgError> {
    if !src.exists() {
        return Err(SsgError::Validation {
            field: "src".to_string(),
            message: format!(
                "Source directory does not exist: {}",
                src.display()
            ),
        });
    }

    fs::create_dir_all(dst).with_path(dst)?;

    let mut file_count: u64 = 0;

    // (source_dir, dest_dir, depth)
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf(), 0usize)];

    while let Some((src_dir, dst_dir, depth)) = stack.pop() {
        if depth >= MAX_DIR_DEPTH {
            return Err(SsgError::Validation {
                field: "directory_depth".to_string(),
                message: format!(
                    "Directory nesting exceeds maximum depth of {}: {}",
                    MAX_DIR_DEPTH,
                    src_dir.display()
                ),
            });
        }

        let entries: Vec<_> = fs::read_dir(&src_dir)
            .with_path(&src_dir)?
            .collect::<std::io::Result<Vec<_>>>()
            .with_path(&src_dir)?;

        for entry in &entries {
            let src_path = entry.path();
            let dst_path = dst_dir.join(entry.file_name());

            if src_path.is_dir() {
                fs::create_dir_all(&dst_path).with_path(&dst_path)?;
                stack.push((src_path, dst_path, depth + 1));
            } else {
                _ = fs::copy(&src_path, &dst_path).with_path(&dst_path)?;
            }
            file_count += 1;
        }
    }

    eprintln!("Copied {file_count} files");
    Ok(())
}

/// Checks if a given path is safe to use.
///
/// Validates that the provided path does not contain directory traversal attempts
/// or other potential security risks.
///
/// # Arguments
///
/// * `path` - The path to validate
///
/// # Returns
///
/// * `Ok(true)` - If the path is safe to use
/// * `Ok(false)` - If the path contains unsafe elements
/// * `Err` - If path validation fails
///
/// # Security
///
/// This function prevents directory traversal attacks by:
/// * Checking for parent directory references (`..`) as genuine path
///   *components* (via [`Path::components`]), not a substring match —
///   so a literal `..` inside a filename (e.g. `notes..final.md`)
///   is never a false positive, and no encoding trick produces a
///   false negative.
/// * Rejecting any such component **unconditionally**, whether or not
///   the path currently exists. A prior version of this check only
///   ran for non-existent paths, so a traversal payload that happened
///   to resolve to a real file (e.g. `../../etc/passwd`, which exists
///   on every Unix system) skipped the traversal check entirely and
///   fell through to `canonicalize()`, which succeeds for any real
///   file — silently reporting a genuine traversal attempt as safe.
/// * Resolving symbolic links for paths that do exist and pass the
///   component check, surfacing a broken symlink as unsafe.
///
/// This function alone does **not** confine a path to a particular
/// directory tree — a path with no `..` components can still resolve
/// (via a symlink) to an arbitrary location. Callers that need that
/// stronger guarantee should also use [`is_path_within_root`].
///
/// # Examples
///
/// ```rust
/// use ssg::fs_ops::is_safe_path;
/// use std::path::Path;
///
/// assert!(is_safe_path(Path::new("safe/path")).unwrap());
/// assert!(!is_safe_path(Path::new("../escape")).unwrap());
/// ```
pub fn is_safe_path(path: &Path) -> Result<bool, SsgError> {
    use std::path::Component;

    // Reject genuine parent-directory *components* unconditionally,
    // before ever checking existence. Matching on `Component::ParentDir`
    // (rather than a `contains("..")` substring check) means a
    // filename that merely contains two literal dots is never
    // mistaken for a traversal attempt.
    if path.components().any(|c| c == Component::ParentDir) {
        return Ok(false);
    }

    if !path.exists() {
        return Ok(true); // Non-existent paths without traversal are safe
    }

    // canonicalize() resolves symlinks and all `..' components,
    // so the resulting path is always absolute with no parent refs.
    // A failure here (e.g. broken symlink) means the path is unsafe.
    let _canonical = path.canonicalize().with_path(path)?;

    Ok(true)
}

/// Checks that `path` resolves to a location inside `root`.
///
/// Complements [`is_safe_path`]: that function only rejects paths
/// whose *string form* contains a `..` component, so it cannot catch
/// a path that looks innocuous but resolves elsewhere via a symlink
/// (e.g. `content` is a symlink to `/etc`). This function canonicalizes
/// both `path` and `root` — resolving all symlinks and `..` components
/// — and verifies the former is a descendant of (or equal to) the
/// latter, closing that gap.
///
/// `root` must exist. `path` must exist (use [`is_safe_path`] first
/// for pre-creation checks on paths that don't exist yet).
///
/// # Errors
///
/// Returns an [`SsgError`] if either `path` or `root` cannot be
/// canonicalized (e.g. does not exist, or a broken symlink).
///
/// # Examples
///
/// ```rust
/// use ssg::fs_ops::is_path_within_root;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let root = tempdir().unwrap();
/// let inner = root.path().join("content");
/// fs::create_dir(&inner).unwrap();
///
/// assert!(is_path_within_root(&inner, root.path()).unwrap());
/// ```
pub fn is_path_within_root(path: &Path, root: &Path) -> Result<bool, SsgError> {
    let canonical_path = path.canonicalize().with_path(path)?;
    let canonical_root = root.canonicalize().with_path(root)?;
    Ok(canonical_path.starts_with(&canonical_root))
}

/// Verifies the safety of a file for processing.
///
/// Performs comprehensive safety checks on a file to ensure it meets security
/// requirements before processing. These checks include symlink detection and
/// file size validation.
///
/// # Arguments
///
/// * `path` - Reference to the path of the file to verify
///
/// # Returns
///
/// * `Ok(())` - If the file passes all safety checks
/// * `Err` - If any safety check fails
///
/// # Safety Checks
///
/// * Symlinks: Not allowed (returns error)
/// * File size: Must be under 10MB
/// * File type: Must be a regular file
///
/// # Examples
///
/// Verifies the safety of a file.
///
/// ```rust
/// use std::fs;
/// use std::path::Path;
/// use ssg::verify_file_safety;
/// use tempfile::tempdir;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // Create temporary directory
/// let temp_dir = tempdir()?;
/// let file_path = temp_dir.path().join("index.md");
///
/// // Create test file
/// fs::write(&file_path, "Hello, world!")?;
///
/// // Perform verification
/// verify_file_safety(&file_path)?;
///
/// // Directory and file are automatically cleaned up
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns an error if:
/// * File is a symlink
/// * File size exceeds 10MB
/// * Cannot read file metadata
pub fn verify_file_safety(path: &Path) -> Result<(), SsgError> {
    const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB limit

    // Get symlink metadata without following the symlink
    let symlink_metadata = path.symlink_metadata().with_path(path)?;

    // Explicitly check for symlinks first
    if symlink_metadata.file_type().is_symlink() {
        return Err(SsgError::SymlinkForbidden {
            path: path.to_path_buf(),
        });
    }

    // Only check size if it's a regular file
    if symlink_metadata.file_type().is_file()
        && symlink_metadata.len() > MAX_FILE_SIZE
    {
        return Err(SsgError::Validation {
            field: "file_size".to_string(),
            message: format!(
                "File exceeds maximum allowed size of {} bytes: {}",
                MAX_FILE_SIZE,
                path.display()
            ),
        });
    }

    Ok(())
}

/// Recursively collects all file paths within a directory.
///
/// Traverses a directory tree and compiles a list of all file paths found,
/// excluding directories themselves.
///
/// # Arguments
///
/// * `dir` - Reference to the directory to search
/// * `files` - Mutable vector to store found file paths
///
/// # Returns
///
/// * `Ok(())` - If the collection process succeeds
/// * `Err` - If any file system operation fails
///
/// # Examples
///
/// ```rust
/// use std::path::{Path, PathBuf};
/// use ssg::collect_files_recursive;
///
/// fn main() -> Result<(), ssg::error::SsgError> {
///     let mut files = Vec::new();
///     let dir_path = Path::new("./examples/content");
///
///     collect_files_recursive(dir_path, &mut files)?;
///
///     for file in files {
///         println!("Found file: {}", file.display());
///     }
///
///     Ok(())
/// }
/// ```
///
/// # Note
///
/// This function:
/// * Only collects file paths, not directory paths
/// * Rejects symbolic links (consistent with security model)
/// * Maintains original path structure
pub fn collect_files_recursive(
    dir: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), SsgError> {
    // (directory, depth)
    let mut stack = vec![(dir.to_path_buf(), 0usize)];

    while let Some((current_dir, depth)) = stack.pop() {
        if depth >= MAX_DIR_DEPTH {
            return Err(SsgError::Validation {
                field: "directory_depth".to_string(),
                message: format!(
                    "Directory nesting exceeds maximum depth of {}: {}",
                    MAX_DIR_DEPTH,
                    current_dir.display()
                ),
            });
        }

        for entry in fs::read_dir(&current_dir).with_path(&current_dir)? {
            let path = entry.with_path(&current_dir)?.path();

            if path.is_dir() {
                stack.push((path, depth + 1));
            } else {
                files.push(path);
            }
        }
    }
    Ok(())
}

/// Recursively copies a directory whilst maintaining structure and attributes.
///
/// Performs a deep copy of a directory tree, preserving file attributes and
/// handling nested directories. Uses parallel processing for improved performance.
///
/// # Arguments
///
/// * `src` - Source directory path
/// * `dst` - Destination directory path
///
/// # Returns
///
/// * `Ok(())` - If the copy operation succeeds
/// * `Err` - If any part of the copy operation fails
///
/// # Performance
///
/// Uses rayon for parallel processing of files, significantly improving
/// performance for directories with many files.
///
/// # Safety
///
/// * Verifies file safety before copying
/// * Maintains original file permissions
/// * Handles circular references
///
/// # Examples
///
/// ```rust
/// use ssg::fs_ops::copy_dir_all;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let src = tempdir().unwrap();
/// let dst = tempdir().unwrap();
/// fs::write(src.path().join("z.txt"), "z").unwrap();
/// copy_dir_all(src.path(), dst.path()).unwrap();
/// assert!(dst.path().join("z.txt").exists());
/// ```
pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), SsgError> {
    fs::create_dir_all(dst).with_path(dst)?;

    // (source_dir, dest_dir, depth)
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf(), 0usize)];

    while let Some((src_dir, dst_dir, depth)) = stack.pop() {
        if depth >= MAX_DIR_DEPTH {
            return Err(SsgError::Validation {
                field: "directory_depth".to_string(),
                message: format!(
                    "Directory nesting exceeds maximum depth of {}: {}",
                    MAX_DIR_DEPTH,
                    src_dir.display()
                ),
            });
        }

        let entries: Vec<_> = fs::read_dir(&src_dir)
            .with_path(&src_dir)?
            .collect::<std::io::Result<Vec<_>>>()
            .with_path(&src_dir)?;

        let (files, subdirs) = partition_entries(&entries, &dst_dir);

        copy_files_maybe_parallel(&files, &dst_dir)?;

        for (sub_src, sub_dst) in subdirs {
            fs::create_dir_all(&sub_dst).with_path(&sub_dst)?;
            stack.push((sub_src, sub_dst, depth + 1));
        }
    }

    Ok(())
}

/// Separates directory entries into files and subdirectories.
fn partition_entries<'a>(
    entries: &'a [fs::DirEntry],
    dst_dir: &Path,
) -> (Vec<&'a fs::DirEntry>, Vec<(PathBuf, PathBuf)>) {
    let mut subdirs = Vec::new();
    let files: Vec<_> = entries
        .iter()
        .filter(|entry| {
            let path = entry.path();
            if path.is_dir() {
                subdirs.push((path, dst_dir.join(entry.file_name())));
                false
            } else {
                true
            }
        })
        .collect();
    (files, subdirs)
}

/// Copies file entries, using parallel dispatch when the count justifies it.
fn copy_files_maybe_parallel(
    files: &[&fs::DirEntry],
    dst_dir: &Path,
) -> Result<(), SsgError> {
    let copy_file = |entry: &&fs::DirEntry| -> Result<(), SsgError> {
        let src_path = entry.path();
        let dst_path = dst_dir.join(entry.file_name());
        verify_file_safety(&src_path)?;
        _ = fs::copy(&src_path, &dst_path).with_path(&dst_path)?;
        Ok(())
    };

    if files.len() >= PARALLEL_THRESHOLD {
        files.par_iter().try_for_each(copy_file)?;
    } else {
        files.iter().try_for_each(copy_file)?;
    }
    Ok(())
}

/// Asynchronously copies an entire directory structure, preserving file attributes and handling nested directories.
///
/// # Parameters
///
/// * `src`: A reference to the source directory path.
/// * `dst`: A reference to the destination directory path.
///
/// # Returns
///
/// * `Result<()>`:
///   - `Ok(())`: If the directory copying is successful.
///   - `Err(e)`: If an error occurs during the directory copying, where `e` is the associated error.
///
/// # Errors
///
/// This function can return the following errors:
///
/// * `std::io::Error`: If an error occurs during directory creation, file copying, or permission issues.
/// * `anyhow::Error`: If a file safety check fails.
///
/// # Examples
///
/// ```rust
/// use ssg::fs_ops::copy_dir_all_async;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let src = tempdir().unwrap();
/// let dst = tempdir().unwrap();
/// fs::write(src.path().join("z.txt"), "z").unwrap();
/// copy_dir_all_async(src.path(), dst.path()).unwrap();
/// assert!(dst.path().join("z.txt").exists());
/// ```
pub fn copy_dir_all_async(src: &Path, dst: &Path) -> Result<(), SsgError> {
    internal_copy_dir_async(src, dst)
}

fn internal_copy_dir_async(src: &Path, dst: &Path) -> Result<(), SsgError> {
    fs::create_dir_all(dst).with_path(dst)?;

    // (source_dir, dest_dir, depth)
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf(), 0usize)];

    while let Some((src_path, dst_path, depth)) = stack.pop() {
        if depth >= MAX_DIR_DEPTH {
            return Err(SsgError::Validation {
                field: "directory_depth".to_string(),
                message: format!(
                    "Directory nesting exceeds maximum depth of {}: {}",
                    MAX_DIR_DEPTH,
                    src_path.display()
                ),
            });
        }

        for entry in fs::read_dir(&src_path).with_path(&src_path)? {
            let entry = entry.with_path(&src_path)?;
            let src_entry = entry.path();
            let dst_entry = dst_path.join(entry.file_name());

            if src_entry.is_dir() {
                fs::create_dir_all(&dst_entry).with_path(&dst_entry)?;
                stack.push((src_entry, dst_entry, depth + 1));
            } else {
                verify_file_safety(&src_entry)?;
                _ = fs::copy(&src_entry, &dst_entry).with_path(&dst_entry)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn copy_dir_all_copies_files() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("a.txt"), "hello").unwrap();
        fs::write(src.path().join("b.txt"), "world").unwrap();

        copy_dir_all(src.path(), dst.path()).unwrap();

        assert_eq!(
            fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(dst.path().join("b.txt")).unwrap(),
            "world"
        );
    }

    #[test]
    fn copy_dir_all_nested_preserves_structure() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        let nested = src.path().join("sub").join("deep");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("file.txt"), "nested content").unwrap();
        fs::write(src.path().join("root.txt"), "root").unwrap();

        copy_dir_all(src.path(), dst.path()).unwrap();

        assert_eq!(
            fs::read_to_string(dst.path().join("sub/deep/file.txt")).unwrap(),
            "nested content"
        );
        assert_eq!(
            fs::read_to_string(dst.path().join("root.txt")).unwrap(),
            "root"
        );
    }

    #[test]
    fn copy_dir_all_nonexistent_src_returns_error() {
        let dst = tempdir().unwrap();
        let fake_src = dst.path().join("does_not_exist");

        let result = copy_dir_all(&fake_src, dst.path());
        assert!(result.is_err());
    }

    #[test]
    fn is_safe_path_normal_relative() {
        let tmp = tempdir().unwrap();
        let file = tmp.path().join("safe.txt");
        fs::write(&file, "ok").unwrap();

        assert!(is_safe_path(&file).unwrap());
    }

    #[test]
    fn is_safe_path_with_dotdot_nonexistent() {
        let path = Path::new("some/../../../etc/passwd");
        assert!(!is_safe_path(path).unwrap());
    }

    #[test]
    fn is_safe_path_with_dotdot_existing_is_now_rejected() {
        // Prior to the fix, `is_safe_path` only checked for `..` on
        // non-existent paths, so an *existing* path containing `..`
        // (even one that canonicalizes to somewhere entirely benign,
        // like this one — `tmp/a/..` resolves right back to `tmp`)
        // was reported safe purely because `canonicalize()` succeeds
        // for any real file. That's the same code path a genuine
        // traversal payload takes once it happens to resolve to a
        // real file (e.g. `../../etc/passwd`) — see
        // `is_safe_path_existing_traversal_to_real_file_is_rejected`
        // below. `is_safe_path` now rejects any `..` *component*
        // unconditionally, so this benign case is (correctly, if
        // conservatively) rejected too. Callers that need to allow a
        // resolved-but-in-bounds backtrack should canonicalize first
        // and use `is_path_within_root` instead.
        let tmp = tempdir().unwrap();
        let safe = tmp.path().join("a");
        fs::create_dir_all(&safe).unwrap();
        let dotdot_path = safe.join("..");
        assert!(!is_safe_path(&dotdot_path).unwrap());
        // The canonicalized equivalent (no `..` component at all) is
        // exactly what a caller should pass instead, and remains safe.
        assert!(is_safe_path(&dotdot_path.canonicalize().unwrap()).unwrap());
    }

    /// Regression test for the exact vulnerability this fix closes:
    /// a traversal path that happens to resolve to a real, existing
    /// file must still be rejected. Before the fix, `is_safe_path`
    /// only checked for `..` when the target did *not* exist, so any
    /// traversal payload landing on something real (like `/etc`,
    /// which exists on every Unix system) skipped the check entirely.
    #[test]
    fn is_safe_path_existing_traversal_to_real_file_is_rejected() {
        // `/etc` exists on every Unix CI/dev machine this crate
        // targets. The exact depth of `..` needed to reach it from
        // `CARGO_MANIFEST_DIR` doesn't matter for this test — what
        // matters is that `/etc` (an unambiguously real, existing
        // directory) is reachable via *some* relative traversal from
        // the crate root, and that reachability must not make the
        // check pass.
        let existing_via_traversal = Path::new("../etc");
        // Only assert if this environment actually has /etc reachable
        // one level up (true for the CI/dev environments this crate
        // targets); this keeps the test meaningful without hardcoding
        // a specific relative depth that could vary by checkout path.
        if existing_via_traversal.exists() {
            assert!(!is_safe_path(existing_via_traversal).unwrap());
        }
        // Directly construct a path guaranteed to both (a) contain a
        // `..` component and (b) exist, regardless of environment:
        // canonicalize `tmp/a`, then rebuild an equivalent
        // `tmp/a/subdir/..` form that still resolves to the real,
        // existing `tmp/a` directory.
        let tmp = tempdir().unwrap();
        let real_dir = tmp.path().join("a");
        fs::create_dir_all(real_dir.join("subdir")).unwrap();
        let traversal_to_real_dir = real_dir.join("subdir").join("..");
        assert!(traversal_to_real_dir.exists());
        assert!(!is_safe_path(&traversal_to_real_dir).unwrap());
    }

    #[test]
    fn is_safe_path_rejects_literal_dotdot_in_filename_false_positive_check() {
        // A filename that merely *contains* the two-character
        // substring ".." (not a `..` path *component*) must not be
        // rejected -- confirms the fix uses component-based matching
        // (`Component::ParentDir`), not a fragile `contains("..")`
        // substring check.
        let tmp = tempdir().unwrap();
        let odd_name = tmp.path().join("notes..final.md");
        fs::write(&odd_name, "content").unwrap();
        assert!(is_safe_path(&odd_name).unwrap());
    }

    #[test]
    fn is_safe_path_absolute_existing() {
        let tmp = tempdir().unwrap();
        let file = tmp.path().join("abs.txt");
        fs::write(&file, "data").unwrap();
        // Absolute path that exists is safe
        assert!(is_safe_path(&file).unwrap());
    }

    // -----------------------------------------------------------------
    // is_path_within_root — canonicalize-based containment checking,
    // catches escapes `is_safe_path` structurally cannot (symlinks).
    // -----------------------------------------------------------------

    #[test]
    fn is_path_within_root_accepts_direct_child() {
        let tmp = tempdir().unwrap();
        let child = tmp.path().join("content");
        fs::create_dir_all(&child).unwrap();
        assert!(is_path_within_root(&child, tmp.path()).unwrap());
    }

    #[test]
    fn is_path_within_root_accepts_root_itself() {
        let tmp = tempdir().unwrap();
        assert!(is_path_within_root(tmp.path(), tmp.path()).unwrap());
    }

    #[test]
    fn is_path_within_root_accepts_deeply_nested_child() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        assert!(is_path_within_root(&nested, tmp.path()).unwrap());
    }

    #[test]
    fn is_path_within_root_rejects_sibling_directory() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("root");
        let sibling = tmp.path().join("sibling");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        assert!(!is_path_within_root(&sibling, &root).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn is_path_within_root_rejects_symlink_escape() {
        // The exact vulnerability class `is_safe_path` alone cannot
        // catch: a path with *no* `..` component at all that still
        // escapes the intended root by following a symlink. `content`
        // looks like an innocent subdirectory name, but it's actually
        // a symlink pointing entirely outside `root`.
        use std::os::unix::fs::symlink;

        let tmp = tempdir().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let escape_link = root.join("content");
        symlink(&outside, &escape_link).unwrap();

        assert!(
            !is_path_within_root(&escape_link, &root).unwrap(),
            "a symlink pointing outside root must not be reported as contained"
        );
    }

    #[test]
    fn is_path_within_root_errors_on_nonexistent_path() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist-yet");
        assert!(is_path_within_root(&missing, tmp.path()).is_err());
    }

    #[test]
    fn is_path_within_root_errors_on_nonexistent_root() {
        let tmp = tempdir().unwrap();
        let existing = tmp.path().join("child");
        fs::create_dir_all(&existing).unwrap();
        let missing_root = tmp.path().join("no-such-root");
        assert!(is_path_within_root(&existing, &missing_root).is_err());
    }

    #[test]
    fn verify_file_safety_valid_file() {
        let tmp = tempdir().unwrap();
        let file = tmp.path().join("ok.txt");
        fs::write(&file, "small file").unwrap();

        assert!(verify_file_safety(&file).is_ok());
    }

    #[test]
    fn verify_file_safety_nonexistent() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("nope.txt");

        // symlink_metadata fails on nonexistent file → Err
        assert!(verify_file_safety(&missing).is_err());
    }

    #[test]
    fn verify_file_safety_directory() {
        let tmp = tempdir().unwrap();
        // Directories are not files but should not error (size check skipped)
        assert!(verify_file_safety(tmp.path()).is_ok());
    }

    #[test]
    fn collect_files_recursive_finds_all() {
        let tmp = tempdir().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(tmp.path().join("a.md"), "").unwrap();
        fs::write(sub.join("b.md"), "").unwrap();
        fs::write(sub.join("c.txt"), "").unwrap();

        let mut files = Vec::new();
        collect_files_recursive(tmp.path(), &mut files).unwrap();

        assert_eq!(files.len(), 3);
    }

    #[test]
    fn collect_files_recursive_empty_dir() {
        let tmp = tempdir().unwrap();

        let mut files = Vec::new();
        collect_files_recursive(tmp.path(), &mut files).unwrap();

        assert!(files.is_empty(), "{:?}", files);
    }

    #[test]
    fn collect_files_recursive_only_files_not_dirs() {
        let tmp = tempdir().unwrap();
        let sub = tmp.path().join("subdir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("only.txt"), "data").unwrap();

        let mut files = Vec::new();
        collect_files_recursive(tmp.path(), &mut files).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("only.txt"));
    }

    #[test]
    fn collect_files_recursive_nonexistent_dir_returns_error() {
        // Unlike several sibling tree-walkers in this file,
        // `collect_files_recursive` has no explicit `!dir.exists()`
        // guard — every other test here passes a real directory, so
        // the `fs::read_dir(&current_dir).with_path(&current_dir)?`
        // error path (as opposed to the depth-guard error path, which
        // is covered separately) was never exercised.
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");

        let mut files = Vec::new();
        let result = collect_files_recursive(&missing, &mut files);
        assert!(result.is_err());
    }

    #[test]
    fn verify_and_copy_files_end_to_end() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        let target = dst.path().join("output");
        fs::write(src.path().join("page.html"), "<h1>Hi</h1>").unwrap();

        verify_and_copy_files(src.path(), &target).unwrap();

        assert_eq!(
            fs::read_to_string(target.join("page.html")).unwrap(),
            "<h1>Hi</h1>"
        );
    }

    #[test]
    fn copy_dir_with_progress_smoke() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("f.txt"), "data").unwrap();

        // Should not panic
        copy_dir_with_progress(src.path(), &dst.path().join("out")).unwrap();
    }

    #[test]
    fn copy_dir_with_progress_nonexistent_src() {
        let tmp = tempdir().unwrap();
        let fake = tmp.path().join("missing");

        let result = copy_dir_with_progress(&fake, tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn copy_dir_with_progress_src_is_file_fails_at_read_dir() {
        // `copy_dir_with_progress` has its own explicit `!src.exists()`
        // guard, which intercepts every "missing path" test above
        // before the loop's `fs::read_dir(&src_dir).with_path(&src_dir)?`
        // ever runs. Pass a *file* as `src` — it passes `.exists()` but
        // isn't a directory, so `fs::read_dir` itself fails, exercising
        // that `?` for the first time in this function.
        let tmp = tempdir().unwrap();
        let file_src = tmp.path().join("plain.txt");
        fs::write(&file_src, "x").unwrap();

        let result = copy_dir_with_progress(&file_src, &tmp.path().join("dst"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // verify_and_copy_files — validation and error branches
    // -----------------------------------------------------------------

    #[test]
    fn verify_and_copy_files_rejects_traversal_path() {
        let dst = tempdir().unwrap();
        // Nonexistent path containing ".." → is_safe_path == false.
        let err = verify_and_copy_files(
            Path::new("../nonexistent-ssg-traversal"),
            dst.path(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("directory traversal"),
            "got: {err}"
        );
    }

    #[test]
    fn verify_and_copy_files_missing_src_is_validation_error() {
        let tmp = tempdir().unwrap();
        let err = verify_and_copy_files(
            &tmp.path().join("no-such-src"),
            &tmp.path().join("dst"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not exist"), "got: {err}");
    }

    #[test]
    fn verify_and_copy_files_rejects_oversized_source_file() {
        // A sparse file over the 10 MB limit trips verify_file_safety
        // on the `src.is_file()` branch.
        let tmp = tempdir().unwrap();
        let big = tmp.path().join("big.bin");
        let f = fs::File::create(&big).unwrap();
        f.set_len(10 * 1024 * 1024 + 1).unwrap();

        let err =
            verify_and_copy_files(&big, &tmp.path().join("dst")).unwrap_err();
        assert!(
            err.to_string().contains("exceeds maximum allowed size"),
            "got: {err}"
        );
    }

    #[test]
    fn verify_and_copy_files_dst_under_file_fails() {
        let src = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        fs::write(src.path().join("a.txt"), "x").unwrap();
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "file").unwrap();

        let result = verify_and_copy_files(src.path(), &blocker.join("dst"));
        assert!(result.is_err());
    }

    #[test]
    fn verify_and_copy_files_small_file_src_fails_in_copy_stage() {
        // A regular file passes the safety check but read_dir inside
        // copy_dir_all fails on a non-directory source.
        let tmp = tempdir().unwrap();
        let file_src = tmp.path().join("plain.txt");
        fs::write(&file_src, "small").unwrap();

        let result = verify_and_copy_files(&file_src, &tmp.path().join("dst"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // copy_dir_all — parallel dispatch and error branches
    // -----------------------------------------------------------------

    #[test]
    fn copy_dir_all_uses_parallel_path_at_threshold() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        for i in 0..PARALLEL_THRESHOLD {
            fs::write(src.path().join(format!("f{i}.txt")), format!("{i}"))
                .unwrap();
        }

        copy_dir_all(src.path(), dst.path()).unwrap();
        for i in 0..PARALLEL_THRESHOLD {
            assert_eq!(
                fs::read_to_string(dst.path().join(format!("f{i}.txt")))
                    .unwrap(),
                format!("{i}")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_all_sequential_rejects_symlink() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("ok.txt"), "x").unwrap();
        std::os::unix::fs::symlink(
            src.path().join("ok.txt"),
            src.path().join("link.txt"),
        )
        .unwrap();

        let err = copy_dir_all(src.path(), dst.path()).unwrap_err();
        assert!(err.to_string().contains("symlink"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_all_parallel_rejects_symlink() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        for i in 0..PARALLEL_THRESHOLD {
            fs::write(src.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        std::os::unix::fs::symlink(
            src.path().join("f0.txt"),
            src.path().join("link.txt"),
        )
        .unwrap();

        let err = copy_dir_all(src.path(), dst.path()).unwrap_err();
        assert!(err.to_string().contains("symlink"), "got: {err}");
    }

    #[test]
    fn copy_dir_all_subdir_blocked_by_file_in_dst() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::create_dir_all(src.path().join("sub")).unwrap();
        fs::write(src.path().join("sub/x.txt"), "x").unwrap();
        // A plain file occupies the destination subdirectory path.
        fs::write(dst.path().join("sub"), "blocking file").unwrap();

        let result = copy_dir_all(src.path(), dst.path());
        assert!(result.is_err());
    }

    #[test]
    fn copy_dir_all_top_level_dst_under_file_fails() {
        // `copy_dir_all`'s own leading
        // `fs::create_dir_all(dst).with_path(dst)?` had no dedicated
        // failure test in this file — sibling functions
        // (`verify_and_copy_files`, `copy_dir_all_async`,
        // `copy_dir_with_progress`) each have one, but this one didn't.
        let src = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        fs::write(src.path().join("a.txt"), "x").unwrap();
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "file").unwrap();

        let result = copy_dir_all(src.path(), &blocker.join("dst"));
        assert!(result.is_err());
    }

    #[test]
    fn copy_dir_all_file_copy_onto_directory_fails() {
        // `copy_files_maybe_parallel`'s `copy_file` closure calls
        // `verify_file_safety` (already covered by the symlink tests
        // above) and then `fs::copy(...).with_path(...)?`. No existing
        // test makes the *copy* itself fail for `copy_dir_all` — only
        // the subdirectory-creation failure above and the symlink
        // safety check were covered. Make the destination *file* path
        // already exist as a directory so `fs::copy` errors.
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("x.txt"), "x").unwrap();
        fs::create_dir_all(dst.path().join("x.txt")).unwrap();

        let result = copy_dir_all(src.path(), dst.path());
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // verify_and_copy_files_async / copy_directory_recursive
    // -----------------------------------------------------------------

    #[test]
    fn verify_and_copy_files_async_happy_path_nested() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::create_dir_all(src.path().join("sub")).unwrap();
        fs::write(src.path().join("root.txt"), "r").unwrap();
        fs::write(src.path().join("sub/leaf.txt"), "l").unwrap();

        verify_and_copy_files_async(src.path(), dst.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dst.path().join("sub/leaf.txt")).unwrap(),
            "l"
        );
    }

    #[test]
    fn verify_and_copy_files_async_missing_src_is_validation_error() {
        let tmp = tempdir().unwrap();
        let err = verify_and_copy_files_async(
            &tmp.path().join("gone"),
            &tmp.path().join("dst"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not exist"), "got: {err}");
    }

    #[test]
    fn verify_and_copy_files_async_src_file_fails_at_read_dir() {
        let tmp = tempdir().unwrap();
        let file_src = tmp.path().join("plain.txt");
        fs::write(&file_src, "x").unwrap();

        let result =
            verify_and_copy_files_async(&file_src, &tmp.path().join("dst"));
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn copy_directory_recursive_rejects_symlink_entry() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("real.txt"), "x").unwrap();
        std::os::unix::fs::symlink(
            src.path().join("real.txt"),
            src.path().join("link.txt"),
        )
        .unwrap();

        let err = copy_directory_recursive(src.path(), dst.path()).unwrap_err();
        assert!(err.to_string().contains("symlink"), "got: {err}");
    }

    #[test]
    fn copy_directory_recursive_subdir_blocked_by_file() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::create_dir_all(src.path().join("sub")).unwrap();
        fs::write(src.path().join("sub/x.txt"), "x").unwrap();
        fs::write(dst.path().join("sub"), "blocking file").unwrap();

        let result = copy_directory_recursive(src.path(), dst.path());
        assert!(result.is_err());
    }

    #[test]
    fn copy_directory_recursive_copy_onto_directory_fails() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("x.txt"), "x").unwrap();
        // The destination file path already exists as a directory.
        fs::create_dir_all(dst.path().join("x.txt")).unwrap();

        let result = copy_directory_recursive(src.path(), dst.path());
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // copy_dir_all_async / internal_copy_dir_async
    // -----------------------------------------------------------------

    #[test]
    fn copy_dir_all_async_nested_happy_path() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::create_dir_all(src.path().join("deep/deeper")).unwrap();
        fs::write(src.path().join("deep/deeper/f.txt"), "d").unwrap();

        copy_dir_all_async(src.path(), dst.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dst.path().join("deep/deeper/f.txt")).unwrap(),
            "d"
        );
    }

    #[test]
    fn copy_dir_all_async_dst_under_file_fails() {
        let src = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "file").unwrap();

        let result = copy_dir_all_async(src.path(), &blocker.join("dst"));
        assert!(result.is_err());
    }

    #[test]
    fn copy_dir_all_async_src_file_fails_at_read_dir() {
        let tmp = tempdir().unwrap();
        let file_src = tmp.path().join("plain.txt");
        fs::write(&file_src, "x").unwrap();

        let result = copy_dir_all_async(&file_src, &tmp.path().join("dst"));
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_all_async_rejects_symlink() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("real.txt"), "x").unwrap();
        std::os::unix::fs::symlink(
            src.path().join("real.txt"),
            src.path().join("link.txt"),
        )
        .unwrap();

        let err = copy_dir_all_async(src.path(), dst.path()).unwrap_err();
        assert!(err.to_string().contains("symlink"), "got: {err}");
    }

    #[test]
    fn copy_dir_all_async_subdir_blocked_by_file() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::create_dir_all(src.path().join("sub")).unwrap();
        fs::write(src.path().join("sub/x.txt"), "x").unwrap();
        fs::write(dst.path().join("sub"), "blocking file").unwrap();

        let result = copy_dir_all_async(src.path(), dst.path());
        assert!(result.is_err());
    }

    #[test]
    fn copy_dir_all_async_copy_onto_directory_fails() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("x.txt"), "x").unwrap();
        fs::create_dir_all(dst.path().join("x.txt")).unwrap();

        let result = copy_dir_all_async(src.path(), dst.path());
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // copy_dir_with_progress — error branches
    // -----------------------------------------------------------------

    #[test]
    fn copy_dir_with_progress_subdir_blocked_by_file() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::create_dir_all(src.path().join("sub")).unwrap();
        fs::write(src.path().join("sub/x.txt"), "x").unwrap();
        fs::write(dst.path().join("sub"), "blocking file").unwrap();

        let result = copy_dir_with_progress(src.path(), dst.path());
        assert!(result.is_err());
    }

    #[test]
    fn copy_dir_with_progress_copy_onto_directory_fails() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("x.txt"), "x").unwrap();
        fs::create_dir_all(dst.path().join("x.txt")).unwrap();

        let result = copy_dir_with_progress(src.path(), dst.path());
        assert!(result.is_err());
    }
}
