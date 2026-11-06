// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Typed content collection API (issue #456).
//!
//! Mirrors the ergonomics of Astro's `getCollection` / `getEntry`
//! and Eleventy's collection helpers, but with **compile-time type
//! safety** via serde. Authors define a struct that derives
//! `serde::Deserialize`, then load every Markdown file under a
//! directory as `Vec<Entry<T>>` with one call.
//!
//! # Quick start
//!
//! ```no_run
//! use serde::Deserialize;
//! use ssg::collections::{get_collection, Entry};
//!
//! #[derive(Debug, Deserialize)]
//! struct BlogPost {
//!     title: String,
//!     date: String,
//!     description: Option<String>,
//!     #[serde(default)]
//!     tags: Vec<String>,
//! }
//!
//! # fn main() -> anyhow::Result<()> {
//! let posts: Vec<Entry<BlogPost>> =
//!     get_collection("content/blog")?;
//!
//! for post in posts {
//!     println!("{} ({})", post.data.title, post.slug);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Why typed?
//!
//! Hand-rolling frontmatter access via `serde_yml::Value` or string
//! lookups produces stringly-typed code that fails at runtime when a
//! field is renamed or its type changes. The typed API surfaces the
//! mismatch as a compile error or a clean `Result::Err` at load
//! time, with the file path in the error chain.
//!
//! # Loading semantics
//!
//! - **Walks recursively** under the given directory.
//! - **Markdown only** (`.md`, `.markdown`). Other files are skipped.
//! - **Skips files without frontmatter** silently — they're treated
//!   as plain pages outside the collection.
//! - **Returns parse errors with context**: each error carries the
//!   absolute path of the file that failed.
//! - **Slug derivation**: the slug is the file's `stem` (filename
//!   without extension). `index.md` files in subdirectories use the
//!   subdirectory name as the slug.
//! - **Deterministic ordering**: entries are returned sorted by
//!   slug so consumers that hash the result (e.g. for golden tests
//!   or perf benchmarks) get stable output.
//!
//! # Single-entry access
//!
//! [`get_entry`](crate::collections::get_entry) loads exactly one file by slug, returning
//! `Ok(None)` if no matching `.md` is found. Use this when a page
//! references another by its known slug (sidebar layouts, related
//! posts).

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::fs;
use std::path::{Path, PathBuf};

/// One parsed entry from a content collection.
///
/// `data` is the typed frontmatter (your struct), `body` is the raw
/// Markdown body (everything after the closing `---`). `slug` and
/// `path` give callers enough information to build URLs and
/// breadcrumbs without re-parsing the filename.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Entry<T> {
    /// Parsed frontmatter, deserialised into the caller's struct.
    pub data: T,
    /// Raw Markdown body (frontmatter delimiters stripped).
    pub body: String,
    /// URL-style slug derived from the filename.
    pub slug: String,
    /// Absolute path of the source file on disk.
    pub path: PathBuf,
}

/// Loads every Markdown file under `dir` whose frontmatter matches
/// `T`. Returns entries sorted by slug.
///
/// # Errors
///
/// - Returns the first I/O error encountered while walking the
///   directory.
/// - Returns the first frontmatter deserialisation error, with the
///   failing path in the error chain (`anyhow::Error::context`).
///
/// Files without a frontmatter delimiter are silently skipped — the
/// collection is for *structured* content, and pages without
/// frontmatter aren't part of the schema.
///
/// # Determinism
///
/// Output is sorted by `Entry::slug` (lexicographic). Callers that
/// hash collections for golden tests or fingerprinting benefit
/// directly.
///
/// # Examples
///
/// ```rust
/// use serde::Deserialize;
/// use ssg::collections::get_collection;
/// use tempfile::tempdir;
/// use std::fs;
///
/// #[derive(Debug, Deserialize)]
/// struct Post { title: String }
///
/// let dir = tempdir().unwrap();
/// fs::write(dir.path().join("a.md"), "---\ntitle: Hi\n---\nBody").unwrap();
/// let posts: Vec<_> = get_collection::<Post>(dir.path()).unwrap();
/// assert_eq!(posts.len(), 1);
/// assert_eq!(posts[0].data.title, "Hi");
/// ```
pub fn get_collection<T: DeserializeOwned>(
    dir: impl AsRef<Path>,
) -> Result<Vec<Entry<T>>> {
    let dir = dir.as_ref();
    let mut files = Vec::new();
    walk_markdown(dir, &mut files)?;
    files.sort();

    let mut out = Vec::with_capacity(files.len());
    for path in files {
        let entry = load_entry::<T>(&path)?;
        if let Some(e) = entry {
            out.push(e);
        }
    }

    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

/// Loads a single entry from `dir` whose slug matches `slug`.
///
/// Returns `Ok(None)` when no Markdown file with that slug exists.
/// Use [`get_collection`] when you need every entry or when you
/// don't know the slug ahead of time.
///
/// # Examples
///
/// ```rust
/// use serde::Deserialize;
/// use ssg::collections::get_entry;
/// use tempfile::tempdir;
/// use std::fs;
///
/// #[derive(Debug, Deserialize)]
/// struct Post { title: String }
///
/// let dir = tempdir().unwrap();
/// fs::write(dir.path().join("hello.md"), "---\ntitle: Hello\n---\nBody").unwrap();
/// let entry = get_entry::<Post>(dir.path(), "hello").unwrap();
/// assert!(entry.is_some());
/// assert!(get_entry::<Post>(dir.path(), "missing").unwrap().is_none());
/// ```
///
/// # Errors
///
/// Same as [`get_collection`].
pub fn get_entry<T: DeserializeOwned>(
    dir: impl AsRef<Path>,
    slug: &str,
) -> Result<Option<Entry<T>>> {
    let dir = dir.as_ref();
    let mut files = Vec::new();
    walk_markdown(dir, &mut files)?;

    for path in files {
        let candidate = derive_slug(&path, dir);
        if candidate == slug {
            return load_entry::<T>(&path);
        }
    }
    Ok(None)
}

/// Appends every `.md` / `.markdown` file under `dir`, sorted, to `out`.
///
/// Delegates to the shared walker, which bounds the depth at
/// [`crate::MAX_DIR_DEPTH`] and enters each real directory once, so a
/// symlink loop in the content tree cannot repeat or multiply entries.
fn walk_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    out.extend(crate::walk::collect_files_matching(
        dir,
        crate::MAX_DIR_DEPTH,
        |p| {
            p.extension().is_some_and(|e| {
                e.eq_ignore_ascii_case("md")
                    || e.eq_ignore_ascii_case("markdown")
            })
        },
    )?);
    Ok(())
}

fn load_entry<T: DeserializeOwned>(path: &Path) -> Result<Option<Entry<T>>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let Ok((fm, body)) = frontmatter_gen::extract(&raw) else {
        return Ok(None); // no frontmatter — not part of collection
    };
    let json_map = crate::frontmatter::frontmatter_into_json(fm);
    let json_value = serde_json::Value::Object(json_map.into_iter().collect());
    let data: T = serde_json::from_value(json_value).with_context(|| {
        format!("deserialize frontmatter from {}", path.display())
    })?;
    let dir_anchor = path.parent().unwrap_or(path);
    Ok(Some(Entry {
        data,
        body: body.to_string(),
        slug: derive_slug(path, dir_anchor),
        path: path.to_path_buf(),
    }))
}

/// Derives the URL-style slug from a file path:
///
/// - `posts/hello-world.md` → `hello-world`
/// - `posts/about/index.md` → `about` (parent dir name)
/// - `posts/index.md` → `index`
fn derive_slug(path: &Path, _dir: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if stem == "index" {
        if let Some(parent) = path.parent().and_then(Path::file_name) {
            return parent.to_string_lossy().to_string();
        }
    }
    stem
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use tempfile::tempdir;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Post {
        title: String,
        date: String,
        #[serde(default)]
        tags: Vec<String>,
    }

    fn write_post(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn derive_slug_uses_file_stem() {
        let p = PathBuf::from("posts/hello-world.md");
        assert_eq!(derive_slug(&p, Path::new("posts")), "hello-world");
    }

    #[test]
    fn derive_slug_index_uses_parent_dir() {
        let p = PathBuf::from("posts/about/index.md");
        assert_eq!(derive_slug(&p, Path::new("posts")), "about");
    }

    #[test]
    fn get_collection_loads_typed_entries() {
        let dir = tempdir().unwrap();
        // Inline YAML list — frontmatter-gen 0.0.5 doesn't support
        // the multi-line `- item` form for nested lists. Inline form
        // (`[rust, ssg]`) is the canonical short syntax it accepts.
        write_post(
            dir.path(),
            "first.md",
            "---\ntitle: First\ndate: 2026-01-01\ntags: [rust, ssg]\n---\nBody one.\n",
        );
        write_post(
            dir.path(),
            "second.md",
            "---\ntitle: Second\ndate: 2026-01-02\n---\nBody two.\n",
        );

        let posts: Vec<Entry<Post>> = get_collection(dir.path()).unwrap();
        assert_eq!(posts.len(), 2);
        // Sorted by slug.
        assert_eq!(posts[0].slug, "first");
        assert_eq!(posts[1].slug, "second");
        assert_eq!(posts[0].data.title, "First");
        assert!(posts[0].body.starts_with("Body one"));
    }

    #[test]
    fn get_collection_skips_files_without_frontmatter() {
        let dir = tempdir().unwrap();
        write_post(dir.path(), "naked.md", "# No frontmatter\n");
        write_post(
            dir.path(),
            "ok.md",
            "---\ntitle: x\ndate: 2026-01-01\n---\n",
        );
        let posts: Vec<Entry<Post>> = get_collection(dir.path()).unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].slug, "ok");
    }

    #[test]
    fn get_collection_recurses_into_subdirectories() {
        let dir = tempdir().unwrap();
        write_post(
            dir.path(),
            "a.md",
            "---\ntitle: A\ndate: 2026-01-01\n---\n",
        );
        write_post(
            dir.path(),
            "nested/b.md",
            "---\ntitle: B\ndate: 2026-01-02\n---\n",
        );
        let posts: Vec<Entry<Post>> = get_collection(dir.path()).unwrap();
        assert_eq!(posts.len(), 2);
    }

    #[test]
    fn get_collection_returns_error_with_path_context_on_bad_yaml() {
        let dir = tempdir().unwrap();
        write_post(
            dir.path(),
            "broken.md",
            "---\ntitle: 12\ndate: 2026-01-01\n---\n",
        );
        // `title` is required to be a String; passing 12 deserialises
        // ok actually because serde-yml coerces. Make a real type
        // mismatch:
        write_post(
            dir.path(),
            "bad.md",
            "---\ntitle:\n  - a list\ndate: 2026-01-01\n---\n",
        );
        let err = get_collection::<Post>(dir.path()).unwrap_err();
        let chain: String = err
            .chain()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            chain.contains("bad.md") || chain.contains("broken.md"),
            "expected file path in error chain, got: {chain}"
        );
    }

    #[test]
    fn get_entry_finds_by_slug() {
        let dir = tempdir().unwrap();
        write_post(
            dir.path(),
            "hello.md",
            "---\ntitle: H\ndate: 2026-01-01\n---\nbody\n",
        );
        let post: Option<Entry<Post>> = get_entry(dir.path(), "hello").unwrap();
        assert!(post.is_some());
        assert_eq!(post.unwrap().data.title, "H");
    }

    #[test]
    fn get_entry_returns_none_for_unknown_slug() {
        let dir = tempdir().unwrap();
        write_post(
            dir.path(),
            "exists.md",
            "---\ntitle: E\ndate: 2026-01-01\n---\n",
        );
        let post: Option<Entry<Post>> =
            get_entry(dir.path(), "missing").unwrap();
        assert!(post.is_none());
    }

    #[test]
    fn get_collection_empty_dir_returns_empty_vec() {
        let dir = tempdir().unwrap();
        let posts: Vec<Entry<Post>> = get_collection(dir.path()).unwrap();
        assert!(posts.is_empty());
    }

    #[test]
    fn get_collection_missing_dir_returns_empty_vec() {
        let posts: Vec<Entry<Post>> =
            get_collection("/nonexistent/path/here").unwrap();
        assert!(posts.is_empty());
    }

    #[test]
    fn derive_slug_root_index_falls_back_to_stem() {
        // "index.md" at the tree root: parent().file_name() is None,
        // so the parent-name branch is skipped and the stem is used.
        let p = PathBuf::from("index.md");
        assert_eq!(derive_slug(&p, Path::new("")), "index");
    }

    #[test]
    fn walk_markdown_accepts_markdown_extension_and_skips_others() {
        let dir = tempdir().unwrap();
        write_post(
            dir.path(),
            "long.markdown",
            "---\ntitle: L\ndate: 2026-01-01\n---\nBody\n",
        );
        write_post(dir.path(), "notes.txt", "not collected");

        let posts: Vec<Entry<Post>> = get_collection(dir.path()).unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].slug, "long");
    }

    /// Lists what `walk_markdown` finds under `root`, relative to it.
    #[cfg(unix)]
    fn walk_relative(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        walk_markdown(root, &mut files).unwrap();
        files
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_path_buf())
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn walk_markdown_does_not_follow_a_symlink_loop() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        for rel in ["zeta.md", "alpha.md", "sub/deep/beta.markdown"] {
            fs::write(root.join(rel), "x").unwrap();
        }
        // `sub/loop` points back at the content root.
        symlink(root, root.join("sub/loop")).unwrap();

        assert_eq!(
            walk_relative(root),
            ["alpha.md", "sub/deep/beta.markdown", "zeta.md"]
                .map(PathBuf::from),
            "each file once, sorted, nothing reached through the loop"
        );
    }

    #[cfg(unix)]
    #[test]
    fn walk_markdown_terminates_on_branching_symlink_loops() {
        use std::os::unix::fs::symlink;
        use std::sync::mpsc;
        use std::time::Duration;

        // Two links back to the root in one directory make the
        // followed tree branch at every level: 2^n paths at depth n.
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.md"), "x").unwrap();
        symlink(&root, root.join("l1")).unwrap();
        symlink(&root, root.join("l2")).unwrap();

        let (tx, rx) = mpsc::channel();
        let walk_root = root.clone();
        let _ = std::thread::spawn(move || {
            let _ = tx.send(walk_relative(&walk_root));
        });
        let found = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("walk did not finish within 10 s");
        assert_eq!(found, [PathBuf::from("a.md")]);
    }

    #[test]
    fn load_entry_read_failure_carries_path_context() {
        let err = load_entry::<Post>(Path::new(
            "/nonexistent-ssg-collections/missing.md",
        ))
        .unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("read /nonexistent-ssg-collections"),
            "got: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn get_collection_propagates_unreadable_root_dir_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let content = dir.path().join("locked");
        fs::create_dir_all(&content).unwrap();
        fs::set_permissions(&content, fs::Permissions::from_mode(0o000))
            .unwrap();

        let result = get_collection::<Post>(&content);

        fs::set_permissions(&content, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(result.is_err(), "unreadable dir must error");
    }

    #[cfg(unix)]
    #[test]
    fn get_collection_propagates_nested_unreadable_dir_error() {
        use std::os::unix::fs::PermissionsExt;

        // Drives the recursive walk_markdown `?` error path.
        let dir = tempdir().unwrap();
        let nested = dir.path().join("sub");
        fs::create_dir_all(&nested).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o000))
            .unwrap();

        let result = get_collection::<Post>(dir.path());

        fs::set_permissions(&nested, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(result.is_err(), "nested unreadable dir must error");
    }

    #[cfg(unix)]
    #[test]
    fn get_entry_propagates_unreadable_root_dir_error() {
        // `get_entry` has its own `walk_markdown(dir, &mut files)?`
        // call site, distinct from `get_collection`'s. Region coverage
        // is tracked per call site, so the error arm here must be
        // exercised independently of the `get_collection` equivalent.
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let content = dir.path().join("locked");
        fs::create_dir_all(&content).unwrap();
        fs::set_permissions(&content, fs::Permissions::from_mode(0o000))
            .unwrap();

        let result = get_entry::<Post>(&content, "whatever");

        fs::set_permissions(&content, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(result.is_err(), "unreadable dir must error");
    }

    #[test]
    fn get_entry_propagates_frontmatter_type_error() {
        let dir = tempdir().unwrap();
        write_post(
            dir.path(),
            "bad.md",
            "---\ntitle:\n  - a list\ndate: 2026-01-01\n---\n",
        );
        let result = get_entry::<Post>(dir.path(), "bad");
        assert!(result.is_err(), "type mismatch must propagate");
    }
}
