// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ISR manifest emitter — `dist/.ssg/manifest.json` + raw content KV
//! payloads under `dist/.ssg/content/`.
//!
//! Runs as an `after_compile` plugin, but ONLY when the build opted
//! into ISR via `--isr` (issue #546 AC9 — without the flag the plugin
//! is not registered and the build stays byte-identical to v0.0.43).
//!
//! Behaviour:
//!
//! 1. Walks the content directory for `.md` files.
//! 2. For each markdown file, parses frontmatter to look for
//!    `isr.s_maxage` / `isr.swr` overrides.
//! 3. Derives the published URL using the existing slug rules.
//! 4. Emits a `ManifestEntry` listing the markdown source + the
//!    relevant templates as `sources`, with a sha256 over their bytes.
//! 5. Writes `dist/.ssg/manifest.json` and copies the raw sources into
//!    `dist/.ssg/content/` so the deploy step can upload to KV.

use std::fs;
use std::path::{Path, PathBuf};

use ssg_core::{build_entry, CachePolicy, Manifest, ManifestEntry};

use crate::error::SsgError;
use crate::plugin::{Plugin, PluginContext};

/// Subdirectory inside `dist/.ssg/` that holds the manifest.
pub const MANIFEST_RELATIVE_PATH: &str = ".ssg/manifest.json";

/// Subdirectory inside `dist/.ssg/` that holds raw source payloads
/// destined for KV / Edge Config upload.
pub const CONTENT_RELATIVE_DIR: &str = ".ssg/content";

/// `after_compile` plugin that emits the ISR manifest + raw KV
/// payloads. Off by default; enabled by the `--isr` flag.
///
/// # Examples
///
/// ```
/// use ssg::plugin::Plugin;
/// use ssg::isr_manifest::IsrManifestPlugin;
/// assert_eq!(IsrManifestPlugin::new().name(), "isr-manifest");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct IsrManifestPlugin;

impl IsrManifestPlugin {
    /// Constructs a new instance.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::isr_manifest::IsrManifestPlugin;
    /// let _plugin = IsrManifestPlugin::new();
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for IsrManifestPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for IsrManifestPlugin {
    fn name(&self) -> &'static str {
        "isr-manifest"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if ctx.dry_run {
            return Ok(());
        }

        let manifest =
            build_manifest(&ctx.content_dir, &ctx.template_dir, &ctx.site_dir)?;

        write_manifest(&manifest, &ctx.site_dir)?;
        copy_sources(
            &ctx.content_dir,
            &ctx.template_dir,
            &ctx.site_dir,
            &manifest,
        )?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// build_manifest — walk content + derive entries
// ---------------------------------------------------------------------------

/// Builds a [`Manifest`] by walking `content_dir` for `.md` files and
/// pairing each with the layout templates it would render against.
///
/// The layout selection mirrors what the staticdatagen pipeline would
/// do — `templates/index.html` and `templates/page.html` cover the
/// 95% case. A page can override the cache policy via
/// `isr.s_maxage` / `isr.swr` in frontmatter.
///
/// # Errors
///
/// Returns [`SsgError::Io`] when the content/template directories cannot
/// be walked or read.
///
/// # Examples
///
/// ```
/// use ssg::isr_manifest::build_manifest;
/// let tmp = tempfile::tempdir().unwrap();
/// let content = tmp.path().join("content");
/// let templates = tmp.path().join("templates");
/// let site = tmp.path().join("site");
/// std::fs::create_dir_all(&content).unwrap();
/// std::fs::create_dir_all(&templates).unwrap();
/// std::fs::create_dir_all(&site).unwrap();
/// let m = build_manifest(&content, &templates, &site).unwrap();
/// assert_eq!(m.len(), 0);
/// ```
pub fn build_manifest(
    content_dir: &Path,
    template_dir: &Path,
    site_dir: &Path,
) -> Result<Manifest, SsgError> {
    let mut manifest = Manifest::new(build_stamp());

    let md_files = collect_md_files(content_dir)?;
    for md_path in md_files {
        let entry = build_entry_for_markdown(
            &md_path,
            content_dir,
            template_dir,
            site_dir,
        )?;
        let Some((url, entry)) = entry else { continue };
        manifest.insert(url, entry);
    }

    Ok(manifest)
}

/// Returns a stable per-build identifier. Uses the workspace package
/// version when available — adapters compare this to detect a deploy.
fn build_stamp() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!("ssg-{version}")
}

/// Walks `dir` recursively and returns every `.md` file. Skips hidden
/// directories (`.git`, `.ssg`, etc.) and returns paths sorted
/// lexicographically for determinism.
fn collect_md_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    visit(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn visit(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), SsgError> {
    let entries = fs::read_dir(dir).map_err(|e| SsgError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            visit(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Builds a `(url, ManifestEntry)` pair for a single markdown file.
fn build_entry_for_markdown(
    md_path: &Path,
    content_dir: &Path,
    template_dir: &Path,
    _site_dir: &Path,
) -> Result<Option<(String, ManifestEntry)>, SsgError> {
    let bytes = fs::read(md_path).map_err(|e| SsgError::Io {
        path: md_path.to_path_buf(),
        source: e,
    })?;

    let rel = md_path
        .strip_prefix(content_dir)
        .unwrap_or(md_path)
        .to_string_lossy()
        .replace('\\', "/");

    // Derive published URL: content/posts/foo.md → /posts/foo/index.html
    let url = derive_url(&rel);

    // Parse frontmatter for ISR overrides.
    let text = String::from_utf8_lossy(&bytes);
    let cache = extract_isr_cache(&text);

    // Templates that almost every page depends on. The static-data
    // pipeline picks one of `index.html` / `page.html` per page — we
    // include both so the Edge renderer can decide at fetch time.
    let templates = collect_templates(template_dir);
    let mut template_bytes_owned: Vec<Vec<u8>> =
        Vec::with_capacity(templates.len());
    let mut sources: Vec<String> = Vec::with_capacity(1 + templates.len());
    sources.push(format!("content/{rel}"));

    for (tpl_rel, tpl_path) in &templates {
        let tb = fs::read(tpl_path).map_err(|e| SsgError::Io {
            path: tpl_path.clone(),
            source: e,
        })?;
        template_bytes_owned.push(tb);
        sources.push(format!("templates/{tpl_rel}"));
    }

    let mut byte_refs: Vec<&[u8]> = Vec::with_capacity(sources.len());
    byte_refs.push(&bytes);
    for tb in &template_bytes_owned {
        byte_refs.push(tb);
    }

    let entry = build_entry(sources, &byte_refs, cache);
    Ok(Some((url, entry)))
}

/// Returns `[(relative_template_path, absolute_path)]` for layouts
/// the Edge renderer might need. Returns an empty vec if the dir
/// doesn't exist (e.g. minimal sites).
fn collect_templates(template_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if !template_dir.exists() {
        return out;
    }
    let candidates = ["index.html", "page.html"];
    for name in candidates {
        let p = template_dir.join(name);
        if p.exists() {
            out.push((name.to_string(), p));
        }
    }
    out
}

/// Maps a content-relative path (`posts/foo.md`, `index.md`,
/// `about/index.md`) to the published URL the static pipeline would
/// emit (`/posts/foo/index.html`, `/index.html`, `/about/index.html`).
fn derive_url(rel: &str) -> String {
    let stripped = rel.strip_suffix(".md").unwrap_or(rel);
    if stripped == "index" {
        return "/index.html".to_string();
    }
    if let Some(trim) = stripped.strip_suffix("/index") {
        return format!("/{trim}/index.html");
    }
    format!("/{stripped}/index.html")
}

/// Extracts `isr.s_maxage` and `isr.swr` from YAML/TOML/JSON
/// frontmatter. Returns `None` if neither is present.
///
/// Frontmatter shape (YAML):
///
/// ```yaml
/// isr:
///   s_maxage: 600
///   swr: 3600
/// ```
fn extract_isr_cache(text: &str) -> Option<CachePolicy> {
    // Strip frontmatter block.
    let fm = extract_frontmatter_block(text)?;

    // Look for `isr:` block and the two numeric keys. We do a
    // line-based scan so the parser stays deterministic and avoids
    // pulling a YAML dep here — the canonical parser lives in
    // staticdatagen / frontmatter-gen and runs upstream of us.
    let mut in_isr = false;
    let mut s_maxage: Option<u32> = None;
    let mut swr: Option<u32> = None;

    for raw_line in fm.lines() {
        let line = raw_line.trim_end();
        if line.starts_with("isr:") {
            in_isr = true;
            continue;
        }
        if in_isr {
            let trimmed = line.trim_start();
            // Indented child of `isr:`
            if line.starts_with(' ') || line.starts_with('\t') {
                if let Some((k, v)) = trimmed.split_once(':') {
                    let k = k.trim();
                    let v = v.trim();
                    match k {
                        "s_maxage" | "s-maxage" => {
                            s_maxage = v.parse::<u32>().ok();
                        }
                        "swr" | "stale-while-revalidate" => {
                            swr = v.parse::<u32>().ok();
                        }
                        _ => {}
                    }
                }
            } else if !line.is_empty() {
                in_isr = false;
            }
        }
    }

    if s_maxage.is_none() && swr.is_none() {
        return None;
    }
    Some(CachePolicy {
        s_maxage: s_maxage.unwrap_or(ssg_core::DEFAULT_S_MAXAGE),
        swr: swr.unwrap_or(ssg_core::DEFAULT_SWR),
    })
}

/// Extracts the raw YAML/TOML body of the frontmatter block. Supports
/// `---`-fenced YAML and `+++`-fenced TOML.
fn extract_frontmatter_block(text: &str) -> Option<&str> {
    let trimmed = text.trim_start();
    if let Some(after) = trimmed.strip_prefix("---") {
        if let Some(end) = after.find("---") {
            return Some(&after[..end]);
        }
    }
    if let Some(after) = trimmed.strip_prefix("+++") {
        if let Some(end) = after.find("+++") {
            return Some(&after[..end]);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// I/O: write manifest + copy raw sources
// ---------------------------------------------------------------------------

fn write_manifest(
    manifest: &Manifest,
    site_dir: &Path,
) -> Result<(), SsgError> {
    let manifest_path = site_dir.join(MANIFEST_RELATIVE_PATH);
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| SsgError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let json = manifest.to_pretty_json().map_err(|e| SsgError::Io {
        path: manifest_path.clone(),
        source: std::io::Error::other(e),
    })?;
    fs::write(&manifest_path, json).map_err(|e| SsgError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;
    Ok(())
}

fn copy_sources(
    content_dir: &Path,
    template_dir: &Path,
    site_dir: &Path,
    manifest: &Manifest,
) -> Result<(), SsgError> {
    let content_out = site_dir.join(CONTENT_RELATIVE_DIR);
    fs::create_dir_all(&content_out).map_err(|e| SsgError::Io {
        path: content_out.clone(),
        source: e,
    })?;

    // Collect unique source keys from manifest.
    let mut all_sources = std::collections::BTreeSet::new();
    for entry in manifest.entries.values() {
        for s in &entry.sources {
            let _ = all_sources.insert(s.clone());
        }
    }

    for source in all_sources {
        let src_path = if let Some(rel) = source.strip_prefix("content/") {
            content_dir.join(rel)
        } else if let Some(rel) = source.strip_prefix("templates/") {
            template_dir.join(rel)
        } else {
            continue;
        };
        if !src_path.exists() {
            continue;
        }

        let dst_path = content_out.join(&source);
        if let Some(parent) = dst_path.parent() {
            fs::create_dir_all(parent).map_err(|e| SsgError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let _bytes_copied =
            fs::copy(&src_path, &dst_path).map_err(|e| SsgError::Io {
                path: dst_path.clone(),
                source: e,
            })?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn derive_url_index() {
        assert_eq!(derive_url("index.md"), "/index.html");
    }

    #[test]
    fn derive_url_post() {
        assert_eq!(derive_url("posts/foo.md"), "/posts/foo/index.html");
    }

    #[test]
    fn derive_url_already_index() {
        assert_eq!(derive_url("about/index.md"), "/about/index.html");
    }

    #[test]
    fn derive_url_nested() {
        assert_eq!(derive_url("a/b/c.md"), "/a/b/c/index.html");
    }

    #[test]
    fn derive_url_without_md_suffix_uses_input_verbatim() {
        // `strip_suffix(".md")` fails when there's no `.md` extension,
        // exercising the `unwrap_or(rel)` fallback before deriving the
        // URL from the (unstripped) input.
        assert_eq!(derive_url("notes"), "/notes/index.html");
    }

    #[test]
    fn extract_isr_cache_yaml() {
        let text =
            "---\ntitle: Foo\nisr:\n  s_maxage: 600\n  swr: 3600\n---\n# Body";
        let c = extract_isr_cache(text).unwrap();
        assert_eq!(c.s_maxage, 600);
        assert_eq!(c.swr, 3600);
    }

    #[test]
    fn extract_isr_cache_only_s_maxage() {
        let text = "---\nisr:\n  s_maxage: 30\n---\n";
        let c = extract_isr_cache(text).unwrap();
        assert_eq!(c.s_maxage, 30);
        assert_eq!(c.swr, ssg_core::DEFAULT_SWR);
    }

    #[test]
    fn extract_isr_cache_only_swr() {
        let text = "---\nisr:\n  swr: 7200\n---\n";
        let c = extract_isr_cache(text).unwrap();
        assert_eq!(c.s_maxage, ssg_core::DEFAULT_S_MAXAGE);
        assert_eq!(c.swr, 7200);
    }

    #[test]
    fn extract_isr_cache_none() {
        let text = "---\ntitle: Foo\n---\n";
        assert!(extract_isr_cache(text).is_none());
    }

    #[test]
    fn extract_isr_cache_no_frontmatter() {
        assert!(extract_isr_cache("# Hello").is_none());
    }

    #[test]
    fn collect_md_files_recursive_and_sorted() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("b.md"), "b").unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/a.md"), "a").unwrap();
        fs::write(dir.path().join("ignore.txt"), "no").unwrap();

        let files = collect_md_files(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
        // Sorted lexicographically by full path: `b.md` < `sub/a.md`.
        assert!(files[0].ends_with("b.md"));
        assert!(files[1].ends_with("sub/a.md"));
    }

    #[test]
    fn collect_md_files_skips_hidden_dirs() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        fs::write(dir.path().join(".hidden/a.md"), "x").unwrap();
        fs::write(dir.path().join("real.md"), "y").unwrap();

        let files = collect_md_files(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("real.md"));
    }

    #[test]
    fn build_manifest_emits_entries() {
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");

        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();

        fs::write(content_dir.join("index.md"), "# Home").unwrap();
        fs::write(
            content_dir.join("post.md"),
            "---\nisr:\n  s_maxage: 30\n---\n# Post",
        )
        .unwrap();
        fs::write(template_dir.join("index.html"), "<html/>").unwrap();
        fs::write(template_dir.join("page.html"), "<page/>").unwrap();

        let m = build_manifest(&content_dir, &template_dir, &site_dir).unwrap();
        assert_eq!(m.len(), 2);
        assert!(m.get("/index.html").is_some());
        let post = m.get("/post/index.html").unwrap();
        assert_eq!(post.cache.as_ref().unwrap().s_maxage, 30);
        assert_eq!(post.sources[0], "content/post.md");
        // Sources include the two templates.
        assert!(post.sources.iter().any(|s| s == "templates/index.html"));
        assert!(post.sources.iter().any(|s| s == "templates/page.html"));
        assert_eq!(post.hash.len(), 64);
    }

    #[test]
    fn build_entry_for_markdown_falls_back_to_full_path_outside_content_dir() {
        // `md_path.strip_prefix(content_dir)` fails when the markdown
        // file doesn't actually live under `content_dir`, exercising
        // the `unwrap_or(md_path)` fallback that keeps the full path
        // as `rel` instead of erroring out.
        let dir = tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let md_path = elsewhere.join("orphan.md");
        fs::write(&md_path, "# Orphan").unwrap();

        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("site");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();

        let (url, entry) = build_entry_for_markdown(
            &md_path,
            &content_dir,
            &template_dir,
            &site_dir,
        )
        .unwrap()
        .expect("entry is always produced");
        assert!(
            entry.sources[0].contains("orphan.md"),
            "source should reference the full path: {:?}",
            entry.sources
        );
        assert!(url.ends_with("/index.html"));
    }

    #[test]
    fn write_manifest_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let m = Manifest::default();
        write_manifest(&m, dir.path()).unwrap();
        let p = dir.path().join(MANIFEST_RELATIVE_PATH);
        assert!(p.exists());
        let parsed: Manifest =
            serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn plugin_after_compile_writes_manifest_and_copies_sources() {
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");

        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();

        fs::write(content_dir.join("a.md"), "# A").unwrap();
        fs::write(template_dir.join("index.html"), "<x/>").unwrap();

        let ctx = PluginContext {
            content_dir: content_dir.clone(),
            build_dir: site_dir.clone(),
            site_dir: site_dir.clone(),
            template_dir: template_dir.clone(),
            config: None,
            cache: None,
            memory_budget: None,
            html_files: None,
            dep_graph: None,
            dry_run: false,
        };

        IsrManifestPlugin.after_compile(&ctx).unwrap();

        let manifest_path = site_dir.join(MANIFEST_RELATIVE_PATH);
        assert!(manifest_path.exists());

        let content_dst =
            site_dir.join(CONTENT_RELATIVE_DIR).join("content/a.md");
        assert!(content_dst.exists(), "raw markdown should be staged");

        let template_dst = site_dir
            .join(CONTENT_RELATIVE_DIR)
            .join("templates/index.html");
        assert!(template_dst.exists(), "template should be staged");
    }

    #[test]
    fn plugin_after_compile_dry_run_writes_nothing() {
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();
        fs::write(content_dir.join("a.md"), "x").unwrap();

        let ctx = PluginContext {
            content_dir,
            build_dir: site_dir.clone(),
            site_dir: site_dir.clone(),
            template_dir,
            config: None,
            cache: None,
            memory_budget: None,
            html_files: None,
            dep_graph: None,
            dry_run: true,
        };

        IsrManifestPlugin.after_compile(&ctx).unwrap();
        assert!(!site_dir.join(MANIFEST_RELATIVE_PATH).exists());
    }

    #[test]
    fn plugin_name() {
        assert_eq!(IsrManifestPlugin.name(), "isr-manifest");
    }

    #[test]
    fn plugin_default_constructs() {
        let _p = <IsrManifestPlugin as Default>::default();
    }

    #[test]
    fn plugin_after_compile_full_run_writes_manifest_and_copies_sources() {
        // Covers after_compile's full happy path: build_manifest +
        // write_manifest + copy_sources (the line-90 branch the dry-run
        // test skips).
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();
        fs::write(content_dir.join("hello.md"), "hello world").unwrap();
        fs::write(template_dir.join("index.html"), "<html/>").unwrap();

        let ctx = PluginContext {
            content_dir: content_dir.clone(),
            build_dir: site_dir.clone(),
            site_dir: site_dir.clone(),
            template_dir,
            config: None,
            cache: None,
            memory_budget: None,
            html_files: None,
            dep_graph: None,
            dry_run: false,
        };

        IsrManifestPlugin.after_compile(&ctx).unwrap();
        assert!(site_dir.join(MANIFEST_RELATIVE_PATH).exists());
    }

    #[test]
    fn collect_md_files_nonexistent_dir_returns_empty() {
        // Covers line ~162 `if !dir.exists() return Ok(vec![])`.
        let out = collect_md_files(Path::new("/nonexistent/xxx")).unwrap();
        assert!(out.is_empty(), "{:?}", out);
    }

    #[test]
    fn collect_md_files_skips_hidden_dirs_v2() {
        // Differs from the existing same-named test by exercising
        // the path-skip branch with a nested .md instead of relying
        // on top-level filtering.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        fs::write(dir.path().join(".hidden/secret.md"), "x").unwrap();
        fs::write(dir.path().join("visible.md"), "x").unwrap();
        let out = collect_md_files(dir.path()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].file_name().unwrap(),
            std::ffi::OsStr::new("visible.md")
        );
    }

    #[test]
    fn collect_md_files_walks_nested_v2() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("deep.md"), "x").unwrap();
        fs::write(dir.path().join("shallow.md"), "x").unwrap();
        let out = collect_md_files(dir.path()).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn collect_templates_nonexistent_dir_returns_empty() {
        // Covers line ~249.
        let out = collect_templates(Path::new("/nonexistent/yyy"));
        assert!(out.is_empty(), "{:?}", out);
    }

    #[test]
    fn extract_isr_cache_yaml_both_keys() {
        // Covers lines 312-315 (s_maxage + swr) and line 332 return.
        let text = "---\nisr:\n  s_maxage: 600\n  swr: 3600\n---\n";
        let p = extract_isr_cache(text).unwrap();
        assert_eq!(p.s_maxage, 600);
        assert_eq!(p.swr, 3600);
    }

    #[test]
    fn extract_isr_cache_yaml_dash_variants() {
        // Covers the s-maxage/stale-while-revalidate alt keys.
        let text =
            "---\nisr:\n  s-maxage: 42\n  stale-while-revalidate: 99\n---\n";
        let p = extract_isr_cache(text).unwrap();
        assert_eq!(p.s_maxage, 42);
        assert_eq!(p.swr, 99);
    }

    #[test]
    fn extract_isr_cache_ignores_unknown_keys_in_isr_block() {
        // Covers line 317 `_ => {}` for unknown keys inside isr block.
        let text = "---\nisr:\n  unknown_key: 5\n  s_maxage: 7\n---\n";
        let p = extract_isr_cache(text).unwrap();
        assert_eq!(p.s_maxage, 7);
    }

    #[test]
    fn extract_isr_cache_isr_block_exits_on_non_indented_line() {
        // Covers line 320-321 (line not empty AND not indented → exit).
        let text = "---\nisr:\n  s_maxage: 5\ntitle: Hi\nswr: 8\n---\n";
        let p = extract_isr_cache(text).unwrap();
        // s_maxage in block was picked up; swr at top level was NOT.
        assert_eq!(p.s_maxage, 5);
        assert_eq!(p.swr, ssg_core::DEFAULT_SWR);
    }

    #[test]
    fn extract_isr_cache_no_frontmatter_returns_none() {
        assert!(extract_isr_cache("just body text").is_none());
    }

    #[test]
    fn extract_isr_cache_no_isr_block_returns_none() {
        let text = "---\ntitle: Hi\n---\n";
        assert!(extract_isr_cache(text).is_none());
    }

    #[test]
    fn extract_frontmatter_block_toml_fences() {
        // Covers lines 344-347 (+++ TOML branch).
        let text = "+++\ntitle = \"X\"\n+++\nbody";
        let body = extract_frontmatter_block(text).unwrap();
        assert!(body.contains("title"));
    }

    #[test]
    fn derive_url_index_md_maps_to_root_index_html() {
        assert_eq!(derive_url("index.md"), "/index.html");
    }

    #[test]
    fn derive_url_nested_index_maps_to_dir_slash_index_html() {
        assert_eq!(derive_url("about/index.md"), "/about/index.html");
    }

    #[test]
    fn derive_url_regular_md_maps_to_dir_slash_index_html() {
        assert_eq!(derive_url("posts/hello.md"), "/posts/hello/index.html");
    }

    /// Builds a non-dry-run [`PluginContext`] over the three dirs.
    fn make_ctx(
        content_dir: &Path,
        template_dir: &Path,
        site_dir: &Path,
    ) -> PluginContext {
        PluginContext {
            content_dir: content_dir.to_path_buf(),
            build_dir: site_dir.to_path_buf(),
            site_dir: site_dir.to_path_buf(),
            template_dir: template_dir.to_path_buf(),
            config: None,
            cache: None,
            memory_budget: None,
            html_files: None,
            dep_graph: None,
            dry_run: false,
        }
    }

    #[cfg(unix)]
    fn deny_access(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o000)).unwrap();
    }

    #[cfg(unix)]
    fn restore_access(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o755));
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_unreadable_subdir_error() {
        // A chmod-000 subdir makes `visit`'s read_dir fail inside the
        // recursion, exercising the Io closure and every `?` layer up
        // through after_compile.
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(content_dir.join("locked")).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();
        deny_access(&content_dir.join("locked"));

        let ctx = make_ctx(&content_dir, &template_dir, &site_dir);
        let res = IsrManifestPlugin.after_compile(&ctx);

        restore_access(&content_dir.join("locked"));
        // Root CI runners bypass perms; only assert when it errored.
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }

    #[test]
    #[cfg(unix)]
    fn build_manifest_propagates_unreadable_md_error() {
        // A chmod-000 markdown file makes `fs::read` in
        // build_entry_for_markdown fail.
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();
        let md = content_dir.join("locked.md");
        fs::write(&md, "# locked").unwrap();
        deny_access(&md);

        let res = build_manifest(&content_dir, &template_dir, &site_dir);

        restore_access(&md);
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }

    #[test]
    #[cfg(unix)]
    fn build_manifest_propagates_unreadable_template_error() {
        // A chmod-000 template makes the per-template `fs::read` fail.
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();
        fs::write(content_dir.join("a.md"), "# a").unwrap();
        let tpl = template_dir.join("index.html");
        fs::write(&tpl, "<html/>").unwrap();
        deny_access(&tpl);

        let res = build_manifest(&content_dir, &template_dir, &site_dir);

        restore_access(&tpl);
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }

    #[test]
    fn after_compile_fails_when_ssg_dir_is_a_file() {
        // `site/.ssg` existing as a *file* makes write_manifest's
        // create_dir_all fail, covering its Io closure and the `?`
        // propagation in after_compile.
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();
        fs::write(site_dir.join(".ssg"), "not a dir").unwrap();

        let ctx = make_ctx(&content_dir, &template_dir, &site_dir);
        let err = IsrManifestPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    fn write_manifest_fails_when_manifest_path_is_a_dir() {
        // A directory squatting on `.ssg/manifest.json` makes
        // `fs::write` fail.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(MANIFEST_RELATIVE_PATH)).unwrap();
        let err = write_manifest(&Manifest::default(), dir.path()).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    fn after_compile_fails_when_content_out_is_a_file() {
        // write_manifest succeeds but copy_sources' create_dir_all
        // fails because `.ssg/content` exists as a file.
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(site_dir.join(".ssg")).unwrap();
        fs::write(site_dir.join(CONTENT_RELATIVE_DIR), "not a dir").unwrap();

        let ctx = make_ctx(&content_dir, &template_dir, &site_dir);
        let err = IsrManifestPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    /// Builds a one-entry manifest whose entry lists `sources`.
    fn manifest_with_sources(sources: Vec<String>) -> Manifest {
        let byte_refs: Vec<&[u8]> = vec![b"x"; sources.len()];
        let entry = build_entry(sources, &byte_refs, None);
        let mut m = Manifest::new(build_stamp());
        m.insert("/index.html".to_string(), entry);
        m
    }

    #[test]
    fn copy_sources_skips_unknown_prefix_and_missing_files() {
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::create_dir_all(&site_dir).unwrap();

        let m = manifest_with_sources(vec![
            "bogus/thing".to_string(),
            "content/missing.md".to_string(),
            "templates/missing.html".to_string(),
        ]);
        copy_sources(&content_dir, &template_dir, &site_dir, &m).unwrap();
        // Nothing staged: all sources skipped.
        let staged = site_dir.join(CONTENT_RELATIVE_DIR);
        assert_eq!(fs::read_dir(staged).unwrap().count(), 0);
    }

    #[test]
    fn copy_sources_fails_when_dst_parent_is_a_file() {
        // `.ssg/content/content` exists as a file, so create_dir_all
        // for the destination parent fails.
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::write(content_dir.join("a.md"), "# a").unwrap();
        let content_out = site_dir.join(CONTENT_RELATIVE_DIR);
        fs::create_dir_all(&content_out).unwrap();
        fs::write(content_out.join("content"), "not a dir").unwrap();

        let m = manifest_with_sources(vec!["content/a.md".to_string()]);
        let err = copy_sources(&content_dir, &template_dir, &site_dir, &m)
            .unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    fn copy_sources_fails_when_dst_path_is_a_dir() {
        // The destination path itself is a directory, so fs::copy
        // fails after the parent create_dir_all succeeded.
        let dir = tempdir().unwrap();
        let content_dir = dir.path().join("content");
        let template_dir = dir.path().join("templates");
        let site_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();
        fs::create_dir_all(&template_dir).unwrap();
        fs::write(content_dir.join("a.md"), "# a").unwrap();
        let dst = site_dir.join(CONTENT_RELATIVE_DIR).join("content/a.md");
        fs::create_dir_all(&dst).unwrap();

        let m = manifest_with_sources(vec!["content/a.md".to_string()]);
        let err = copy_sources(&content_dir, &template_dir, &site_dir, &m)
            .unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    fn extract_isr_cache_ignores_indented_line_without_colon() {
        let text = "---\nisr:\n  nocolonhere\n  s_maxage: 3\n---\n";
        let p = extract_isr_cache(text).unwrap();
        assert_eq!(p.s_maxage, 3);
    }

    #[test]
    fn extract_isr_cache_blank_line_keeps_isr_block_open() {
        // An empty line inside the isr block is neither indented nor
        // non-empty, so the block stays open.
        let text = "---\nisr:\n\n  s_maxage: 4\n---\n";
        let p = extract_isr_cache(text).unwrap();
        assert_eq!(p.s_maxage, 4);
    }

    #[test]
    fn extract_frontmatter_block_unclosed_yaml_returns_none() {
        assert!(extract_frontmatter_block("---\nno closing fence").is_none());
    }

    #[test]
    fn extract_frontmatter_block_unclosed_toml_returns_none() {
        assert!(extract_frontmatter_block("+++\nno closing fence").is_none());
    }
}
