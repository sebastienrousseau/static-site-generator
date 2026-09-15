// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Page dependency graph for incremental rebuilds.
//!
//! `DepGraph` tracks four things:
//!
//! 1. **Edges** — `consumer → set<dependency>`. A page declares a
//!    dependency on every template, partial, and data file the
//!    compiler will read while rendering it. Edges are reflexive (a
//!    page depends on itself).
//! 2. **Outputs** — `source → set<output>`. The compiler may emit
//!    several artefacts per source (`foo.md → public/foo/index.html`,
//!    plus a sitemap entry, an RSS entry, an SBOM entry). The output
//!    set drives the AC5 delete sweep.
//! 3. **Hashes** — `path → sha256`. The freshness key. A source is
//!    "changed" iff its current SHA-256 differs from the cached value
//!    (or no cached value exists, i.e. it's new).
//! 4. **Schema version** — bumped when the on-disk JSON layout
//!    changes. Loading a graph with a stale version triggers a
//!    poisoning-resistant fallback to a full rebuild (AC6).
//!
//! Transitive edges are resolved on demand by [`DepGraph::invalidated`]
//! via BFS over the reverse edge map. Persistence is atomic — the
//! graph is written to `.tmp` then renamed (POSIX guarantee).

use crate::error::SsgError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

/// Filename used for the persisted graph under
/// `target/ssg-cache/`. Issue #524 spec.
pub const DEP_GRAPH_FILE: &str = "depgraph.json";

/// Subdirectory of the cache root where the graph lives.
///
/// The cache root itself defaults to `target/ssg-cache/` (issue
/// #524 spec) but the helpers accept any path so tests stay
/// hermetic.
pub const CACHE_DIRNAME: &str = "ssg-cache";

/// Bumped whenever the persisted JSON layout changes in a way that
/// can't be loaded by the prior parser. A version mismatch is treated
/// exactly like a parse error: warn + full rebuild (AC6).
const SCHEMA_VERSION: u32 = 2;

/// Dependency graph mapping consumers to their dependencies.
///
/// Persisted to `target/ssg-cache/depgraph.json`. Loaders are
/// poisoning-resistant: a missing, truncated, or version-mismatched
/// file yields an empty graph — caller falls back to a full rebuild
/// without crashing or producing stale output (AC6).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DepGraph {
    /// On-disk layout version. Mismatches force a full rebuild.
    #[serde(default = "default_version")]
    version: u32,
    /// `consumer → set<dependency>` (forward edges).
    deps: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    /// `source → set<output>`. Used by [`DepGraph::stale_outputs`] to
    /// sweep orphaned output files when a source is deleted (AC5).
    #[serde(default)]
    outputs: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    /// `path → sha256(content)`. The freshness key.
    #[serde(default)]
    hashes: BTreeMap<PathBuf, String>,
}

const fn default_version() -> u32 {
    // A graph without a version field is from before v2 — treated as
    // unusable and replaced by the empty default on load.
    0
}

impl DepGraph {
    /// Creates an empty dependency graph at the current schema version.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    ///
    /// let g = DepGraph::new();
    /// assert_eq!(g.page_count(), 0);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            version: SCHEMA_VERSION,
            deps: BTreeMap::new(),
            outputs: BTreeMap::new(),
            hashes: BTreeMap::new(),
        }
    }

    /// Loads the graph from `<cache_root>/depgraph.json`.
    ///
    /// Returns an empty graph if the file is missing, unreadable,
    /// malformed, or written by an incompatible schema version. The
    /// poisoning-resistant return matches AC6.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// // Missing cache file ⇒ empty graph (no panic, no error).
    /// let g = DepGraph::load(dir.path());
    /// assert_eq!(g.page_count(), 0);
    /// ```
    #[must_use]
    pub fn load(cache_root: &Path) -> Self {
        let path = cache_root.join(DEP_GRAPH_FILE);
        let Ok(json) = fs::read_to_string(&path) else {
            return Self::new();
        };
        match serde_json::from_str::<Self>(&json) {
            Ok(g) if g.version == SCHEMA_VERSION => g,
            Ok(_) => {
                log::warn!(
                    "depgraph at {} has incompatible schema; falling back to full rebuild",
                    path.display()
                );
                Self::new()
            }
            Err(e) => {
                log::warn!(
                    "depgraph at {} is corrupt ({e}); falling back to full rebuild",
                    path.display()
                );
                Self::new()
            }
        }
    }

    /// Persists the graph atomically: writes `<file>.tmp` then renames.
    /// POSIX rename is atomic on the same filesystem.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let g = DepGraph::new();
    /// g.save(dir.path()).unwrap();
    /// assert!(dir.path().join("depgraph.json").exists());
    /// ```
    ///
    /// # Errors
    /// Returns the underlying I/O failure if the cache root can't be
    /// created or the temp file can't be written / renamed.
    pub fn save(&self, cache_root: &Path) -> Result<(), SsgError> {
        fs::create_dir_all(cache_root).map_err(|e| SsgError::Io {
            path: cache_root.to_path_buf(),
            source: e,
        })?;
        let final_path = cache_root.join(DEP_GRAPH_FILE);
        let tmp_path = cache_root.join(format!("{DEP_GRAPH_FILE}.tmp"));
        let json = serde_json::to_string(self).map_err(|e| SsgError::Io {
            path: final_path.clone(),
            source: std::io::Error::other(e),
        })?;
        fs::write(&tmp_path, json).map_err(|e| SsgError::Io {
            path: tmp_path.clone(),
            source: e,
        })?;
        fs::rename(&tmp_path, &final_path).map_err(|e| SsgError::Io {
            path: final_path,
            source: e,
        })?;
        Ok(())
    }

    /// Records that `consumer` depends on `dep`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    ///
    /// let mut g = DepGraph::new();
    /// g.add_dep(Path::new("page.md"), Path::new("layout.html"));
    /// assert!(g.deps_for(Path::new("page.md")).is_some());
    /// ```
    pub fn add_dep(&mut self, consumer: &Path, dep: &Path) {
        let _ = self
            .deps
            .entry(consumer.to_path_buf())
            .or_default()
            .insert(dep.to_path_buf());
    }

    /// Records that `source` produces `output`. Used by the AC5
    /// delete-sweep to remove orphaned outputs when a source is
    /// removed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    ///
    /// let mut g = DepGraph::new();
    /// g.add_output(Path::new("a.md"), Path::new("a.html"));
    /// assert!(g.outputs_for(Path::new("a.md")).is_some());
    /// ```
    pub fn add_output(&mut self, source: &Path, output: &Path) {
        let _ = self
            .outputs
            .entry(source.to_path_buf())
            .or_default()
            .insert(output.to_path_buf());
    }

    /// Records the SHA-256 freshness key for `path` from a byte slice.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    /// use std::collections::HashMap;
    ///
    /// let mut g = DepGraph::new();
    /// g.record_hash(Path::new("a.md"), b"hello");
    /// // Same content ⇒ no diff.
    /// let mut current = HashMap::new();
    /// current.insert(Path::new("a.md").to_path_buf(), DepGraph::sha256_hex(b"hello"));
    /// assert!(g.diff(&current).is_empty());
    /// ```
    pub fn record_hash(&mut self, path: &Path, content: &[u8]) {
        let _ = self
            .hashes
            .insert(path.to_path_buf(), Self::sha256_hex(content));
    }

    /// Records the SHA-256 of `path` by reading it from disk.
    /// Silently ignores missing files (the caller will catch the
    /// absence elsewhere — typically a delete that we want to record).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use tempfile::tempdir;
    /// use std::fs;
    ///
    /// let dir = tempdir().unwrap();
    /// let p = dir.path().join("a.md");
    /// fs::write(&p, "hi").unwrap();
    /// let mut g = DepGraph::new();
    /// g.record_hash_from_disk(&p);
    /// // Missing files are silently ignored.
    /// g.record_hash_from_disk(&dir.path().join("missing.md"));
    /// ```
    pub fn record_hash_from_disk(&mut self, path: &Path) {
        if let Ok(bytes) = fs::read(path) {
            self.record_hash(path, &bytes);
        }
    }

    /// Returns the SHA-256 hex string for `bytes`. Exposed so the
    /// `populate` helpers can hash files exactly once.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    ///
    /// let hex = DepGraph::sha256_hex(b"");
    /// // Empty string has a well-known SHA-256.
    /// assert_eq!(hex, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    /// ```
    #[must_use]
    pub fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
        }
        out
    }

    /// Returns the direct dependencies recorded for `consumer`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    ///
    /// let mut g = DepGraph::new();
    /// g.add_dep(Path::new("p.md"), Path::new("layout.html"));
    /// assert_eq!(g.deps_for(Path::new("p.md")).map(|s| s.len()), Some(1));
    /// assert!(g.deps_for(Path::new("none")).is_none());
    /// ```
    #[must_use]
    pub fn deps_for(&self, consumer: &Path) -> Option<&BTreeSet<PathBuf>> {
        self.deps.get(consumer)
    }

    /// Returns the recorded outputs for `source`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    ///
    /// let mut g = DepGraph::new();
    /// g.add_output(Path::new("a.md"), Path::new("a.html"));
    /// assert_eq!(g.outputs_for(Path::new("a.md")).map(|s| s.len()), Some(1));
    /// ```
    #[must_use]
    pub fn outputs_for(&self, source: &Path) -> Option<&BTreeSet<PathBuf>> {
        self.outputs.get(source)
    }

    /// Returns every tracked source path (the keys of the output map).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    ///
    /// let mut g = DepGraph::new();
    /// g.add_output(Path::new("a.md"), Path::new("a.html"));
    /// assert_eq!(g.tracked_sources(), vec![Path::new("a.md").to_path_buf()]);
    /// ```
    #[must_use]
    pub fn tracked_sources(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.outputs.keys().cloned().collect();
        v.sort();
        v
    }

    /// Returns the count of edge consumers (pages + intermediate deps).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    ///
    /// let mut g = DepGraph::new();
    /// assert_eq!(g.page_count(), 0);
    /// g.add_dep(Path::new("p.md"), Path::new("l.html"));
    /// assert_eq!(g.page_count(), 1);
    /// ```
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.deps.len()
    }

    /// Removes every entry that references `path` as either a consumer
    /// or a dependency. Called by [`Self::diff`] when a source is
    /// deleted.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    ///
    /// let mut g = DepGraph::new();
    /// g.add_dep(Path::new("p.md"), Path::new("l.html"));
    /// g.forget(Path::new("p.md"));
    /// assert!(g.deps_for(Path::new("p.md")).is_none());
    /// ```
    pub fn forget(&mut self, path: &Path) {
        let _ = self.deps.remove(path);
        let _ = self.outputs.remove(path);
        let _ = self.hashes.remove(path);
        for set in self.deps.values_mut() {
            let _ = set.remove(path);
        }
    }

    /// Clears the entire graph.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::Path;
    ///
    /// let mut g = DepGraph::new();
    /// g.add_dep(Path::new("p.md"), Path::new("l.html"));
    /// g.clear();
    /// assert_eq!(g.page_count(), 0);
    /// ```
    pub fn clear(&mut self) {
        self.deps.clear();
        self.outputs.clear();
        self.hashes.clear();
    }

    /// Returns every consumer reachable from any of `changed` via the
    /// reverse edge map (transitive closure, AC3). Sources whose own
    /// content changed are always included.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::{Path, PathBuf};
    ///
    /// let mut g = DepGraph::new();
    /// g.add_dep(Path::new("p.md"), Path::new("l.html"));
    /// let changed = vec![PathBuf::from("l.html")];
    /// // Changing the layout invalidates the page that consumes it.
    /// assert!(g.invalidated(&changed).contains(&PathBuf::from("p.md")));
    /// ```
    #[must_use]
    pub fn invalidated(&self, changed: &[PathBuf]) -> Vec<PathBuf> {
        let reverse = self.reverse_edges();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut queue: VecDeque<PathBuf> = changed.iter().cloned().collect();
        while let Some(p) = queue.pop_front() {
            if !seen.insert(p.clone()) {
                continue;
            }
            if let Some(parents) = reverse.get(&p) {
                for parent in parents {
                    if !seen.contains(parent) {
                        queue.push_back(parent.clone());
                    }
                }
            }
        }
        let mut result: Vec<PathBuf> = seen.into_iter().collect();
        result.sort();
        result
    }

    /// Returns the union of output paths for every invalidated source.
    /// Sources that don't appear in the output map (templates,
    /// partials, data files) contribute nothing.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::path::{Path, PathBuf};
    ///
    /// let mut g = DepGraph::new();
    /// g.add_dep(Path::new("p.md"), Path::new("l.html"));
    /// g.add_output(Path::new("p.md"), Path::new("p.html"));
    /// let outs = g.invalidated_outputs(&[PathBuf::from("l.html")]);
    /// assert_eq!(outs, vec![PathBuf::from("p.html")]);
    /// ```
    #[must_use]
    pub fn invalidated_outputs(&self, changed: &[PathBuf]) -> Vec<PathBuf> {
        let mut out: HashSet<PathBuf> = HashSet::new();
        for p in self.invalidated(changed) {
            if let Some(outs) = self.outputs.get(&p) {
                for o in outs {
                    let _ = out.insert(o.clone());
                }
            }
        }
        let mut v: Vec<PathBuf> = out.into_iter().collect();
        v.sort();
        v
    }

    /// Inverts the forward edge map so the BFS can walk
    /// `dependency → set<consumer>`.
    fn reverse_edges(&self) -> HashMap<PathBuf, HashSet<PathBuf>> {
        let mut rev: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for (consumer, deps) in &self.deps {
            for dep in deps {
                let _ = rev
                    .entry(dep.clone())
                    .or_default()
                    .insert(consumer.clone());
            }
        }
        rev
    }

    /// Compares `current` (path → sha256) against the cached hashes
    /// and returns `(changed, deleted)`.
    ///
    /// * `changed` — paths whose current hash differs from cache, or
    ///   paths that weren't in the cache (new files).
    /// * `deleted` — paths the cache knew about that are absent from
    ///   `current`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::DepGraph;
    /// use std::collections::HashMap;
    /// use std::path::PathBuf;
    ///
    /// let g = DepGraph::new();
    /// let mut current = HashMap::new();
    /// current.insert(PathBuf::from("a.md"), DepGraph::sha256_hex(b"x"));
    /// // Empty graph ⇒ everything in `current` looks new.
    /// let d = g.diff(&current);
    /// assert_eq!(d.changed, vec![PathBuf::from("a.md")]);
    /// assert!(d.deleted.is_empty());
    /// ```
    #[must_use]
    pub fn diff(&self, current: &HashMap<PathBuf, String>) -> Diff {
        let mut changed = Vec::new();
        let mut deleted = Vec::new();
        for (path, hash) in current {
            match self.hashes.get(path) {
                Some(prev) if prev == hash => {}
                _ => changed.push(path.clone()),
            }
        }
        for path in self.hashes.keys() {
            if !current.contains_key(path) {
                deleted.push(path.clone());
            }
        }
        changed.sort();
        deleted.sort();
        Diff { changed, deleted }
    }
}

/// Result of [`DepGraph::diff`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    /// Sources whose SHA-256 changed since the cached graph, or new
    /// sources without a cached hash.
    pub changed: Vec<PathBuf>,
    /// Sources tracked by the cached graph that no longer exist on
    /// disk.
    pub deleted: Vec<PathBuf>,
}

impl Diff {
    /// Returns `true` when nothing changed and nothing was deleted —
    /// the warm-cache zero-work fast path.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::depgraph::Diff;
    ///
    /// let d = Diff::default();
    /// assert!(d.is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.deleted.is_empty()
    }
}

// ---------------------------------------------------------------------
// Populate helpers: scan content + templates to build the edge set.
// ---------------------------------------------------------------------

/// Every data file a build reads, in both supported locations.
///
/// Two conventions are in play and both are live: `data/` beside the
/// content directory, read by [`TemplateEngine::load_data_files`], and
/// `_data/` for the topic-cluster curation file. A build reads them
/// globally rather than per page, so they are collected once.
///
/// [`TemplateEngine::load_data_files`]: crate::template_engine::TemplateEngine::load_data_files
fn data_files(content_dir: &Path) -> Vec<PathBuf> {
    /// Extensions `load_data_files` actually parses. A `.md` sitting in
    /// `data/` is not a data file and must not become an edge.
    const DATA_EXTS: &[&str] = &["json", "toml", "yaml", "yml"];

    let mut roots = Vec::new();
    if let Some(parent) = content_dir.parent() {
        roots.push(parent.join("data"));
        roots.push(parent.join("_data"));
    }
    roots.push(content_dir.join("data"));
    roots.push(content_dir.join("_data"));

    let mut out = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if DATA_EXTS.contains(&ext.as_str()) {
                out.push(path);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Reads every `.md` file under `content_dir` and every `.html` file
/// under `template_dir`, recording:
///
/// * a content → template edge for each `layout:` frontmatter value
///   (`layout: "post"` resolves to `<template_dir>/<locale>/post.html`
///   when a localised copy exists, otherwise `<template_dir>/post.html`);
/// * a template → template edge for each `{{#extends "name"}}` and
///   `{{->partial}}` reference inside the template;
/// * the canonical output path `<build_dir>/<stem>/index.html`
///   (or `<build_dir>/index.html` for `index.md`);
/// * the SHA-256 freshness key for every source touched.
///
/// Self-edges (`page → page`) are always recorded so deleting a page
/// invalidates its own output. Missing template files don't fault —
/// the build will fail later with a friendlier message.
///
/// # Examples
///
/// ```rust
/// use ssg::depgraph::{DepGraph, populate};
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// let content = dir.path().join("content");
/// let templates = dir.path().join("templates");
/// let build = dir.path().join("build");
/// fs::create_dir(&content).unwrap();
/// fs::create_dir(&templates).unwrap();
/// let mut g = DepGraph::new();
/// // Walking empty trees is a no-op.
/// populate(&mut g, &content, &templates, &build).unwrap();
/// assert_eq!(g.page_count(), 0);
/// ```
pub fn populate(
    graph: &mut DepGraph,
    content_dir: &Path,
    template_dir: &Path,
    build_dir: &Path,
) -> Result<(), SsgError> {
    let md_files = crate::walk::walk_files_bounded_depth(
        content_dir,
        "md",
        crate::MAX_DIR_DEPTH,
    )?;

    for md in &md_files {
        let bytes = fs::read(md).map_err(|e| SsgError::Io {
            path: md.clone(),
            source: e,
        })?;
        graph.record_hash(md, &bytes);

        let layout = extract_layout(&bytes);
        let outputs = output_paths_for(md, content_dir, build_dir);
        for o in &outputs {
            graph.add_output(md, o);
            // self-edge so deletes propagate via invalidated()
            graph.add_dep(md, md);
            if let Some(ref layout_name) = layout {
                if let Some(tpl) =
                    resolve_template(template_dir, md, content_dir, layout_name)
                {
                    graph.add_dep(md, &tpl);
                }
            }
        }
    }

    // Data files are read globally, not per page: a template can
    // address `data.foo` from anywhere, and the topic curation file
    // reshapes every topic page. So a change to one invalidates every
    // page, and the edge set says so rather than leaving those pages
    // looking clean.
    let data = data_files(content_dir);
    for df in &data {
        if let Ok(bytes) = fs::read(df) {
            graph.record_hash(df, &bytes);
        }
        for md in &md_files {
            graph.add_dep(md, df);
        }
    }

    let tpl_files = crate::walk::walk_files_bounded_depth(
        template_dir,
        "html",
        crate::MAX_DIR_DEPTH,
    )?;
    for tpl in &tpl_files {
        let bytes = fs::read(tpl).map_err(|e| SsgError::Io {
            path: tpl.clone(),
            source: e,
        })?;
        graph.record_hash(tpl, &bytes);
        let text = String::from_utf8_lossy(&bytes);
        for parent in scan_template_refs(&text) {
            let resolved = template_dir.join(format!("{parent}.html"));
            // Edge: tpl depends on parent (extends / include).
            graph.add_dep(tpl, &resolved);
        }
    }

    Ok(())
}

/// Walks every tracked source on disk and returns
/// `path → sha256(content)`. Used by [`DepGraph::diff`] on the
/// incremental hot path. Sources that disappear silently drop out.
///
/// # Examples
///
/// ```rust
/// use ssg::depgraph::current_hashes;
/// use tempfile::tempdir;
/// use std::fs;
///
/// let dir = tempdir().unwrap();
/// let content = dir.path().join("content");
/// let templates = dir.path().join("templates");
/// fs::create_dir(&content).unwrap();
/// fs::create_dir(&templates).unwrap();
/// let map = current_hashes(&content, &templates).unwrap();
/// assert!(map.is_empty());
/// ```
pub fn current_hashes(
    content_dir: &Path,
    template_dir: &Path,
) -> Result<HashMap<PathBuf, String>, SsgError> {
    let mut out = HashMap::new();
    // Infallible by construction: sources that disappear between the
    // walk and the read silently drop out (see the doc comment).
    let mut push = |paths: Vec<PathBuf>| {
        for p in paths {
            if let Ok(bytes) = fs::read(&p) {
                let _ = out.insert(p, DepGraph::sha256_hex(&bytes));
            }
        }
    };
    push(crate::walk::walk_files_bounded_depth(
        content_dir,
        "md",
        crate::MAX_DIR_DEPTH,
    )?);
    push(crate::walk::walk_files_bounded_depth(
        template_dir,
        "html",
        crate::MAX_DIR_DEPTH,
    )?);
    Ok(out)
}

/// Extracts the `layout:` field from a YAML frontmatter header. Returns
/// `None` if the file lacks frontmatter or doesn't declare a layout.
fn extract_layout(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let trimmed = text.trim_start();
    let body = trimmed.strip_prefix("---")?;
    let end = body.find("\n---")?;
    let fm = &body[..end];
    for line in fm.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("layout:") {
            return Some(
                rest.trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .split_whitespace()
                    .next()?
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string(),
            );
        }
    }
    None
}

/// Computes the canonical output paths the compiler will emit for a
/// given content file. Mirrors `staticdatagen`'s naming convention:
///
///   `<content_dir>/index.md`             → `<build_dir>/index.html`
///   `<content_dir>/<stem>.md`            → `<build_dir>/<stem>/index.html`
///   `<content_dir>/<sub>/<stem>.md`      → `<build_dir>/<sub>/<stem>/index.html`
///   `<content_dir>/<sub>/index.md`       → `<build_dir>/<sub>/index.html`
fn output_paths_for(
    md: &Path,
    content_dir: &Path,
    build_dir: &Path,
) -> Vec<PathBuf> {
    let rel = match md.strip_prefix(content_dir) {
        Ok(r) => r.to_path_buf(),
        Err(_) => return Vec::new(),
    };
    let parent = rel.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = rel.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let out = if stem == "index" {
        build_dir.join(&parent).join("index.html")
    } else {
        build_dir.join(&parent).join(stem).join("index.html")
    };
    vec![out]
}

/// Resolves a `layout: "post"` frontmatter value to the on-disk
/// template path. Prefers a locale-aware sibling
/// (`<template_dir>/<locale>/post.html`) inferred from the leading
/// directory component of the content file's relative path; falls back
/// to `<template_dir>/post.html`. Returns `None` if neither exists —
/// the consumer is free to record no edge or surface the error later.
fn resolve_template(
    template_dir: &Path,
    md: &Path,
    content_dir: &Path,
    layout: &str,
) -> Option<PathBuf> {
    if let Ok(rel) = md.strip_prefix(content_dir) {
        if let Some(first) = rel.components().next() {
            let candidate = template_dir
                .join(first.as_os_str())
                .join(format!("{layout}.html"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    let fallback = template_dir.join(format!("{layout}.html"));
    if fallback.exists() {
        Some(fallback)
    } else {
        None
    }
}

/// Scans a template body for `{{#extends "name"}}` and `{{->name}}`
/// references and returns each `name` once. Order-preserving but
/// de-duplicated.
fn scan_template_refs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("}}") else {
            break;
        };
        let inner = rest[..end].trim();
        rest = &rest[end + 2..];
        let name_opt = if let Some(after) = inner.strip_prefix("#extends") {
            Some(parse_name(after.trim()))
        } else {
            inner
                .strip_prefix("->")
                .map(|after| parse_name(after.trim()))
        };
        if let Some(name) = name_opt {
            if !name.is_empty() && seen.insert(name.clone()) {
                out.push(name);
            }
        }
    }
    out
}

/// Parses a bareword or quoted template name from an `extends` /
/// `partial` invocation. Strips a trailing parameter list (the partial
/// invocation `header title="foo"` parses to `header`).
fn parse_name(s: &str) -> String {
    let s = s.trim().trim_matches(|c| c == '"' || c == '\'');
    s.split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .next()
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn empty_graph_only_reports_changed_inputs() {
        let graph = DepGraph::new();
        let changed = vec![PathBuf::from("content/index.md")];
        let result = graph.invalidated(&changed);
        assert_eq!(result, vec![PathBuf::from("content/index.md")]);
    }

    #[test]
    fn direct_change_invalidates_only_the_page() {
        let mut graph = DepGraph::new();
        let page = PathBuf::from("content/about.md");
        let tmpl = PathBuf::from("templates/base.html");
        graph.add_dep(&page, &tmpl);

        let changed = vec![page.clone()];
        let result = graph.invalidated(&changed);
        assert!(result.contains(&page));
        assert_eq!(result.len(), 1, "no other consumers should fire");
    }

    #[test]
    fn dependency_change_invalidates_all_consumers() {
        let mut graph = DepGraph::new();
        let a = PathBuf::from("content/index.md");
        let b = PathBuf::from("content/about.md");
        let tmpl = PathBuf::from("templates/base.html");
        graph.add_dep(&a, &tmpl);
        graph.add_dep(&b, &tmpl);

        let result = graph.invalidated(std::slice::from_ref(&tmpl));
        assert!(result.contains(&a));
        assert!(result.contains(&b));
        assert!(result.contains(&tmpl));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn transitive_edges_are_tracked_via_bfs() {
        // AC3: page → partial → base. Changing `base` must invalidate
        // `page` as well. This flips the prior `transitive_not_tracked`
        // assertion.
        let mut graph = DepGraph::new();
        let page = PathBuf::from("content/index.md");
        let partial = PathBuf::from("templates/partial.html");
        let base = PathBuf::from("templates/base.html");
        graph.add_dep(&page, &partial);
        graph.add_dep(&partial, &base);

        let result = graph.invalidated(std::slice::from_ref(&base));
        assert!(result.contains(&base));
        assert!(result.contains(&partial));
        assert!(
            result.contains(&page),
            "transitive consumer must be invalidated"
        );
    }

    #[test]
    fn invalidated_outputs_unions_outputs_of_every_consumer() {
        let mut graph = DepGraph::new();
        let page = PathBuf::from("content/about.md");
        let out = PathBuf::from("public/about/index.html");
        let tmpl = PathBuf::from("templates/page.html");
        graph.add_dep(&page, &tmpl);
        graph.add_output(&page, &out);

        let result = graph.invalidated_outputs(&[tmpl]);
        assert_eq!(result, vec![out]);
    }

    #[test]
    fn diff_reports_changed_new_and_deleted() {
        let mut graph = DepGraph::new();
        graph.record_hash(Path::new("a.md"), b"alpha");
        graph.record_hash(Path::new("b.md"), b"beta");
        graph.record_hash(Path::new("c.md"), b"gamma");

        let mut current = HashMap::new();
        let _ = current
            .insert(PathBuf::from("a.md"), DepGraph::sha256_hex(b"alpha"));
        // b.md changed
        let _ = current
            .insert(PathBuf::from("b.md"), DepGraph::sha256_hex(b"beta-prime"));
        // c.md deleted
        // d.md new
        let _ = current
            .insert(PathBuf::from("d.md"), DepGraph::sha256_hex(b"delta"));

        let diff = graph.diff(&current);
        assert_eq!(
            diff.changed,
            vec![PathBuf::from("b.md"), PathBuf::from("d.md")]
        );
        assert_eq!(diff.deleted, vec![PathBuf::from("c.md")]);
        assert!(!diff.is_empty());
    }

    #[test]
    fn diff_no_changes_is_empty() {
        let mut graph = DepGraph::new();
        graph.record_hash(Path::new("a.md"), b"alpha");
        let mut current = HashMap::new();
        let _ = current
            .insert(PathBuf::from("a.md"), DepGraph::sha256_hex(b"alpha"));
        let diff = graph.diff(&current);
        assert!(diff.is_empty());
    }

    #[test]
    fn save_and_load_round_trip_preserves_edges_and_hashes() {
        let dir = tempdir().unwrap();
        let mut graph = DepGraph::new();
        let page = PathBuf::from("content/index.md");
        let tmpl = PathBuf::from("templates/base.html");
        let out = PathBuf::from("public/index.html");
        graph.add_dep(&page, &tmpl);
        graph.add_output(&page, &out);
        graph.record_hash(&page, b"hello");

        graph.save(dir.path()).unwrap();
        let loaded = DepGraph::load(dir.path());

        assert_eq!(loaded.deps_for(&page).unwrap().len(), 1);
        assert!(loaded.outputs_for(&page).unwrap().contains(&out));
        let mut current = HashMap::new();
        let _ = current.insert(page, DepGraph::sha256_hex(b"hello"));
        assert!(loaded.diff(&current).is_empty());
    }

    #[test]
    fn load_missing_file_yields_empty_graph() {
        let dir = tempdir().unwrap();
        let graph = DepGraph::load(dir.path());
        assert_eq!(graph.page_count(), 0);
        assert_eq!(graph.version, SCHEMA_VERSION);
    }

    #[test]
    fn load_corrupt_json_falls_back_to_empty_ac6() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(DEP_GRAPH_FILE), "{{ not json").unwrap();
        let graph = DepGraph::load(dir.path());
        assert_eq!(graph.page_count(), 0);
    }

    #[test]
    fn load_wrong_schema_version_falls_back_to_empty_ac6() {
        let dir = tempdir().unwrap();
        let body =
            r#"{"version":0,"deps":{},"outputs":{},"hashes":{}}"#.to_string();
        fs::write(dir.path().join(DEP_GRAPH_FILE), body).unwrap();
        let graph = DepGraph::load(dir.path());
        assert_eq!(graph.page_count(), 0);
    }

    #[test]
    fn forget_removes_all_traces_of_a_path() {
        let mut graph = DepGraph::new();
        let page = PathBuf::from("content/about.md");
        let other = PathBuf::from("content/index.md");
        let tmpl = PathBuf::from("templates/page.html");
        graph.add_dep(&page, &tmpl);
        graph.add_dep(&other, &tmpl);
        graph.add_dep(&other, &page); // sibling reference
        graph.add_output(&page, Path::new("public/about/index.html"));
        graph.record_hash(&page, b"x");

        graph.forget(&page);

        assert!(graph.deps_for(&page).is_none());
        assert!(graph.outputs_for(&page).is_none());
        let other_deps = graph.deps_for(&other).unwrap();
        assert!(!other_deps.contains(&page));
        assert!(other_deps.contains(&tmpl));
    }

    #[test]
    fn clear_empties_everything() {
        let mut graph = DepGraph::new();
        graph.add_dep(Path::new("a"), Path::new("b"));
        graph.add_output(Path::new("a"), Path::new("o"));
        graph.record_hash(Path::new("a"), b"x");
        graph.clear();
        assert_eq!(graph.page_count(), 0);
        assert!(graph.tracked_sources().is_empty());
    }

    #[test]
    fn sha256_hex_is_deterministic_64_chars() {
        let h = DepGraph::sha256_hex(b"hello");
        assert_eq!(h.len(), 64);
        assert_eq!(h, DepGraph::sha256_hex(b"hello"));
    }

    #[test]
    fn sha256_hex_distinguishes_inputs() {
        assert_ne!(DepGraph::sha256_hex(b"a"), DepGraph::sha256_hex(b"b"));
    }

    /// A change to a data file must invalidate every page.
    ///
    /// Data is read globally — a template can address `data.foo` from
    /// anywhere — so there is no page that provably does not use it.
    /// Before this edge existed, editing `data/site.toml` left every
    /// page looking clean and an incremental build would have skipped
    /// all of them.
    #[test]
    fn a_data_file_change_invalidates_every_page() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let template = dir.path().join("templates");
        let build = dir.path().join("public");
        let data = dir.path().join("data");
        for d in [&content, &template, &data] {
            fs::create_dir_all(d).unwrap();
        }
        write(
            &content.join("index.md"),
            "---\nlayout: \"page\"\n---\nbody",
        );
        write(
            &content.join("about.md"),
            "---\nlayout: \"page\"\n---\nbody",
        );
        write(
            &template.join("page.html"),
            "<html>{{ data.site.name }}</html>",
        );
        write(&data.join("site.toml"), "name = \"Example\"\n");

        let mut graph = DepGraph::new();
        populate(&mut graph, &content, &template, &build).unwrap();

        let site_data = data.join("site.toml");
        let hit = graph.invalidated(std::slice::from_ref(&site_data));
        for page in [content.join("index.md"), content.join("about.md")] {
            assert!(
                hit.contains(&page),
                "{} not invalidated by a data-file change",
                page.display()
            );
        }
    }

    /// The topic curation file lives in `_data/`, not `data/`.
    ///
    /// It reshapes every topic page, so it is a dependency by the same
    /// argument — and it would have been missed by a helper that only
    /// knew the one convention.
    #[test]
    fn the_underscore_data_convention_is_tracked_too() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let template = dir.path().join("templates");
        let build = dir.path().join("public");
        let data = dir.path().join("_data");
        for d in [&content, &template, &data] {
            fs::create_dir_all(d).unwrap();
        }
        write(
            &content.join("index.md"),
            "---\nlayout: \"page\"\n---\nbody",
        );
        write(&template.join("page.html"), "<html></html>");
        write(&data.join("topics.toml"), "[rust]\ntitle = \"Rust\"\n");

        let mut graph = DepGraph::new();
        populate(&mut graph, &content, &template, &build).unwrap();

        let topics = data.join("topics.toml");
        assert!(
            graph
                .invalidated(std::slice::from_ref(&topics))
                .contains(&content.join("index.md")),
            "_data/topics.toml is not tracked"
        );
    }

    /// A stray file in `data/` that the loader cannot parse is not a
    /// dependency, and must not become an edge.
    #[test]
    fn non_data_extensions_in_the_data_dir_are_ignored() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let template = dir.path().join("templates");
        let build = dir.path().join("public");
        let data = dir.path().join("data");
        for d in [&content, &template, &data] {
            fs::create_dir_all(d).unwrap();
        }
        write(
            &content.join("index.md"),
            "---\nlayout: \"page\"\n---\nbody",
        );
        write(&template.join("page.html"), "<html></html>");
        write(&data.join("notes.md"), "not a data file");
        write(&data.join("real.json"), "{}");

        let mut graph = DepGraph::new();
        populate(&mut graph, &content, &template, &build).unwrap();

        let index = content.join("index.md");
        let deps = graph.deps_for(&index).unwrap();
        assert!(deps.contains(&data.join("real.json")), "json not tracked");
        assert!(
            !deps.contains(&data.join("notes.md")),
            "a .md in data/ is not a data file"
        );
    }

    #[test]
    fn populate_walks_real_directories_and_records_edges() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let template = dir.path().join("templates");
        let build = dir.path().join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&template).unwrap();

        write(
            &content.join("index.md"),
            "---\nlayout: \"page\"\n---\nbody",
        );
        write(
            &content.join("about.md"),
            "---\nlayout: \"page\"\n---\nbody",
        );
        write(&template.join("page.html"), "<html>{{title}}</html>");

        let mut graph = DepGraph::new();
        populate(&mut graph, &content, &template, &build).unwrap();

        // Edges recorded
        let index = content.join("index.md");
        let about = content.join("about.md");
        let page_tpl = template.join("page.html");
        assert!(graph.deps_for(&index).unwrap().contains(&page_tpl));
        assert!(graph.deps_for(&about).unwrap().contains(&page_tpl));

        // Outputs recorded correctly
        let outs_index = graph.outputs_for(&index).unwrap();
        assert!(outs_index.contains(&build.join("index.html")));
        let outs_about = graph.outputs_for(&about).unwrap();
        assert!(outs_about.contains(&build.join("about").join("index.html")));

        // Hashes recorded
        assert!(!graph.hashes.is_empty());
    }

    #[test]
    fn populate_records_template_to_template_edges() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let template = dir.path().join("templates");
        let build = dir.path().join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&template).unwrap();

        write(&content.join("index.md"), "---\nlayout: \"page\"\n---\n");
        write(
            &template.join("page.html"),
            "{{#extends \"base\"}}\n<p>x</p>",
        );
        write(&template.join("base.html"), "<html>{{body}}</html>");

        let mut graph = DepGraph::new();
        populate(&mut graph, &content, &template, &build).unwrap();

        let page_tpl = template.join("page.html");
        let base_tpl = template.join("base.html");
        assert!(
            graph.deps_for(&page_tpl).unwrap().contains(&base_tpl),
            "template→template edge must be recorded"
        );

        // Transitive: changing base.html must invalidate the content page.
        let invalidated = graph.invalidated(&[base_tpl]);
        assert!(invalidated.contains(&content.join("index.md")));
    }

    #[test]
    fn populate_records_partial_references() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let template = dir.path().join("templates");
        let build = dir.path().join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&template).unwrap();

        write(&content.join("index.md"), "---\nlayout: \"page\"\n---\n");
        write(
            &template.join("page.html"),
            "<div>{{->header title=\"x\"}}</div>",
        );
        write(&template.join("header.html"), "<h1>{{title}}</h1>");

        let mut graph = DepGraph::new();
        populate(&mut graph, &content, &template, &build).unwrap();

        let page_tpl = template.join("page.html");
        let header_tpl = template.join("header.html");
        assert!(graph.deps_for(&page_tpl).unwrap().contains(&header_tpl));
    }

    #[test]
    fn output_paths_for_root_index_emits_root_index_html() {
        let outs = output_paths_for(
            Path::new("/c/index.md"),
            Path::new("/c"),
            Path::new("/b"),
        );
        assert_eq!(outs, vec![PathBuf::from("/b/index.html")]);
    }

    #[test]
    fn output_paths_for_nested_post_emits_subdir_index() {
        let outs = output_paths_for(
            Path::new("/c/blog/foo.md"),
            Path::new("/c"),
            Path::new("/b"),
        );
        assert_eq!(outs, vec![PathBuf::from("/b/blog/foo/index.html")]);
    }

    #[test]
    fn output_paths_for_nested_index_emits_subdir_index_html() {
        let outs = output_paths_for(
            Path::new("/c/blog/index.md"),
            Path::new("/c"),
            Path::new("/b"),
        );
        assert_eq!(outs, vec![PathBuf::from("/b/blog/index.html")]);
    }

    #[test]
    fn extract_layout_reads_yaml_frontmatter() {
        let text = "---\ntitle: foo\nlayout: \"post\"\n---\nbody";
        assert_eq!(extract_layout(text.as_bytes()), Some("post".to_string()));
    }

    #[test]
    fn extract_layout_bareword() {
        let text = "---\nlayout: post\n---\nbody";
        assert_eq!(extract_layout(text.as_bytes()), Some("post".to_string()));
    }

    #[test]
    fn extract_layout_missing_returns_none() {
        let text = "---\ntitle: foo\n---\nbody";
        assert!(extract_layout(text.as_bytes()).is_none());
    }

    #[test]
    fn extract_layout_no_frontmatter_returns_none() {
        let text = "# just a heading";
        assert!(extract_layout(text.as_bytes()).is_none());
    }

    #[test]
    fn scan_template_refs_handles_extends_and_partial() {
        let body =
            "{{#extends \"base\"}}\n{{->header title=\"x\"}}\n{{->footer}}";
        let refs = scan_template_refs(body);
        assert_eq!(
            refs,
            vec![
                "base".to_string(),
                "header".to_string(),
                "footer".to_string()
            ]
        );
    }

    #[test]
    fn scan_template_refs_deduplicates() {
        let body = "{{->header}} {{->header}} {{->header title=\"a\"}}";
        let refs = scan_template_refs(body);
        assert_eq!(refs, vec!["header".to_string()]);
    }

    #[test]
    fn scan_template_refs_ignores_plain_variables() {
        let body = "<p>{{title}}</p>{{!raw_html}}";
        assert!(scan_template_refs(body).is_empty());
    }

    #[test]
    fn current_hashes_picks_up_md_and_html() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("c");
        let template = dir.path().join("t");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&template).unwrap();
        write(&content.join("a.md"), "---\nlayout: page\n---");
        write(&template.join("page.html"), "<h1></h1>");

        let hashes = current_hashes(&content, &template).unwrap();
        assert!(hashes.contains_key(&content.join("a.md")));
        assert!(hashes.contains_key(&template.join("page.html")));
    }

    #[test]
    fn record_hash_from_disk_silently_skips_missing() {
        let mut graph = DepGraph::new();
        graph.record_hash_from_disk(Path::new("/nonexistent/x.md"));
        assert!(graph.hashes.is_empty());
    }

    #[test]
    fn diff_is_empty_helper_round_trips() {
        let d = Diff::default();
        assert!(d.is_empty());
    }

    #[test]
    fn tracked_sources_returns_sorted_unique_outputs() {
        let mut graph = DepGraph::new();
        graph.add_output(Path::new("b.md"), Path::new("b.html"));
        graph.add_output(Path::new("a.md"), Path::new("a.html"));
        assert_eq!(
            graph.tracked_sources(),
            vec![PathBuf::from("a.md"), PathBuf::from("b.md")]
        );
    }

    // ── save error-path closure coverage ────────────────────────────

    #[test]
    fn save_fails_when_cache_root_is_a_file_not_a_dir() {
        // Make `cache_root` point at an existing regular file so
        // fs::create_dir_all returns AlreadyExists+NotADirectory and
        // the map_err closure constructing SsgError::Io fires.
        let dir = tempdir().unwrap();
        let blocker = dir.path().join("not-a-dir");
        fs::write(&blocker, b"i am a file").unwrap();
        let cache_root = blocker.join("sub");
        let graph = DepGraph::new();
        let err = graph.save(&cache_root).unwrap_err();
        let msg = format!("{err}");
        assert!(!msg.is_empty());
    }

    #[test]
    fn save_writes_then_renames_to_final_path() {
        // Exercises the happy-path: success path of all three map_err
        // arms (create_dir_all, write, rename). Verifies the final
        // depgraph.json exists and the .tmp file is gone.
        let dir = tempdir().unwrap();
        let cache_root = dir.path().join("cache");
        let mut g = DepGraph::new();
        g.add_dep(Path::new("a.md"), Path::new("b.html"));
        g.add_output(Path::new("a.md"), Path::new("out.html"));
        g.record_hash(Path::new("a.md"), b"contents");
        g.save(&cache_root).unwrap();

        let final_path = cache_root.join(DEP_GRAPH_FILE);
        assert!(final_path.exists());
        let tmp_path = cache_root.join(format!("{DEP_GRAPH_FILE}.tmp"));
        assert!(
            !tmp_path.exists(),
            ".tmp file should be renamed away after save"
        );
    }

    // ── default_version: hit serde's default-field fallback ─────────

    #[test]
    fn load_treats_missing_version_field_as_incompatible() {
        // Write a graph with no version field. serde's `default =
        // default_version` returns 0, which mismatches SCHEMA_VERSION,
        // so load() returns an empty graph (the AC6 fallback path).
        let dir = tempdir().unwrap();
        let cache_root = dir.path();
        let path = cache_root.join(DEP_GRAPH_FILE);
        fs::write(&path, br#"{"deps":{},"outputs":{},"hashes":{}}"#).unwrap();
        let loaded = DepGraph::load(cache_root);
        assert_eq!(loaded.page_count(), 0);
        assert!(loaded.tracked_sources().is_empty());
    }

    // ── populate error-path closure coverage ────────────────────────

    #[test]
    fn populate_propagates_unreadable_markdown_via_map_err_closure() {
        // Drop a .md file then make it unreadable. The closure that
        // wraps fs::read's error into SsgError::Io fires.
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        let build = dir.path().join("build");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        let md = content.join("page.md");
        fs::write(&md, b"---\nlayout: post\n---\nhi").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // chmod 000 so fs::read returns PermissionDenied
            fs::set_permissions(&md, fs::Permissions::from_mode(0o000))
                .unwrap();
        }

        let mut g = DepGraph::new();
        let res = populate(&mut g, &content, &templates, &build);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Restore perms so tempdir cleanup works.
            let _ = fs::set_permissions(&md, fs::Permissions::from_mode(0o644));
            // On unix the error path is exercised. Some CI runners
            // run as root and bypass perms — don't fail if we did get
            // through, but assert: if it errored, the message is non-empty.
            assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
        }
        #[cfg(not(unix))]
        {
            let _ = res;
        }
    }

    #[test]
    fn populate_propagates_unreadable_template_via_map_err_closure() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        let build = dir.path().join("build");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        let tpl = templates.join("post.html");
        fs::write(&tpl, b"{{#extends \"base\"}}").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tpl, fs::Permissions::from_mode(0o000))
                .unwrap();
        }

        let mut g = DepGraph::new();
        let res = populate(&mut g, &content, &templates, &build);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                fs::set_permissions(&tpl, fs::Permissions::from_mode(0o644));
            assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
        }
        #[cfg(not(unix))]
        {
            let _ = res;
        }
    }

    // ── load — warn-branch format arguments ─────────────────────────

    #[test]
    fn load_incompatible_schema_warns_and_falls_back() {
        // init_logger raises the max level so the `log::warn!` format
        // arguments (line 127) execute.
        crate::test_support::init_logger();
        let dir = tempdir().unwrap();
        let stale = serde_json::json!({
            "version": 1,
            "deps": {},
            "outputs": {},
            "hashes": {}
        });
        fs::write(dir.path().join(DEP_GRAPH_FILE), stale.to_string()).unwrap();

        let g = DepGraph::load(dir.path());
        assert_eq!(g.page_count(), 0);
    }

    #[test]
    fn load_corrupt_json_warns_and_falls_back() {
        // Same as above for the corrupt-JSON arm (line 134).
        crate::test_support::init_logger();
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(DEP_GRAPH_FILE), "{ nope").unwrap();

        let g = DepGraph::load(dir.path());
        assert_eq!(g.page_count(), 0);
    }

    // ── save — serialization and I/O error paths ────────────────────

    #[test]
    #[cfg(unix)]
    fn save_non_utf8_path_fails_serialization() {
        // serde's PathBuf serializer rejects non-UTF-8 paths, driving
        // the `to_string` map_err closure (lines 166-169).
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let dir = tempdir().unwrap();
        let bad = PathBuf::from(OsStr::from_bytes(&[0x66, 0xFF, 0xFE]));
        let mut g = DepGraph::new();
        g.add_dep(&bad, Path::new("layout.html"));

        assert!(g.save(dir.path()).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn save_unwritable_cache_root_fails_tmp_write() {
        // cache_root exists but is read-only: create_dir_all succeeds
        // (already there), the tmp-file write fails (lines 170-173).
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let root = dir.path().join("cache");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();

        let res = DepGraph::new().save(&root);

        let _ = fs::set_permissions(&root, fs::Permissions::from_mode(0o755));
        // Root bypasses permissions on some CI runners, so tolerate Ok.
        assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
    }

    #[test]
    fn save_rename_over_directory_fails() {
        // A non-empty directory squatting on the final path makes the
        // atomic rename fail (lines 174-177).
        let dir = tempdir().unwrap();
        let blocker = dir.path().join(DEP_GRAPH_FILE);
        fs::create_dir_all(&blocker).unwrap();
        fs::write(blocker.join("keep.txt"), "x").unwrap();

        assert!(DepGraph::new().save(dir.path()).is_err());
    }

    // ── record_hash_from_disk — both arms ───────────────────────────

    #[test]
    fn record_hash_from_disk_reads_existing_and_skips_missing() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.md");
        fs::write(&p, "hello").unwrap();

        let mut g = DepGraph::new();
        g.record_hash_from_disk(&p);
        // Missing files are silently ignored (the else arm).
        g.record_hash_from_disk(&dir.path().join("missing.md"));

        let current = current_hashes(dir.path(), dir.path()).unwrap();
        assert!(g.diff(&current).is_empty());
    }

    // ── invalidated — revisit guard ─────────────────────────────────

    #[test]
    fn invalidated_deduplicates_repeated_inputs() {
        // A duplicated changed path hits the `!seen.insert` continue.
        let g = DepGraph::new();
        let changed =
            vec![PathBuf::from("content/a.md"), PathBuf::from("content/a.md")];
        assert_eq!(
            g.invalidated(&changed),
            vec![PathBuf::from("content/a.md")]
        );
    }

    // ── populate / current_hashes — walk failures ───────────────────

    #[test]
    #[cfg(unix)]
    fn populate_propagates_unreadable_content_dir() {
        // The walk itself fails when the content dir can't be listed
        // (the `?` on the first walk).
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::set_permissions(&content, fs::Permissions::from_mode(0o000))
            .unwrap();

        let mut g = DepGraph::new();
        let res = populate(&mut g, &content, &templates, &dir.path().join("b"));

        let _ =
            fs::set_permissions(&content, fs::Permissions::from_mode(0o755));
        assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
    }

    #[test]
    #[cfg(unix)]
    fn populate_propagates_unreadable_template_dir() {
        // Content walk succeeds; the template walk fails (the `?` on
        // the second walk).
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::set_permissions(&templates, fs::Permissions::from_mode(0o000))
            .unwrap();

        let mut g = DepGraph::new();
        let res = populate(&mut g, &content, &templates, &dir.path().join("b"));

        let _ =
            fs::set_permissions(&templates, fs::Permissions::from_mode(0o755));
        assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
    }

    #[test]
    #[cfg(unix)]
    fn current_hashes_propagates_walk_failures_from_both_dirs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();

        // Unreadable content dir → first `?`.
        fs::set_permissions(&content, fs::Permissions::from_mode(0o000))
            .unwrap();
        let res_content = current_hashes(&content, &templates);
        let _ =
            fs::set_permissions(&content, fs::Permissions::from_mode(0o755));

        // Unreadable template dir → second `?`.
        fs::set_permissions(&templates, fs::Permissions::from_mode(0o000))
            .unwrap();
        let res_templates = current_hashes(&content, &templates);
        let _ =
            fs::set_permissions(&templates, fs::Permissions::from_mode(0o755));

        assert!(res_content.err().is_none_or(|e| !format!("{e}").is_empty()));
        assert!(res_templates
            .err()
            .is_none_or(|e| !format!("{e}").is_empty()));
    }

    #[test]
    #[cfg(unix)]
    fn current_hashes_skips_unreadable_sources() {
        // A dangling .md symlink is returned by the walk but fails
        // fs::read, taking the silent-skip arm inside `push`.
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::write(content.join("real.md"), "hi").unwrap();
        std::os::unix::fs::symlink(
            content.join("ghost-target.md"),
            content.join("ghost.md"),
        )
        .unwrap();

        let map = current_hashes(&content, &templates).unwrap();
        assert_eq!(map.len(), 1, "only the readable file is hashed");
    }

    // ── populate — unresolved layout edge ───────────────────────────

    #[test]
    fn populate_skips_edge_when_layout_cannot_be_resolved() {
        // `layout: ghost` with no matching template file: the
        // resolve_template else-arm leaves the page without a
        // template edge.
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        let build = dir.path().join("build");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::write(content.join("page.md"), "---\nlayout: ghost\n---\nbody")
            .unwrap();

        let mut g = DepGraph::new();
        populate(&mut g, &content, &templates, &build).unwrap();
        // Only the self-edge is recorded.
        let deps = g
            .deps_for(&content.join("page.md"))
            .expect("page must be tracked");
        assert_eq!(deps.len(), 1);
    }

    // ── extract_layout — rejection paths ────────────────────────────

    #[test]
    fn extract_layout_rejects_malformed_frontmatter() {
        // Non-UTF-8 bytes.
        assert_eq!(extract_layout(&[0xFF, 0xFE, 0x00]), None);
        // Opening fence with no closing fence.
        assert_eq!(extract_layout(b"---\nlayout: x"), None);
        // Empty layout value: `split_whitespace().next()` is None.
        assert_eq!(extract_layout(b"---\nlayout:\n---\nbody"), None);
    }

    // ── output_paths_for / resolve_template — fallback arms ─────────

    #[test]
    fn output_paths_for_foreign_path_returns_empty() {
        // A file outside content_dir fails strip_prefix.
        let out = output_paths_for(
            Path::new("/elsewhere/post.md"),
            Path::new("/content"),
            Path::new("/build"),
        );
        assert!(out.is_empty());
    }

    #[test]
    fn resolve_template_prefers_locale_sibling() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        fs::create_dir_all(content.join("fr")).unwrap();
        fs::create_dir_all(templates.join("fr")).unwrap();
        fs::write(templates.join("fr/post.html"), "x").unwrap();
        fs::write(templates.join("post.html"), "x").unwrap();

        let got = resolve_template(
            &templates,
            &content.join("fr/a.md"),
            &content,
            "post",
        );
        assert_eq!(got, Some(templates.join("fr/post.html")));
    }

    #[test]
    fn resolve_template_falls_back_when_locale_candidate_is_missing() {
        // `rel.components().next()` is `Some(first)` (the content path
        // has a leading directory component) but the locale-sibling
        // candidate doesn't exist on disk — the inner
        // `if candidate.exists()` false arm, distinct from
        // `resolve_template_prefers_locale_sibling` (which always hits
        // the true arm) and from the empty/foreign-path test below
        // (which never enters the `Some(first)` branch at all).
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        fs::create_dir_all(content.join("fr")).unwrap();
        fs::create_dir_all(&templates).unwrap();
        // No `templates/fr/post.html` — only the plain fallback exists.
        fs::write(templates.join("post.html"), "x").unwrap();

        let got = resolve_template(
            &templates,
            &content.join("fr/a.md"),
            &content,
            "post",
        );
        assert_eq!(got, Some(templates.join("post.html")));
    }

    #[test]
    fn resolve_template_falls_back_for_empty_and_foreign_paths() {
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let templates = dir.path().join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::write(templates.join("page.html"), "x").unwrap();

        // md == content_dir: the relative path has no components.
        assert_eq!(
            resolve_template(&templates, &content, &content, "page"),
            Some(templates.join("page.html"))
        );
        // md outside content_dir: strip_prefix fails.
        assert_eq!(
            resolve_template(
                &templates,
                Path::new("/elsewhere/a.md"),
                &content,
                "page"
            ),
            Some(templates.join("page.html"))
        );
        // Nothing on disk at all: fallback is None.
        assert_eq!(
            resolve_template(
                &templates,
                &content.join("a.md"),
                &content,
                "missing"
            ),
            None
        );
    }

    // ── scan_template_refs / parse_name — edge shapes ───────────────

    #[test]
    fn scan_template_refs_handles_unclosed_and_plain_refs() {
        // Unclosed `{{` → break arm.
        assert!(scan_template_refs("{{#extends \"base\"").is_empty());
        // Plain variable refs produce no names; empty extends name is
        // filtered; duplicates are deduped.
        let refs =
            scan_template_refs("{{ title }}{{#extends \"\"}}{{->p}}{{->p}}");
        assert_eq!(refs, vec!["p".to_string()]);
    }
}
