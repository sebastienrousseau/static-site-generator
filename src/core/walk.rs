// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Shared bounded directory walkers.
//!
//! Replaces the per-plugin `collect_*_files` helpers that previously
//! lived in nearly every module. Each function performs an iterative
//! (no-recursion) walk with optional bounds and returns a sorted
//! `Vec<PathBuf>` for deterministic test output.
//!
//! ## Variants
//!
//! - [`walk_files`] — single-extension filter, no bounds.
//! - [`walk_files_multi`] — multiple extensions (case-insensitive).
//! - [`walk_files_bounded_depth`] — single extension with a maximum
//!   directory depth (for content trees).
//! - [`walk_files_bounded_count`] — single extension with a maximum
//!   file-count cap (for live-reload / batch I/O fast-paths).
//!
//! All variants return `Ok(Vec::new())` when the root directory does
//! not exist or is not a directory — matching the convention used by
//! every previous local collector in the crate.
//!
//! ## Symlinks
//!
//! The collecting walkers follow symlinked directories, but enter each
//! real directory (by canonical path) at most once. A link back to an
//! ancestor is therefore skipped instead of walked again, so a symlink
//! loop can neither repeat files nor make the walk branch without end.
//! Subdirectories are visited in sorted order, so when one directory is
//! reachable by two names the name kept is the same on every platform.

use crate::error::{PathErrorExt, SsgError};
use std::collections::HashSet;
use std::ffi::OsString;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Recursively collects files matching `extension` under `dir`.
///
/// Sorted output, no recursion (uses an explicit stack), no depth or
/// count bounds. Returns `Ok(Vec::new())` if `dir` does not exist.
///
/// # Examples
///
/// ```rust
/// use ssg::walk::walk_files;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// fs::write(dir.path().join("a.md"), "").unwrap();
/// fs::write(dir.path().join("b.txt"), "").unwrap();
/// let mds = walk_files(dir.path(), "md").unwrap();
/// assert_eq!(mds.len(), 1);
/// ```
pub fn walk_files(
    dir: &Path,
    extension: &str,
) -> Result<Vec<PathBuf>, SsgError> {
    collect_files_matching(dir, usize::MAX, |p| has_extension(p, extension))
}

/// Recursively collects files matching any of `extensions` under `dir`.
///
/// Extension matching is **case-insensitive** so `IMG.JPG` and
/// `img.jpg` are both collected when `extensions` contains `"jpg"`.
/// Sorted output.
///
/// # Examples
///
/// ```rust
/// use ssg::walk::walk_files_multi;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// fs::write(dir.path().join("a.jpg"), "").unwrap();
/// fs::write(dir.path().join("B.PNG"), "").unwrap();
/// let imgs = walk_files_multi(dir.path(), &["jpg", "png"]).unwrap();
/// assert_eq!(imgs.len(), 2);
/// ```
pub fn walk_files_multi(
    dir: &Path,
    extensions: &[&str],
) -> Result<Vec<PathBuf>, SsgError> {
    collect_files_matching(dir, usize::MAX, |p| {
        has_any_extension_ignore_case(p, extensions)
    })
}

/// Visits every file with extension `ext` under `dir`, in a deterministic
/// order, without materialising the file list.
///
/// [`walk_files_bounded_depth`] returns a sorted `Vec<PathBuf>`. That is the
/// right shape when a caller needs the whole list, and the wrong one when
/// it only needs to see each path once: on a 10,000-page site the vector
/// alone is ~1.9 MiB and it is held for the entire pass. `emit_sidecars`
/// measured its peak heap at exactly that figure — the per-document work
/// never exceeded the list it was iterating (#578).
///
/// This walks depth-first, sorting each directory's entries by file name
/// before descending, so the visit order is identical to sorting the full
/// list — on every platform, since `read_dir` order is not portable — while
/// peak memory is one directory listing rather than the tree.
///
/// The callback's error stops the walk and is returned as-is.
///
/// # Errors
///
/// Returns the first I/O error from reading a directory, or the callback's.
pub fn visit_files_bounded_depth<E, F>(
    dir: &Path,
    ext: &str,
    max_depth: usize,
    mut visit: F,
) -> Result<(), E>
where
    E: From<std::io::Error>,
    F: FnMut(&Path) -> Result<(), E>,
{
    fn recurse<E, F>(
        dir: &Path,
        ext: &str,
        depth_left: usize,
        visit: &mut F,
    ) -> Result<(), E>
    where
        E: From<std::io::Error>,
        F: FnMut(&Path) -> Result<(), E>,
    {
        // Names only, not paths. A flat 10,000-file directory is one listing,
        // so what is held here *is* the walk's footprint: an `OsString` per
        // entry (~40 bytes) rather than a `PathBuf` (~190), and the full path
        // is built only for the entry being visited. Measured on the #578
        // fixture, holding paths here peaked *above* the collected Vec it
        // replaced.
        let mut names: Vec<(OsString, bool)> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let is_dir = entry.file_type()?.is_dir();
            names.push((entry.file_name(), is_dir));
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, is_dir) in names {
            let path = dir.join(&name);
            if is_dir {
                if depth_left > 0 {
                    recurse(&path, ext, depth_left - 1, visit)?;
                }
            } else if path.extension().is_some_and(|x| x == ext) {
                visit(&path)?;
            }
        }
        Ok(())
    }
    // A missing root is not an error, matching `walk_files_bounded_depth`:
    // `emit_sidecars` on a project with no content directory returns zero
    // sidecars, and callers rely on that. An *unreadable* root still errors,
    // also matching the collecting walk.
    if !dir.exists() {
        return Ok(());
    }
    recurse(dir, ext, max_depth, &mut visit)
}

/// Recursively collects files matching `extension`, bounded by depth.
///
/// Subdirectories beyond `max_depth` are silently skipped. Used by
/// content walkers that respect [`crate::MAX_DIR_DEPTH`] as a guard
/// against pathological symlink loops.
///
/// # Examples
///
/// ```rust
/// use ssg::walk::walk_files_bounded_depth;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// fs::write(dir.path().join("a.md"), "").unwrap();
/// let v = walk_files_bounded_depth(dir.path(), "md", 4).unwrap();
/// assert_eq!(v.len(), 1);
/// ```
pub fn walk_files_bounded_depth(
    dir: &Path,
    extension: &str,
    max_depth: usize,
) -> Result<Vec<PathBuf>, SsgError> {
    collect_files_matching(dir, max_depth, |p| has_extension(p, extension))
}

/// Collects every file under `dir` for which `matches` returns `true`,
/// descending at most `max_depth` directories below `dir`.
///
/// The one walk behind [`walk_files`], [`walk_files_multi`] and
/// [`walk_files_bounded_depth`]: iterative, sorted output, and each real
/// directory entered at most once (see the module's *Symlinks* notes).
///
/// # Errors
///
/// Returns the first I/O error from reading a directory or an entry.
pub(crate) fn collect_files_matching(
    dir: &Path,
    max_depth: usize,
    matches: impl Fn(&Path) -> bool,
) -> Result<Vec<PathBuf>, SsgError> {
    let mut files = Vec::new();
    let mut seen = SeenDirs::default();
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    while let Some((current, depth)) = stack.pop() {
        if depth > max_depth || !current.is_dir() || !seen.first_visit(&current)
        {
            continue;
        }
        let mut subdirs = Vec::new();
        for entry in fs::read_dir(&current).with_path(&current)? {
            let path = entry.with_path(&current)?.path();
            if path.is_dir() {
                subdirs.push(path);
            } else if matches(&path) {
                files.push(path);
            }
        }
        // Reverse-sorted onto the stack, so subdirectories pop in order.
        subdirs.sort_unstable_by(|a, b| b.cmp(a));
        stack.extend(subdirs.into_iter().map(|p| (p, depth + 1)));
    }
    files.sort();
    Ok(files)
}

/// The real directories a walk has already entered.
#[derive(Default)]
struct SeenDirs(HashSet<PathBuf>);

impl SeenDirs {
    /// Records `dir` and reports whether this is its first visit.
    ///
    /// A directory whose canonical path cannot be resolved is entered,
    /// so that reading it reports the underlying error as before.
    fn first_visit(&mut self, dir: &Path) -> bool {
        fs::canonicalize(dir).map_or(true, |real| self.0.insert(real))
    }
}

/// Whether `path` has exactly the extension `extension`.
fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|e| e == extension)
}

/// Whether `path`'s extension, lowercased, is one of `extensions`.
fn has_any_extension_ignore_case(path: &Path, extensions: &[&str]) -> bool {
    path.extension().is_some_and(|ext| {
        let lower = ext.to_string_lossy().to_lowercase();
        extensions.contains(&lower.as_str())
    })
}

/// Recursively collects files matching `extension`, capped at
/// `max_files`. Provides `with_context` on the underlying `read_dir`
/// failure.
///
/// Used by `livereload` (50 000 file cap) and similar fast-path
/// walkers that need a bounded latency upper bound.
///
/// # Examples
///
/// ```rust
/// use ssg::walk::walk_files_bounded_count;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// fs::write(dir.path().join("a.md"), "").unwrap();
/// fs::write(dir.path().join("b.md"), "").unwrap();
/// let v = walk_files_bounded_count(dir.path(), "md", 1).unwrap();
/// assert_eq!(v.len(), 1);
/// ```
pub fn walk_files_bounded_count(
    dir: &Path,
    extension: &str,
    max_files: usize,
) -> Result<Vec<PathBuf>, SsgError> {
    let mut files = Vec::new();
    let mut seen = SeenDirs::default();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        if files.len() >= max_files {
            break;
        }
        if !current.is_dir() || !seen.first_visit(&current) {
            continue;
        }
        let entries = fs::read_dir(&current).with_path(&current)?;
        for entry in entries {
            let path = entry.with_path(&current)?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == extension) {
                files.push(path);
                if files.len() >= max_files {
                    break;
                }
            }
        }
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    /// The streaming walk visits exactly what the collecting walk returns,
    /// in exactly that order. Files are created in deliberately
    /// non-alphabetical order across nested directories so a walk that
    /// merely reflected `read_dir` order would fail here.
    #[test]
    fn streaming_walk_matches_collected_order() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        for rel in [
            "zeta.md",
            "alpha.md",
            "sub/yak.md",
            "sub/ant.md",
            "mid.md",
            "sub/deep/omega.md",
            "sub/deep/beta.md",
            "note.txt",
        ] {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "x").unwrap();
        }
        let collected = walk_files_bounded_depth(root, "md", 8).unwrap();
        let mut streamed = Vec::new();
        visit_files_bounded_depth(
            root,
            "md",
            8,
            |p| -> Result<(), std::io::Error> {
                streamed.push(p.to_path_buf());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(streamed, collected);
        assert_eq!(streamed.len(), 7, "the .txt must be excluded");
    }

    use super::*;
    use tempfile::tempdir;

    // -------------------------------------------------------------------
    // walk_files
    // -------------------------------------------------------------------

    #[test]
    fn walk_files_returns_empty_for_missing_directory() {
        let dir = tempdir().unwrap();
        let result = walk_files(&dir.path().join("missing"), "html").unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    fn walk_files_filters_by_extension() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.html"), "").unwrap();
        fs::write(dir.path().join("b.css"), "").unwrap();
        fs::write(dir.path().join("c.js"), "").unwrap();

        let result = walk_files(dir.path(), "html").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with("a.html"));
    }

    #[test]
    fn walk_files_recurses_into_subdirectories() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("top.md"), "").unwrap();
        fs::write(nested.join("deep.md"), "").unwrap();

        let result = walk_files(dir.path(), "md").unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn walk_files_skips_extensionless_files() {
        // `path.extension()` returns `None` for a file with no dot in
        // its name, short-circuiting `is_some_and` without invoking
        // the comparison closure — a branch distinct from a
        // mismatched-extension file like `b.css` (covered above).
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("README"), "").unwrap();
        fs::write(dir.path().join("a.html"), "").unwrap();

        let result = walk_files(dir.path(), "html").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with("a.html"));
    }

    #[test]
    fn walk_files_returns_results_sorted() {
        let dir = tempdir().unwrap();
        for name in ["zebra.html", "apple.html", "mango.html"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let result = walk_files(dir.path(), "html").unwrap();
        let names: Vec<_> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["apple.html", "mango.html", "zebra.html"]);
    }

    // -------------------------------------------------------------------
    // walk_files_multi
    // -------------------------------------------------------------------

    #[test]
    fn walk_files_multi_collects_each_supplied_extension() {
        let dir = tempdir().unwrap();
        for name in ["a.jpg", "b.jpeg", "c.png", "d.gif", "e.txt"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let result =
            walk_files_multi(dir.path(), &["jpg", "jpeg", "png"]).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn walk_files_multi_extension_match_is_case_insensitive() {
        let dir = tempdir().unwrap();
        for name in ["A.JPG", "B.PNG", "C.JPEG"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let result =
            walk_files_multi(dir.path(), &["jpg", "jpeg", "png"]).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn walk_files_multi_skips_extensionless_files() {
        // Exercises the `None` arm of `if let Some(ext) = path.extension()`
        // — a file with no extension at all is silently skipped.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("README"), "").unwrap();
        fs::write(dir.path().join("a.jpg"), "").unwrap();

        let result = walk_files_multi(dir.path(), &["jpg"]).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with("a.jpg"));
    }

    #[test]
    fn walk_files_multi_returns_empty_for_missing_directory() {
        let dir = tempdir().unwrap();
        let result =
            walk_files_multi(&dir.path().join("missing"), &["jpg"]).unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    // -------------------------------------------------------------------
    // walk_files_bounded_depth
    // -------------------------------------------------------------------

    #[test]
    fn walk_files_bounded_depth_respects_max_depth() {
        let dir = tempdir().unwrap();
        let mut current = dir.path().to_path_buf();
        for i in 0..5 {
            current = current.join(format!("d{i}"));
            fs::create_dir_all(&current).unwrap();
            fs::write(current.join("p.md"), "").unwrap();
        }
        // max_depth=2 → only files at depths 0..=2 should be returned.
        let result = walk_files_bounded_depth(dir.path(), "md", 2).unwrap();
        assert!(result.len() <= 3);
    }

    #[test]
    fn walk_files_bounded_depth_skips_extensionless_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("README"), "").unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();

        let result = walk_files_bounded_depth(dir.path(), "md", 4).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with("a.md"));
    }

    #[test]
    fn walk_files_bounded_depth_returns_empty_for_missing_directory() {
        let dir = tempdir().unwrap();
        let result =
            walk_files_bounded_depth(&dir.path().join("missing"), "md", 8)
                .unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    // -------------------------------------------------------------------
    // walk_files_bounded_count
    // -------------------------------------------------------------------

    #[test]
    fn walk_files_bounded_count_respects_max_files() {
        let dir = tempdir().unwrap();
        for i in 0..10 {
            fs::write(dir.path().join(format!("f{i}.html")), "").unwrap();
        }
        let result = walk_files_bounded_count(dir.path(), "html", 5).unwrap();
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn walk_files_bounded_count_skips_extensionless_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("README"), "").unwrap();
        fs::write(dir.path().join("a.html"), "").unwrap();

        let result = walk_files_bounded_count(dir.path(), "html", 10).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with("a.html"));
    }

    #[test]
    fn walk_files_bounded_count_returns_empty_for_missing_directory() {
        let dir = tempdir().unwrap();
        let result =
            walk_files_bounded_count(&dir.path().join("missing"), "html", 100)
                .unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    fn walk_files_bounded_count_outer_loop_breaks_on_saturation() {
        // Files spread across two subdirectories so the outer-loop
        // saturation `break` fires (not the inner one).
        let dir = tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        for i in 0..3 {
            fs::write(a.join(format!("f{i}.html")), "").unwrap();
            fs::write(b.join(format!("f{i}.html")), "").unwrap();
        }
        let result = walk_files_bounded_count(dir.path(), "html", 2).unwrap();
        assert!(result.len() <= 4);
    }

    // -------------------------------------------------------------------
    // symlink loops (unix-only)
    // -------------------------------------------------------------------

    /// A tree with one file per level and two links back to the root,
    /// so a walk that follows links blindly branches at every level.
    #[cfg(unix)]
    fn tree_with_symlink_loops() -> tempfile::TempDir {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("b.md"), "").unwrap();
        fs::write(root.join("sub/a.md"), "").unwrap();
        symlink(root, root.join("sub/up1")).unwrap();
        symlink(root, root.join("sub/up2")).unwrap();
        dir
    }

    /// Runs `walk` on a thread and fails if it does not return promptly.
    #[cfg(unix)]
    fn relative_within_deadline(
        root: &Path,
        walk: fn(&Path) -> Vec<PathBuf>,
    ) -> Vec<PathBuf> {
        let (tx, rx) = std::sync::mpsc::channel();
        let walk_root = root.to_path_buf();
        let _ = std::thread::spawn(move || {
            let _ = tx.send(walk(&walk_root));
        });
        let found = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("walk did not finish within 10 s");
        found
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_path_buf())
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn collecting_walkers_enter_each_directory_once() {
        let dir = tree_with_symlink_loops();
        let expected = ["b.md", "sub/a.md"].map(PathBuf::from);
        let walkers: [fn(&Path) -> Vec<PathBuf>; 4] = [
            |r| walk_files(r, "md").unwrap(),
            |r| walk_files_multi(r, &["md"]).unwrap(),
            |r| walk_files_bounded_depth(r, "md", 128).unwrap(),
            |r| {
                let mut v = walk_files_bounded_count(r, "md", 1000).unwrap();
                v.sort();
                v
            },
        ];
        for walk in walkers {
            assert_eq!(relative_within_deadline(dir.path(), walk), expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn collecting_walk_still_follows_a_symlinked_directory() {
        use std::os::unix::fs::symlink;

        let outside = tempdir().unwrap();
        fs::write(outside.path().join("shared.md"), "").unwrap();
        let dir = tempdir().unwrap();
        symlink(outside.path(), dir.path().join("linked")).unwrap();

        let found = walk_files(dir.path(), "md").unwrap();
        assert_eq!(found, [dir.path().join("linked/shared.md")]);
    }

    // -------------------------------------------------------------------
    // read_dir error propagation (unreadable directory, unix-only)
    // -------------------------------------------------------------------

    #[cfg(unix)]
    fn with_unreadable_subdir<F: FnOnce(&Path)>(run: F) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        run(dir.path());

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_errors_on_unreadable_directory() {
        with_unreadable_subdir(|root| {
            let result = walk_files(root, "md");
            assert!(result.is_err(), "unreadable dir must error");
        });
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_multi_errors_on_unreadable_directory() {
        with_unreadable_subdir(|root| {
            let result = walk_files_multi(root, &["md"]);
            assert!(result.is_err(), "unreadable dir must error");
        });
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_bounded_depth_errors_on_unreadable_directory() {
        with_unreadable_subdir(|root| {
            let result = walk_files_bounded_depth(root, "md", 8);
            assert!(result.is_err(), "unreadable dir must error");
        });
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_bounded_count_errors_on_unreadable_directory() {
        with_unreadable_subdir(|root| {
            let result = walk_files_bounded_count(root, "md", 10);
            assert!(result.is_err(), "unreadable dir must error");
        });
    }
}
