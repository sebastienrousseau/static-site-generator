// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Content staging — residual upstream-gap workarounds for the
//! `staticdatagen` → `staticweaver` → `metadata-gen` pipeline.
//!
//! ## v0.0.46 residual scope
//!
//! Most of v0.0.45's shim layer was retired in v0.0.46 by upstream
//! fixes:
//!
//! - `staticdatagen 0.0.10` (closes upstream `#67`, `#68`, `#69`,
//!   `#70`, `#71`) handles missing `layout:` keys, absent
//!   `main.js`/`sw.js`, absent tags-page templates, nested-locale
//!   walk (`_posts/<lang>/`), and success-log ordering natively.
//! - `staticweaver 0.0.3` (closes upstream `#28`) added the
//!   `Engine::with_lax_undefined(true)` opt-in.
//! - `staticweaver 0.0.3` also made `escape_html_into` idempotent
//!   (closes [ssg #589](https://github.com/sebastienrousseau/static-site-generator/issues/589)).
//! - `rss-gen 0.0.6` (closes upstream `#34`) prefixed validation
//!   errors with `channel.` / `item.` context and accepts
//!   relative URLs at item level.
//! - `metadata-gen 0.0.5` (closes upstream `#20`) collapses
//!   multi-line double-quoted YAML scalars internally.
//!
//! What remains here are **two** narrow gaps the upstreams haven't
//! closed yet:
//!
//! 1. **Template-default injection.** `staticdatagen 0.0.10` doesn't
//!    yet opt the staticweaver Engine into `lax_undefined` (tracked at
//!    [staticdatagen #99](https://github.com/sebastienrousseau/staticdatagen/issues/99)),
//!    so unresolved `{{ var }}` tags still abort the build.
//!    [`collect_template_vars`] + [`stage_content_with_template_defaults`]
//!    pre-fill an empty `var: ""` for every key the templates reference
//!    but the content omits.
//!
//! 2. **Multi-line quoted-scalar collapse.** `staticdatagen 0.0.10`
//!    pins `metadata-gen = "0.0.4"` (the pre-#20 release; tracked at
//!    [staticdatagen #100](https://github.com/sebastienrousseau/staticdatagen/issues/100)).
//!    `copy_tree` (the per-file staging helper) applies the same collapse pass that's now upstream
//!    in `metadata-gen 0.0.5`, so the user's content sees consistent
//!    behaviour regardless of which `metadata-gen` is transitively
//!    resolved.
//!
//! Both shims auto-retire when the corresponding staticdatagen follow-up
//! releases — the residual module shrinks to nothing.
//!
//! ## Permalink derivation (spec A2/B1, plan §2 item 1.2, issue #586)
//!
//! `staticdatagen`'s RSS generator hard-fails the whole build when a
//! post lacks `permalink:` front matter (`rss-gen`: "channel.link is
//! missing"). Because every page passes through this stager before
//! `staticdatagen::compile`, the stager can make that failure
//! unreachable: when a staged `.md` file's frontmatter carries
//! neither `permalink` nor `url`, [`stage_content_with_site_defaults`]
//! injects `permalink: "{base_url}/{relative_output_path}"` derived
//! via [`crate::urls::derive_permalink`]. Author-specified permalinks
//! always win — files that already declare `permalink` or `url` pass
//! through verbatim. Only YAML `---` fenced frontmatter flows through
//! this stager (the template-default shim shares the same
//! constraint); files without a frontmatter block are left untouched.
//!
//! ## Why staging instead of editing in-place?
//!
//! The user's checkout is sacred:
//!
//! - the build runs from a CI checkout the user expects to be read-only;
//! - reruns of the build would re-inject defaults, doubling lines.
//!
//! Instead we operate on a fresh directory under
//! [`std::env::temp_dir()`] (FNV-1a-keyed by `build_dir` + pid) that's
//! recreated on every build.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Stages a copy of `content_dir` and injects empty defaults for every
/// `{{ var }}` reference the templates make.
///
/// Works around the staticweaver "Unresolved template tag" crash that
/// fires when user content omits a key their template references —
/// `staticdatagen 0.0.10` still calls `Engine::new(...)` without
/// `.with_lax_undefined(true)`. Tracked at [staticdatagen #99].
///
/// `template_var_keys` is the result of [`collect_template_vars`] run
/// over the user's template directory.
///
/// Convenience wrapper over [`stage_content_with_site_defaults`] with
/// no base URL — no `permalink:` derivation happens on staged content.
///
/// [staticdatagen #99]: https://github.com/sebastienrousseau/staticdatagen/issues/99
///
/// # Errors
///
/// Returns [`io::Error`] when the staging directory cannot be created
/// or a source file cannot be read or written.
pub fn stage_content_with_template_defaults(
    content_dir: &Path,
    build_dir: &Path,
    template_var_keys: &[String],
) -> Result<PathBuf, io::Error> {
    stage_content_with_site_defaults(
        content_dir,
        build_dir,
        template_var_keys,
        None,
        &[],
    )
}

/// Like [`stage_content_with_template_defaults`] but also derives a
/// `permalink:` for staged `.md` files that don't declare one.
///
/// Injection targets every staged `.md` file whose frontmatter
/// carries neither `permalink` nor `url` (spec A2/B1, plan §2 item
/// 1.2, issue #586). The derived value comes from
/// [`crate::urls::derive_permalink`] applied to `base_url` and the
/// file's content-relative path — i.e.
/// `{base_url}/{relative_output_path}` under the compiler's
/// `foo.md → foo/index.html` output convention, published as a pretty
/// directory URL (`{base_url}/foo/`). This guarantees `staticdatagen`
/// / `rss-gen` always see a channel/item link and can never abort the
/// build with "channel.link is missing".
///
/// Passing `base_url: None` (or an empty/whitespace-only base URL)
/// disables permalink injection and behaves exactly like
/// [`stage_content_with_template_defaults`] — an rss-gen-valid
/// permalink must be an *absolute* URL, so there is nothing useful to
/// derive without a base.
///
/// Author-specified front matter always wins: files that already
/// declare `permalink` or `url` pass through verbatim.
///
/// # Errors
///
/// Returns [`io::Error`] when the staging directory cannot be created
/// or a source file cannot be read or written.
///
/// # Examples
///
/// ```rust
/// use ssg::content_stager::stage_content_with_site_defaults;
/// use std::fs;
///
/// let tmp = tempfile::tempdir().unwrap();
/// let src = tmp.path().join("content");
/// let build = tmp.path().join("build");
/// fs::create_dir_all(&src).unwrap();
/// fs::write(src.join("post.md"), "---\ntitle: A\n---\nbody").unwrap();
///
/// let staged = stage_content_with_site_defaults(
///     &src, &build, &[], Some("https://example.com"), &[],
/// ).unwrap();
/// let out = fs::read_to_string(staged.join("post.md")).unwrap();
/// assert!(out.contains("permalink: \"https://example.com/post/\""));
/// ```
pub fn stage_content_with_site_defaults(
    content_dir: &Path,
    build_dir: &Path,
    template_var_keys: &[String],
    base_url: Option<&str>,
    locales: &[String],
) -> Result<PathBuf, io::Error> {
    // An empty base URL can't produce the absolute permalink rss-gen
    // validates for — treat it as "no base URL, skip injection".
    let base_url = base_url.map(str::trim).filter(|b| !b.is_empty());

    let staging_dir = staging_root_for("content", build_dir);
    recreate_staging_dir(&staging_dir)?;

    copy_tree(content_dir, &staging_dir, base_url)?;

    // staticdatagen 0.0.10 closes upstream #69 — the tags-page generator
    // is now a no-op when no `tags.md` / `tags/index.md` template is
    // present. The v0.0.45 `ensure_tags_stub` shim was retired in this
    // release.

    if !template_var_keys.is_empty() {
        inject_template_defaults_recursive(
            &staging_dir,
            template_var_keys,
            base_url,
            locales,
        )?;
    }

    Ok(staging_dir)
}

/// Front-matter keys derived from `base_url` and the page's own location.
///
/// Two scopes, named so the call site says which one it means:
///
/// | Key | Value for `fr/a-propos.md` under `https://example.com/atlas` |
/// | --- | --- |
/// | `site_path`   | `/atlas/` |
/// | `site_url`    | `https://example.com/atlas/` |
/// | `locale_path` | `/atlas/fr/` |
/// | `locale_url`  | `https://example.com/atlas/fr/` |
///
/// `site_*` addresses the site root, where assets, feeds, the manifest and
/// the favicon are published — there is exactly one copy of each per site,
/// regardless of locale. `locale_*` addresses the current locale's root,
/// where page-to-page links live.
///
/// Conflating the two is not hypothetical: the previous hand-maintained
/// `base_path` / `asset_path` pair carried no scope in either name, and a
/// French page duly requested `/atlas/fr/styles.css`, which is never
/// written. `{{site_path}}styles.css` and `{{locale_path}}articles/` both
/// read correctly at the call site, and `{{locale_path}}styles.css` reads
/// visibly wrong.
///
/// Every value carries a trailing slash so templates concatenate without a
/// separator. On a single-locale site `locale_*` equals `site_*`, so a theme
/// can use the locale forms throughout and gain locales later without
/// editing content.
///
/// Author front matter always wins — these are only injected when absent.
pub(crate) const DERIVED_PATH_KEYS: [&str; 4] =
    ["site_path", "site_url", "locale_path", "locale_url"];

/// Computes [`DERIVED_PATH_KEYS`] for one staged file.
///
/// `staged_rel` is the file's path relative to the staging root, e.g.
/// `fr/a-propos.md`. A locale is recognised either as a leading directory
/// (`fr/a-propos.md`) or, for a locale home page, as the whole stem
/// (`fr.md` — which the nested-index flattening produces and which compiles
/// to `fr/index.html`).
fn derive_path_globals(
    base_url: Option<&str>,
    staged_rel: &Path,
    locales: &[String],
) -> Vec<(String, String)> {
    let site_url = base_url.map_or_else(
        || "/".to_string(),
        |b| format!("{}/", b.trim_end_matches('/')),
    );
    let site_path = url_path_component(&site_url);

    let locale = detect_locale(staged_rel, locales);
    let (locale_url, locale_path) = match locale {
        Some(l) => (format!("{site_url}{l}/"), format!("{site_path}{l}/")),
        None => (site_url.clone(), site_path.clone()),
    };

    // Zipped against the const so the documented key list and the values
    // actually injected cannot drift apart.
    DERIVED_PATH_KEYS
        .into_iter()
        .map(str::to_string)
        .zip([site_path, site_url, locale_path, locale_url])
        .collect()
}

/// Returns the path component of an absolute URL, with a trailing slash.
///
/// `https://example.com/atlas/` yields `/atlas/`; a bare origin, or a value
/// that is already a path, yields `/`.
fn url_path_component(url: &str) -> String {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = if url.starts_with('/') {
        url
    } else {
        after_scheme.find('/').map_or("/", |i| &after_scheme[i..])
    };
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("/{trimmed}/")
    }
}

/// Identifies which configured locale a staged file belongs to.
fn detect_locale(staged_rel: &Path, locales: &[String]) -> Option<String> {
    if locales.len() < 2 {
        return None;
    }
    let mut comps = staged_rel.components();
    let first = comps.next()?.as_os_str().to_string_lossy().into_owned();

    // `fr/a-propos.md` — locale as a directory.
    if comps.next().is_some() && locales.contains(&first) {
        return Some(first);
    }
    // `fr.md` — a locale home page, flattened from `fr/index.md`.
    let stem = Path::new(&first)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())?;
    locales.contains(&stem).then_some(stem)
}

/// Recreates the staging directory from scratch so a previous build's
/// layout injection doesn't leak into this build's inputs.
fn recreate_staging_dir(staging_dir: &Path) -> Result<(), io::Error> {
    if staging_dir.exists() {
        fs::remove_dir_all(staging_dir)?;
    }
    fs::create_dir_all(staging_dir)?;
    Ok(())
}

/// Picks a staging-directory location *outside* `build_dir` so the
/// staged tree never gets swept into staticdatagen's output.
///
/// Key derivation includes:
/// - the OS temp dir (`std::env::temp_dir()`)
/// - the process id (disambiguates concurrent `ssg` invocations)
/// - a stable FNV-1a hash of `build_dir`'s absolute path
///   (disambiguates concurrent in-process callers — load-bearing for
///   the parallel test runner, where every test shares the same pid)
/// - a per-purpose suffix (`content` / `templates`)
///
/// The hash is FNV-1a 64-bit rather than `DefaultHasher` to keep the
/// path deterministic across runs (good for debugging) while staying
/// well-distributed across the `usize` space.
fn staging_root_for(suffix: &str, build_dir: &Path) -> PathBuf {
    // FNV-1a 64-bit over the build_dir bytes — deterministic and
    // adequate for path disambiguation.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in build_dir.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    std::env::temp_dir().join(format!(
        "ssg-staging-{}-{hash:016x}-{suffix}",
        std::process::id()
    ))
}

/// Recursively mirrors `src` into `dst`, applying
/// [`collapse_multiline_quoted_scalars`] to `.md` files (and, when
/// `base_url` is present, [`inject_permalink_if_missing`] — spec
/// A2/B1, plan §2 item 1.2, issue #586) and copying everything else
/// verbatim. Per-file work runs in parallel via Rayon
/// — `read + transform + write` is the hot path that determined
/// whether the staging shim added meaningful build-time overhead.
/// With ~100 pages the parallel pass keeps the staging cost under
/// ~50 ms on a 4-core runner, well inside the perf-budget gate.
fn copy_tree(
    src: &Path,
    dst: &Path,
    base_url: Option<&str>,
) -> Result<(), io::Error> {
    use rayon::iter::IntoParallelIterator;
    use rayon::iter::ParallelIterator;

    // Collect entries up front so the directory-creation step is
    // serial (cheap) and the per-file work is parallel.
    let mut files: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    let mut dirs: Vec<(PathBuf, PathBuf)> = Vec::new();
    collect_entries(src, dst, &mut files, &mut dirs)?;
    flatten_nested_index_pages(dst, &mut files);

    // Create every directory serially (cheap; preserves order so
    // children can be written without races).
    for (_src_dir, dst_dir) in &dirs {
        fs::create_dir_all(dst_dir)?;
    }

    let errors: Vec<io::Error> = files
        .into_par_iter()
        .filter_map(|(src_path, dst_path, is_md)| {
            let r = if is_md {
                // staticdatagen 0.0.10 closes upstream #67 — missing
                // `layout:` keys default to "page" inside the compiler
                // itself. Per-file work is now ONLY the multi-line
                // quoted-scalar collapse (still needed until
                // staticdatagen bumps `metadata-gen` to 0.0.5;
                // tracked: staticdatagen#100).
                fs::read_to_string(&src_path).and_then(|body| {
                    let mut staged = collapse_multiline_quoted_scalars(&body);
                    // Guarantee a permalink so rss-gen's
                    // "channel.link is missing" hard-fail is
                    // unreachable (spec A2/B1, plan §2 item 1.2,
                    // issue #586). Author-specified `permalink`/`url`
                    // keys always win — see
                    // `inject_permalink_if_missing`.
                    if let Some(base) = base_url {
                        // `strip_prefix(src)` cannot fail here: every
                        // `src_path` in `files` was built by
                        // `collect_entries` descending from this same
                        // `src` via repeated `.join()` calls, so it is
                        // always literally prefixed by `src`. The `Err`
                        // arm is unreachable through the public API and
                        // is kept only as defensive protection against a
                        // future refactor of `collect_entries` /
                        // `copy_tree`'s call graph — not covered by
                        // tests for that reason (100% coverage
                        // verification, v0.0.47).
                        if let Ok(rel) = src_path.strip_prefix(src) {
                            let rel = rel.to_string_lossy();
                            let permalink =
                                crate::urls::derive_permalink(base, &rel);
                            staged = inject_permalink_if_missing(
                                &staged, &permalink,
                            );
                        }
                    }
                    fs::write(&dst_path, staged)
                })
            } else {
                fs::copy(&src_path, &dst_path).map(|_| ())
            };
            r.err()
        })
        .collect();

    if let Some(e) = errors.into_iter().next() {
        return Err(e);
    }
    Ok(())
}

/// Re-targets a staged nested `index.md` onto its parent directory's
/// name so `staticdatagen` writes it to `<parent>/index.html` instead
/// of `<parent>/index/index.html`.
///
/// ## Why this exists
///
/// [`crate::urls::derive_output_rel_path`] documents the compiler's
/// output convention as `about/index.md → about/index.html`, and the
/// ISR manifest's `derive_url` repeats it. `staticdatagen 0.0.11`
/// does not honour it:
/// `utilities::write::write_files_to_build_directory` compares the
/// *whole* processed name against `"index"`, so only a content-root
/// `index.md` reaches the root-index branch. Anything nested —
/// `fr/index.md` — falls through to `write_content_files`, which
/// creates a directory named after the full stem (`fr/index/`) and
/// writes `index.html` inside it. Every per-locale home page is
/// affected.
///
/// `staticdatagen` is a published external dependency, so the mapping
/// cannot be corrected in-repo. It can be side-stepped: the compiler
/// writes `foo.md` to `foo/index.html`, so staging `fr/index.md` under
/// the name `fr.md` produces exactly the documented output path.
///
/// ## Collisions
///
/// If the content tree already carries a file that would stage to the
/// same destination (both `fr.md` and `fr/index.md` authored), the
/// nested file keeps its original staged path. Flattening would
/// silently drop one of the two pages, which is worse than the
/// directory-level bug.
///
/// Directory names containing dots are appended to textually rather
/// than through `Path::set_extension`, which would truncate `v1.2`
/// to `v1`.
fn flatten_nested_index_pages(
    dst_root: &Path,
    files: &mut [(PathBuf, PathBuf, bool)],
) {
    let occupied: std::collections::HashSet<PathBuf> =
        files.iter().map(|(_, dst, _)| dst.clone()).collect();

    for (_src_path, dst_path, is_md) in files.iter_mut() {
        if !*is_md {
            continue;
        }
        if dst_path.file_stem().and_then(|s| s.to_str()) != Some("index") {
            continue;
        }
        // A content-root `index.md` already compiles to the site
        // root's `index.html` — only nested ones are wrong.
        let Some(parent) = dst_path.parent() else {
            continue;
        };
        if parent == dst_root {
            continue;
        }
        let (Some(grandparent), Some(dir_name)) =
            (parent.parent(), parent.file_name())
        else {
            continue;
        };
        let ext = dst_path.extension().unwrap_or_default().to_string_lossy();
        let renamed =
            grandparent.join(format!("{}.{ext}", dir_name.to_string_lossy()));
        if occupied.contains(&renamed) {
            continue;
        }
        *dst_path = renamed;
    }
}

/// Build-time control files that live *in* the content directory but are
/// inputs to `ssg` itself, not pages to compile.
///
/// `content.schema.toml` is the documented location for typed front-matter
/// schemas (see the "Content schema validation" section of the README).
/// It is read by [`crate::core_group::content`] before the compile, and it
/// must not then be handed to `staticdatagen`, which treats every staged
/// file as a page and aborts the whole build with
/// `Failed to extract metadata: No valid front matter found`.
const CONTENT_CONTROL_FILES: &[&str] = &["content.schema.toml"];

/// Walks `src` and partitions entries into directories (to create
/// before parallel writes) and files (with their destination path
/// and a markdown flag).
///
/// Entries named in [`CONTENT_CONTROL_FILES`] are skipped: they configure
/// the build rather than describing a page.
fn collect_entries(
    src: &Path,
    dst: &Path,
    files: &mut Vec<(PathBuf, PathBuf, bool)>,
    dirs: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), io::Error> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let src_path = entry.path();
        let dst_path = dst.join(&file_name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            dirs.push((src_path.clone(), dst_path.clone()));
            collect_entries(&src_path, &dst_path, files, dirs)?;
        } else if file_type.is_file() {
            if CONTENT_CONTROL_FILES
                .iter()
                .any(|name| file_name.as_encoded_bytes() == name.as_bytes())
            {
                continue;
            }
            let is_md = is_markdown(&src_path);
            files.push((src_path, dst_path, is_md));
        }
        // Symlinks and other special files are skipped — staticdatagen
        // wouldn't follow them safely anyway.
    }
    Ok(())
}

/// Walks `template_dir` recursively and returns the sorted, deduped
/// set of every `{{ <var> }}` reference found in the template files.
///
/// Filtered references (`{{ var | filter }}`), dotted paths (`{{ a.b }}`),
/// helpers (`{{#each ...}}`), and the `{{!...}}` raw-emit form are all
/// skipped — only bare top-level keys end up in the result, because
/// those are the ones staticweaver actually looks up against the
/// frontmatter `metadata` `HashMap`.
///
/// Returns an empty Vec if `template_dir` doesn't exist.
///
/// # Errors
///
/// Returns [`io::Error`] only for unexpected failures reading the
/// template tree; a missing directory is treated as "no templates",
/// not as an error.
///
/// # Examples
///
/// ```rust
/// use ssg::content_stager::collect_template_vars;
/// use std::fs;
///
/// let tmp = tempfile::tempdir().unwrap();
/// let t = tmp.path().join("t");
/// fs::create_dir_all(&t).unwrap();
/// fs::write(t.join("page.html"), "<title>{{ title }}</title>{{ author }}").unwrap();
///
/// let vars = collect_template_vars(&t).unwrap();
/// assert!(vars.contains(&"title".to_string()));
/// assert!(vars.contains(&"author".to_string()));
/// ```
pub fn collect_template_vars(
    template_dir: &Path,
) -> Result<Vec<String>, io::Error> {
    let mut out = std::collections::BTreeSet::new();
    if !template_dir.exists() {
        return Ok(Vec::new());
    }
    walk_collect_vars(template_dir, &mut out)?;
    Ok(out.into_iter().collect())
}

fn walk_collect_vars(
    dir: &Path,
    out: &mut std::collections::BTreeSet<String>,
) -> Result<(), io::Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk_collect_vars(&p, out)?;
        } else if ft.is_file() {
            // Only scan templating-eligible files; ignore .js, .css,
            // etc. that happen to live under template_dir.
            let is_template = matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("html" | "htm" | "xml" | "txt" | "rss")
            );
            if is_template {
                if let Ok(body) = fs::read_to_string(&p) {
                    extract_simple_vars(&body, out);
                }
            }
        }
    }
    Ok(())
}

/// Extracts bare `{{ key }}` references from `body` into `out`.
/// Skips refs that include filters (`|`), dotted paths (`.`), or
/// staticweaver helpers (`#`, `/`, `!`, `>`).
fn extract_simple_vars(
    body: &str,
    out: &mut std::collections::BTreeSet<String>,
) {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Walk to the matching `}}`.
            if let Some(end) = find_closing_braces(&body[i + 2..]) {
                let inner = body[i + 2..i + 2 + end].trim();
                if let Some(name) = simple_var_name(inner) {
                    let _ = out.insert(name.to_string());
                }
                i += 2 + end + 2;
                continue;
            }
        }
        i += 1;
    }
}

const fn find_closing_braces(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut j = 0;
    while j + 1 < bytes.len() {
        if bytes[j] == b'}' && bytes[j + 1] == b'}' {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// Returns Some(name) when `inner` is a simple `<key>` lookup —
/// no filter (`|`), no dotted path (`.`), no helper prefix
/// (`#`, `/`, `!`, `>`), no whitespace inside the name.
fn simple_var_name(inner: &str) -> Option<&str> {
    let s = inner.trim();
    if s.is_empty() {
        return None;
    }
    let first = s.as_bytes()[0];
    if matches!(first, b'#' | b'/' | b'!' | b'>') {
        return None;
    }
    if s.contains('|') || s.contains('.') {
        return None;
    }
    if s.bytes().any(|b| b.is_ascii_whitespace()) {
        return None;
    }
    Some(s)
}

/// Walks every `.md` file under `dir` and injects empty defaults for
/// every key in `keys` not already present in that file's frontmatter.
/// Per-file work is parallelised via Rayon so the staging cost on a
/// 100-page corpus stays inside the perf-budget gate.
fn inject_template_defaults_recursive(
    dir: &Path,
    keys: &[String],
    base_url: Option<&str>,
    locales: &[String],
) -> Result<(), io::Error> {
    use rayon::iter::IntoParallelIterator;
    use rayon::iter::ParallelIterator;

    fail_point!("content_stager::inject-defaults", |_| {
        Err(io::Error::other(
            "injected: content_stager::inject-defaults",
        ))
    });

    let mut md_files: Vec<PathBuf> = Vec::new();
    collect_markdown_files(dir, &mut md_files)?;

    let errors: Vec<io::Error> = md_files
        .into_par_iter()
        .filter_map(|p| {
            let body = match fs::read_to_string(&p) {
                Ok(b) => b,
                Err(e) => return Some(e),
            };
            // Only derive what the templates actually reference: a theme
            // that never writes `{{site_path}}` pays nothing, and the
            // skip-write path below stays reachable.
            let rel = p.strip_prefix(dir).unwrap_or(&p);
            let derived: Vec<(String, String)> =
                derive_path_globals(base_url, rel, locales)
                    .into_iter()
                    .filter(|(k, _)| keys.iter().any(|want| want == k))
                    .collect();
            let staged = inject_missing_keys_with_values(&body, keys, &derived);
            if staged == body {
                return None;
            }
            fs::write(&p, staged).err()
        })
        .collect();
    if let Some(e) = errors.into_iter().next() {
        return Err(e);
    }
    Ok(())
}

fn collect_markdown_files(
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), io::Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_markdown_files(&p, out)?;
        } else if ft.is_file() && is_markdown(&p) {
            out.push(p);
        }
    }
    Ok(())
}

/// Injects empty `key: ""` entries into the frontmatter block for any
/// `key` not already present. No-op for files without a frontmatter
/// block.
#[must_use]
pub fn inject_missing_keys(body: &str, keys: &[String]) -> String {
    inject_missing_keys_with_values(body, keys, &[])
}

/// As [`inject_missing_keys`], but `derived` supplies real values for the
/// keys it names instead of the empty-string placeholder.
///
/// Author front matter wins: a key already present in the block is left
/// exactly as written, so a theme can override any derived value — a locale
/// tree served from a different origin, say — without fighting the default.
pub fn inject_missing_keys_with_values(
    body: &str,
    keys: &[String],
    derived: &[(String, String)],
) -> String {
    let trimmed = body.trim_start_matches('\u{FEFF}');
    let Some((_lead, after_open)) = find_opening_fence(trimmed) else {
        return body.to_string();
    };
    let Some(close_rel) = find_closing_fence(after_open) else {
        return body.to_string();
    };
    let block = &after_open[..close_rel];
    let after_block = &after_open[close_rel..];

    let derived_keys: Vec<String> =
        derived.iter().map(|(k, _)| k.clone()).collect();
    let missing: Vec<&String> = keys
        .iter()
        .chain(derived_keys.iter())
        .filter(|k| !frontmatter_has_key(block, k))
        .collect();
    if missing.is_empty() {
        return body.to_string();
    }

    let mut additions = String::with_capacity(missing.len() * 24);
    let mut seen: Vec<&str> = Vec::with_capacity(missing.len());
    for k in missing {
        if seen.contains(&k.as_str()) {
            continue;
        }
        seen.push(k.as_str());
        let value = derived
            .iter()
            .find(|(dk, _)| dk == k)
            .map_or("", |(_, v)| v.as_str());
        additions.push_str(&format!("{k}: \"{value}\"\n"));
    }

    let mut out = String::with_capacity(body.len() + additions.len());
    out.push_str(&trimmed[..trimmed.len() - after_open.len()]);
    out.push_str(&additions);
    out.push_str(block);
    out.push_str(after_block);
    if body.starts_with('\u{FEFF}') {
        return format!("\u{FEFF}{out}");
    }
    out
}

/// Injects `permalink: "<permalink>"` as the first key of the YAML
/// frontmatter block when the block exists and declares *neither*
/// `permalink` nor `url` (spec A2/B1, plan §2 item 1.2, issue #586).
///
/// Author-specified values always win: a file with either key passes
/// through byte-for-byte. Files without a frontmatter fence are left
/// untouched — the stager's structural line-scan (shared with the
/// template-default shim) only operates on YAML `---` fenced blocks,
/// and `staticdatagen` extracts no metadata from fence-less files
/// anyway.
///
/// Idempotent: a second pass over previously-staged content returns
/// the input unchanged (the injected `permalink:` is detected as an
/// existing key).
///
/// # Examples
///
/// ```rust
/// use ssg::content_stager::inject_permalink_if_missing;
///
/// // Missing both keys — derived permalink lands as the first key.
/// let out = inject_permalink_if_missing(
///     "---\ntitle: T\n---\nbody",
///     "https://example.com/t/",
/// );
/// assert!(out.contains("permalink: \"https://example.com/t/\""));
///
/// // Author-specified permalink wins verbatim.
/// let with_permalink = "---\npermalink: /mine/\ntitle: T\n---\nbody";
/// assert_eq!(
///     inject_permalink_if_missing(with_permalink, "https://x/"),
///     with_permalink
/// );
///
/// // `url` counts as author-specified too.
/// let with_url = "---\nurl: /u/\ntitle: T\n---\nbody";
/// assert_eq!(inject_permalink_if_missing(with_url, "https://x/"), with_url);
/// ```
#[must_use]
pub fn inject_permalink_if_missing(body: &str, permalink: &str) -> String {
    let trimmed = body.trim_start_matches('\u{FEFF}');
    let Some((_lead, after_open)) = find_opening_fence(trimmed) else {
        return body.to_string();
    };
    let Some(close_rel) = find_closing_fence(after_open) else {
        return body.to_string();
    };
    let block = &after_open[..close_rel];
    let after_block = &after_open[close_rel..];

    // Author-specified link keys always win (spec B1): `permalink`
    // is what staticdatagen reads; `url` is the common alias authors
    // migrating from other generators carry.
    if frontmatter_has_key(block, "permalink")
        || frontmatter_has_key(block, "url")
    {
        return body.to_string();
    }

    let mut out = String::with_capacity(body.len() + permalink.len() + 16);
    out.push_str(&trimmed[..trimmed.len() - after_open.len()]);
    out.push_str(&format!("permalink: \"{permalink}\"\n"));
    out.push_str(block);
    out.push_str(after_block);
    if body.starts_with('\u{FEFF}') {
        return format!("\u{FEFF}{out}");
    }
    out
}

/// Returns `true` if the frontmatter block declares `key` (any
/// quoting style, comments skipped).
fn frontmatter_has_key(block: &str, key: &str) -> bool {
    for raw in block.lines() {
        let line = raw.trim_start();
        if line.starts_with('#') {
            continue;
        }
        let prefixes = [
            format!("{key}:"),
            format!("{key} :"),
            format!("\"{key}\":"),
            format!("'{key}':"),
        ];
        if prefixes.iter().any(|p| line.starts_with(p)) {
            return true;
        }
    }
    false
}

fn is_markdown(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()),
        Some("md" | "markdown")
    )
}

/// Collapses YAML-spec-compliant multi-line quoted scalars onto a
/// single line so noyalib's stricter parser inside `metadata-gen`
/// can read them.
///
/// Pattern handled (the common one in the wild):
///
/// ```yaml
/// key: "
/// value-on-next-line"
/// ```
///
/// becomes
///
/// ```yaml
/// key: "value-on-next-line"
/// ```
///
/// The collapse joins lines until the matching closing quote is seen
/// on a subsequent line, replacing intervening newlines with a single
/// space (the YAML semantic for folded line breaks inside double-
/// quoted scalars).
fn collapse_multiline_quoted_scalars(block: &str) -> String {
    let mut out = String::with_capacity(block.len());
    let lines: Vec<&str> = block.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // Detect `key: "` (opening quote, nothing after).
        if let Some(eq_pos) = line.find(": \"") {
            let after_quote = &line[eq_pos + 3..];
            // The quote sits at the very end of the line (only
            // whitespace allowed after) AND no closing `"` on this
            // line — multi-line case.
            if after_quote.trim().is_empty() {
                // Walk forward joining lines until we find the
                // closing `"`.
                let mut joined = String::from(&line[..eq_pos + 3]);
                let mut closed = false;
                i += 1;
                while i < lines.len() {
                    let next = lines[i];
                    if let Some(close) = next.find('"') {
                        joined.push_str(next[..close].trim_start());
                        joined.push_str(&next[close..]);
                        out.push_str(&joined);
                        out.push('\n');
                        i += 1;
                        closed = true;
                        break;
                    }
                    joined.push_str(next.trim_start());
                    joined.push(' ');
                    i += 1;
                }
                // Pathological case — no closing quote in the file.
                // Emit what we've accumulated so the downstream
                // extractor sees the same broken content rather than
                // silently swallowing it.
                if !closed {
                    out.push_str(joined.trim_end());
                    out.push('\n');
                }
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
        i += 1;
    }
    out
}

/// Returns `(prefix, after_open_fence)` where `prefix` is the bytes
/// before the first `---\n` (i.e. leading blank lines) and
/// `after_open_fence` starts at the first character past the fence.
fn find_opening_fence(s: &str) -> Option<(&str, &str)> {
    // Walk lines until we see a non-empty one. If it's exactly `---`
    // (with optional CR), the fence opens. Anything else means no
    // frontmatter.
    let mut byte_pos = 0;
    for line in s.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n').trim_end_matches('\r');
        if bare.trim().is_empty() {
            byte_pos += line.len();
            continue;
        }
        if bare == "---" {
            let lead = &s[..byte_pos];
            let after = &s[byte_pos + line.len()..];
            return Some((lead, after));
        }
        return None;
    }
    None
}

/// Returns the byte offset within `after_open` at which the closing
/// `---` fence begins (the offset is the start of the closing fence
/// line, not past it).
fn find_closing_fence(after_open: &str) -> Option<usize> {
    let mut byte_pos = 0;
    for line in after_open.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n').trim_end_matches('\r');
        if bare.trim() == "---" {
            return Some(byte_pos);
        }
        byte_pos += line.len();
    }
    None
}

// ---------------------------------------------------------------------
// Template staging
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_missing_keys_no_frontmatter_passthrough() {
        // Covers `let Some(...) = find_opening_fence(...) else { return body.to_string(); }`.
        let body = "no frontmatter here\nplain markdown";
        let out = inject_missing_keys(body, &["x".to_string()]);
        assert_eq!(out, body);
    }

    #[test]
    fn inject_missing_keys_unterminated_frontmatter_passthrough() {
        // Covers `let Some(...) = find_closing_fence(...) else { return body.to_string(); }`.
        let body = "---\ntitle: T\n# never closes";
        let out = inject_missing_keys(body, &["x".to_string()]);
        assert_eq!(out, body);
    }

    #[test]
    fn inject_missing_keys_with_bom_preserves_bom() {
        // Covers the `if body.starts_with('\u{FEFF}') { format!("\u{FEFF}{out}") }`
        // branch inside `inject_missing_keys` — distinct from the
        // existing `bom_is_preserved` test which only exercises the
        // sibling layout-injection function.
        let body = "\u{FEFF}---\ntitle: T\n---\nbody";
        let out = inject_missing_keys(body, &["author".to_string()]);
        assert!(out.starts_with('\u{FEFF}'));
        assert!(out.contains("author: \"\""));
    }

    #[test]
    fn inject_missing_keys_returns_body_unchanged_when_all_keys_present() {
        // Covers the `if missing.is_empty() { return body.to_string(); }`
        // arm — distinct from the no-frontmatter and unterminated-fence
        // passthroughs above, which never reach this check at all.
        let body = "---\ntitle: T\nauthor: A\n---\nbody";
        let out = inject_missing_keys(
            body,
            &["title".to_string(), "author".to_string()],
        );
        assert_eq!(out, body);
    }

    #[test]
    fn find_closing_braces_returns_none_when_unterminated() {
        // Covers `find_closing_braces` walking past the end without
        // seeing `}}` — happens for malformed templates.
        let mut out = std::collections::BTreeSet::new();
        extract_simple_vars("{{ never_closes", &mut out);
        // No vars extracted because the closing braces weren't found.
        assert!(out.is_empty());
    }

    #[test]
    fn collapse_multiline_quoted_scalar_collapses_two_line() {
        let input = "url: \"\nhttps://example.com/x\"\n";
        let out = collapse_multiline_quoted_scalars(input);
        assert!(out.contains("url: \"https://example.com/x\""));
        assert!(!out.contains("\nhttps"));
    }

    #[test]
    fn collapse_multiline_quoted_scalar_preserves_single_line() {
        let input = "title: \"On one line\"\nauthor: \"Jane\"\n";
        let out = collapse_multiline_quoted_scalars(input);
        assert_eq!(out, input);
    }

    #[test]
    fn collapse_multiline_quoted_scalar_collapses_three_line() {
        let input = "blurb: \"\nline one\nline two\"\n";
        let out = collapse_multiline_quoted_scalars(input);
        assert!(out.contains("blurb: \"line one line two\""));
    }

    #[test]
    fn collapse_handles_unterminated_quote_gracefully() {
        // Pathological case — never close. We should not panic; we
        // just emit what we have so the downstream extractor surfaces
        // a clean error.
        let input = "x: \"\nstill open\n";
        let out = collapse_multiline_quoted_scalars(input);
        assert!(out.contains("still open"));
    }

    #[test]
    fn user_real_world_twitter_url_multiline_collapses() {
        // Exact shape from
        // _posts/2026-04-11-quantum-thresholds-are-moving-again.md
        // — the file that exposed the noyalib brittleness.
        let input = "twitter_url: \"\nhttps://sebastienrousseau.com/2026-04-11-quantum-thresholds-are-moving-again\"\n";
        let out = collapse_multiline_quoted_scalars(input);
        assert!(out.contains("twitter_url: \"https://"));
        assert_eq!(out.lines().count(), 1);
    }

    // ---------------------------------------------------------------
    // Permalink derivation (spec A2/B1, plan §2 item 1.2, issue #586)
    // ---------------------------------------------------------------

    #[test]
    fn inject_permalink_adds_key_when_missing() {
        let out = inject_permalink_if_missing(
            "---\ntitle: T\n---\nbody",
            "https://example.com/t/",
        );
        assert!(out.contains("permalink: \"https://example.com/t/\""));
        assert!(out.contains("title: T"));
        assert!(out.contains("body"));
    }

    #[test]
    fn inject_permalink_preserves_author_permalink_verbatim() {
        let input = "---\npermalink: /custom/place/\ntitle: T\n---\nbody";
        assert_eq!(
            inject_permalink_if_missing(input, "https://example.com/t/"),
            input
        );
    }

    #[test]
    fn inject_permalink_treats_url_key_as_author_specified() {
        let input = "---\nurl: https://elsewhere.example/\ntitle: T\n---\nb";
        assert_eq!(
            inject_permalink_if_missing(input, "https://example.com/t/"),
            input
        );
    }

    #[test]
    fn inject_permalink_no_frontmatter_passthrough() {
        let input = "# Heading\n\nBody.";
        assert_eq!(
            inject_permalink_if_missing(input, "https://example.com/"),
            input
        );
    }

    #[test]
    fn inject_permalink_unterminated_fence_passthrough() {
        let input = "---\ntitle: T\n# never closes";
        assert_eq!(
            inject_permalink_if_missing(input, "https://example.com/"),
            input
        );
    }

    #[test]
    fn inject_permalink_preserves_bom() {
        let input = "\u{FEFF}---\ntitle: T\n---\nbody";
        let out = inject_permalink_if_missing(input, "https://example.com/t/");
        assert!(out.starts_with('\u{FEFF}'));
        assert!(out.contains("permalink: \"https://example.com/t/\""));
    }

    #[test]
    fn inject_permalink_is_idempotent() {
        let input = "---\ntitle: T\n---\nbody";
        let once = inject_permalink_if_missing(input, "https://example.com/t/");
        let twice =
            inject_permalink_if_missing(&once, "https://example.com/t/");
        assert_eq!(once, twice);
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_with_site_defaults_derives_permalinks_for_all_pages() {
        // Plan §2 1.2 acceptance shape: a 3-page fixture where ZERO
        // pages carry `permalink:` must stage with derived permalinks
        // matching `{base_url}/{output_path}` under the compiler's
        // `foo.md → foo/index.html` pretty-URL convention.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(src.join("posts")).unwrap();
        fs::write(src.join("index.md"), "---\ntitle: Home\n---\nhome").unwrap();
        fs::write(src.join("about.md"), "---\ntitle: About\n---\nab").unwrap();
        fs::write(src.join("posts/first.md"), "---\ntitle: First\n---\npost")
            .unwrap();

        let staged = stage_content_with_site_defaults(
            &src,
            &build,
            &[],
            Some("https://example.com"),
            &[],
        )
        .unwrap();

        let home = fs::read_to_string(staged.join("index.md")).unwrap();
        assert!(
            home.contains("permalink: \"https://example.com/\""),
            "index.md must map to the site root URL: {home}"
        );
        let about = fs::read_to_string(staged.join("about.md")).unwrap();
        assert!(
            about.contains("permalink: \"https://example.com/about/\""),
            "about.md must map to a pretty directory URL: {about}"
        );
        let post = fs::read_to_string(staged.join("posts/first.md")).unwrap();
        assert!(
            post.contains("permalink: \"https://example.com/posts/first/\""),
            "nested page must include its directory path: {post}"
        );
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_with_site_defaults_keeps_author_permalink_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("custom.md"),
            "---\nlayout: post\npermalink: \"https://example.com/my-spot/\"\ntitle: C\n---\nbody",
        )
        .unwrap();

        let staged = stage_content_with_site_defaults(
            &src,
            &build,
            &[],
            Some("https://example.com"),
            &[],
        )
        .unwrap();
        let body = fs::read_to_string(staged.join("custom.md")).unwrap();
        assert!(body.contains("permalink: \"https://example.com/my-spot/\""));
        // Exactly one permalink key — no derived duplicate.
        assert_eq!(body.matches("permalink:").count(), 1);
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_with_site_defaults_handles_nested_index_md() {
        // `about/index.md` publishes at `about/index.html` →
        // permalink `{base}/about/` (index.html collapses to the
        // directory URL, matching the Atom feed convention).
        //
        // The staged file is named `about.md`, not `about/index.md` —
        // see `flatten_nested_index_pages`. The permalink is derived
        // from the AUTHORED path, so the value is unaffected.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(src.join("about")).unwrap();
        fs::write(src.join("about/index.md"), "---\ntitle: About\n---\nbody")
            .unwrap();

        let staged = stage_content_with_site_defaults(
            &src,
            &build,
            &[],
            Some("https://example.com/"),
            &[],
        )
        .unwrap();
        let body = fs::read_to_string(staged.join("about.md")).unwrap();
        assert!(
            body.contains("permalink: \"https://example.com/about/\""),
            "trailing-slash base + nested index.md: {body}"
        );
    }

    // -----------------------------------------------------------------
    // Nested `index.md` flattening (staticdatagen output-path gap)
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_flattens_nested_index_md_to_parent_named_file() {
        // `crate::urls::derive_output_rel_path` documents (and asserts)
        // `about/index.md → about/index.html`, but
        // `staticdatagen::utilities::write::write_files_to_build_directory`
        // only special-cases the exact processed name `"index"`, so a
        // nested `fr/index.md` lands at `fr/index/index.html` and every
        // locale home page gains a directory level.
        //
        // Staging the file as `fr.md` restores the documented mapping:
        // the compiler writes `<build>/fr/index.html` for it.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(src.join("fr")).unwrap();
        fs::create_dir_all(src.join("fr/blog")).unwrap();
        fs::write(src.join("index.md"), "---\ntitle: Home\n---\nen").unwrap();
        fs::write(src.join("fr/index.md"), "---\ntitle: Accueil\n---\nfr")
            .unwrap();
        fs::write(src.join("fr/blog/index.md"), "---\ntitle: Blog\n---\nb")
            .unwrap();
        fs::write(src.join("fr/a-propos.md"), "---\ntitle: A\n---\nap")
            .unwrap();

        let staged =
            stage_content_with_template_defaults(&src, &build, &[]).unwrap();

        assert!(
            staged.join("index.md").exists(),
            "root index.md is already correct and must stay put"
        );
        assert!(
            staged.join("fr.md").exists(),
            "fr/index.md must stage as fr.md so it compiles to fr/index.html"
        );
        assert!(
            !staged.join("fr/index.md").exists(),
            "the nested original must not also be staged"
        );
        assert!(
            staged.join("fr/blog.md").exists(),
            "deeper nesting flattens one level too"
        );
        assert!(
            !staged.join("fr/blog/index.md").exists(),
            "the nested original must not also be staged"
        );
        assert!(
            staged.join("fr/a-propos.md").exists(),
            "non-index siblings are untouched"
        );
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_keeps_nested_index_md_when_parent_named_file_exists() {
        // `fr.md` and `fr/index.md` both authored: flattening would
        // silently drop one page, so the nested file keeps its path
        // and the pre-existing (wrong but non-destructive) layout.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(src.join("fr")).unwrap();
        fs::write(src.join("fr.md"), "---\ntitle: FR\n---\nsection").unwrap();
        fs::write(src.join("fr/index.md"), "---\ntitle: Accueil\n---\nfr")
            .unwrap();

        let staged =
            stage_content_with_template_defaults(&src, &build, &[]).unwrap();

        assert!(staged.join("fr.md").exists());
        assert!(
            staged.join("fr/index.md").exists(),
            "collision must not clobber the authored fr.md"
        );
        assert!(
            fs::read_to_string(staged.join("fr.md"))
                .unwrap()
                .contains("title: FR"),
            "the authored fr.md must survive verbatim"
        );
    }

    #[test]
    fn flatten_nested_index_pages_keeps_dotted_directory_names_intact() {
        // `set_extension` on `v1.2` would truncate it to `v1.md`;
        // the implementation appends the extension textually instead.
        let dst = Path::new("/staged");
        let mut files = vec![(
            PathBuf::from("/src/v1.2/index.md"),
            PathBuf::from("/staged/v1.2/index.md"),
            true,
        )];
        flatten_nested_index_pages(dst, &mut files);
        assert_eq!(files[0].1, PathBuf::from("/staged/v1.2.md"));
    }

    #[test]
    fn flatten_nested_index_pages_ignores_non_markdown_and_root_files() {
        let dst = Path::new("/staged");
        let mut files = vec![
            // Non-markdown `index.html` asset — not a compiled page.
            (
                PathBuf::from("/src/fr/index.html"),
                PathBuf::from("/staged/fr/index.html"),
                false,
            ),
            // Root index.md — already compiles to the site root.
            (
                PathBuf::from("/src/index.md"),
                PathBuf::from("/staged/index.md"),
                true,
            ),
            // Nested non-index page — untouched.
            (
                PathBuf::from("/src/fr/about.md"),
                PathBuf::from("/staged/fr/about.md"),
                true,
            ),
        ];
        let before = files.clone();
        flatten_nested_index_pages(dst, &mut files);
        assert_eq!(files, before);
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_without_base_url_injects_no_permalink() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.md"), "---\ntitle: A\n---\nbody").unwrap();

        // Legacy entry point — must stay permalink-free.
        let staged =
            stage_content_with_template_defaults(&src, &build, &[]).unwrap();
        let body = fs::read_to_string(staged.join("a.md")).unwrap();
        assert!(!body.contains("permalink:"));

        // Empty / whitespace-only base URL disables injection too —
        // rss-gen only accepts absolute URLs, so there is nothing
        // valid to derive.
        let staged = stage_content_with_site_defaults(
            &src,
            &build,
            &[],
            Some("   "),
            &[],
        )
        .unwrap();
        let body = fs::read_to_string(staged.join("a.md")).unwrap();
        assert!(!body.contains("permalink:"));
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_with_site_defaults_is_idempotent_across_runs() {
        // v0.0.46 retired the layout-injection shim (staticdatagen
        // 0.0.10 defaults missing `layout:` natively), so the staged
        // output must carry exactly one derived permalink and no
        // injected layout key — across repeated staging runs.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.md"), "---\ntitle: A\n---\nbody").unwrap();

        let _first = stage_content_with_site_defaults(
            &src,
            &build,
            &[],
            Some("https://example.com"),
            &[],
        )
        .unwrap();
        let staged = stage_content_with_site_defaults(
            &src,
            &build,
            &[],
            Some("https://example.com"),
            &[],
        )
        .unwrap();
        let body = fs::read_to_string(staged.join("a.md")).unwrap();
        assert_eq!(body.matches("permalink:").count(), 1);
        assert_eq!(body.matches("layout:").count(), 0);
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_content_with_template_defaults_injects_defaults_end_to_end() {
        // Every other test that reaches `stage_content_with_site_defaults`
        // / `stage_content_with_template_defaults` passes an EMPTY
        // `template_var_keys` slice, so the `if
        // !template_var_keys.is_empty() { inject_template_defaults_recursive(...) }`
        // branch (and everything it calls) is only ever unit-tested via
        // a direct call to `inject_template_defaults_recursive`, never
        // through the public staging entry point. Drive it end to end.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.md"), "---\ntitle: A\n---\nbody").unwrap();

        let staged = stage_content_with_template_defaults(
            &src,
            &build,
            &["author".to_string()],
        )
        .unwrap();

        let body = fs::read_to_string(staged.join("a.md")).unwrap();
        assert!(
            body.contains("author: \"\""),
            "missing template var must be injected: {body}"
        );
    }

    // ── derived path globals ─────────────────────────────────────────

    fn locales() -> Vec<String> {
        vec!["en".to_string(), "fr".to_string()]
    }

    fn derived_for(rel: &str, base: Option<&str>) -> Vec<(String, String)> {
        derive_path_globals(base, Path::new(rel), &locales())
    }

    fn value_of(pairs: &[(String, String)], key: &str) -> String {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// The distinction the old `base_path` / `asset_path` pair failed to
    /// carry: assets live at the site root regardless of locale, pages do
    /// not. A French page asking for `/atlas/fr/styles.css` gets a 404,
    /// because that file is only ever written once, at the site root.
    #[test]
    fn derived_paths_separate_site_scope_from_locale_scope() {
        let d =
            derived_for("fr/a-propos.md", Some("https://example.com/atlas"));

        assert_eq!(value_of(&d, "site_path"), "/atlas/");
        assert_eq!(value_of(&d, "site_url"), "https://example.com/atlas/");
        assert_eq!(value_of(&d, "locale_path"), "/atlas/fr/");
        assert_eq!(value_of(&d, "locale_url"), "https://example.com/atlas/fr/");
    }

    /// A locale home page is staged as `fr.md` by the nested-index
    /// flattening, and still belongs to `fr`.
    #[test]
    fn derived_paths_recognise_a_flattened_locale_home_page() {
        let d = derived_for("fr.md", Some("https://example.com/atlas"));
        assert_eq!(value_of(&d, "locale_path"), "/atlas/fr/");
    }

    /// Default-locale pages live at the site root, so the two scopes
    /// coincide — a theme can use the locale forms throughout.
    #[test]
    fn derived_paths_collapse_for_the_root_hosted_default_locale() {
        let d = derived_for("about.md", Some("https://example.com/atlas"));
        assert_eq!(value_of(&d, "locale_path"), value_of(&d, "site_path"));
        assert_eq!(value_of(&d, "locale_url"), value_of(&d, "site_url"));
    }

    /// A single-locale site never sees a locale segment, even if a
    /// directory happens to share a locale's name.
    #[test]
    fn derived_paths_ignore_locales_when_only_one_is_configured() {
        let d = derive_path_globals(
            Some("https://example.com"),
            Path::new("fr/a-propos.md"),
            &["en".to_string()],
        );
        assert_eq!(value_of(&d, "locale_path"), "/");
    }

    /// A site at the domain root, and a build with no `base_url` at all,
    /// both yield usable root-relative values rather than `//`.
    #[test]
    fn derived_paths_handle_the_domain_root_and_a_missing_base_url() {
        let root = derived_for("about.md", Some("https://example.com"));
        assert_eq!(value_of(&root, "site_path"), "/");
        assert_eq!(value_of(&root, "site_url"), "https://example.com/");

        let none = derived_for("about.md", None);
        assert_eq!(value_of(&none, "site_path"), "/");
        assert_eq!(value_of(&none, "site_url"), "/");
    }

    /// Trailing slashes are guaranteed so templates concatenate directly.
    #[test]
    fn derived_paths_always_end_in_a_slash() {
        for base in [
            Some("https://example.com/atlas/"),
            Some("https://example.com/atlas"),
        ] {
            for (key, value) in derived_for("fr/x.md", base) {
                assert!(value.ends_with('/'), "{key} = {value:?}");
            }
        }
    }

    /// Author front matter wins: a page that declares its own value keeps
    /// it, so a locale served from another origin stays overridable.
    #[test]
    fn author_front_matter_overrides_a_derived_value() {
        let body = "---\nlocale_path: \"/custom/\"\n---\nbody\n";
        let out = inject_missing_keys_with_values(
            body,
            &["locale_path".to_string()],
            &[("locale_path".to_string(), "/atlas/fr/".to_string())],
        );
        assert!(out.contains("/custom/"), "{out}");
        assert!(
            !out.contains("/atlas/fr/"),
            "derived value overrode the author: {out}"
        );
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn inject_template_defaults_recursive_skips_write_when_no_keys_missing() {
        // Covers the `if staged == body { return None; }` skip-write
        // arm inside the parallel closure — every other
        // `inject_template_defaults_recursive` test supplies a key
        // that's actually missing, so the write always happens there.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("staged");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.md");
        let original = "---\ntitle: T\nauthor: A\n---\nbody";
        fs::write(&path, original).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();

        inject_template_defaults_recursive(
            &dir,
            &["title".to_string(), "author".to_string()],
            None,
            &[],
        )
        .unwrap();

        let after_body = fs::read_to_string(&path).unwrap();
        assert_eq!(after_body, original, "no-op write must not alter content");
        let after = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "file must not be rewritten when unchanged");
    }

    // -----------------------------------------------------------------
    // recreate_staging_dir — happy path and both failure arms
    // -----------------------------------------------------------------

    #[test]
    fn recreate_staging_dir_wipes_previous_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("stale.md"), "old").unwrap();

        recreate_staging_dir(&staging).unwrap();

        assert!(staging.is_dir());
        assert!(!staging.join("stale.md").exists());
    }

    #[test]
    fn recreate_staging_dir_fails_when_path_is_a_file() {
        // `remove_dir_all` on a regular file fails — the first `?`.
        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("staging");
        fs::write(&blocked, "not a dir").unwrap();

        assert!(recreate_staging_dir(&blocked).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn recreate_staging_dir_fails_when_parent_is_read_only() {
        // `create_dir_all` under a read-only parent fails — the
        // second `?`.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("ro");
        fs::create_dir_all(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o555))
            .unwrap();

        let res = recreate_staging_dir(&parent.join("staging"));

        let _ = fs::set_permissions(&parent, fs::Permissions::from_mode(0o755));
        // Root bypasses permissions on some CI runners, so tolerate Ok.
        assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn stage_fails_when_staging_root_is_blocked_by_a_file() {
        // Drives the `recreate_staging_dir(..)?` edge inside
        // `stage_content_with_site_defaults`.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(&src).unwrap();

        let staging = staging_root_for("content", &build);
        fs::write(&staging, "blocker").unwrap();

        let res =
            stage_content_with_site_defaults(&src, &build, &[], None, &[]);
        let _ = fs::remove_file(&staging);
        assert!(res.is_err());
    }

    // -----------------------------------------------------------------
    // copy_tree / collect_entries — error and skip arms
    // -----------------------------------------------------------------

    #[test]
    fn copy_tree_fails_when_destination_subdir_is_blocked() {
        // A file squatting on a destination directory name makes the
        // serial `create_dir_all` fail.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/a.md"), "---\nt: a\n---\nx").unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("sub"), "file, not dir").unwrap();

        assert!(copy_tree(&src, &dst, None).is_err());
    }

    #[test]
    fn copy_tree_copies_non_markdown_files_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("style.css"), "body{}").unwrap();

        copy_tree(&src, &dst, None).unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("style.css")).unwrap(),
            "body{}"
        );
    }

    #[test]
    fn copy_tree_reports_first_per_file_error() {
        // A non-UTF-8 .md file fails `read_to_string` inside the
        // parallel pass; the first collected error is returned.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("bad.md"), [0xFF, 0xFE, 0x00]).unwrap();

        assert!(copy_tree(&src, &dst, None).is_err());
    }

    #[test]
    fn copy_tree_reports_error_copying_non_markdown_file() {
        // The `fs::copy(&src_path, &dst_path).map(|_| ())` arm for
        // non-markdown files is only exercised on the success path
        // elsewhere (`copy_tree_copies_non_markdown_files_verbatim`).
        // Make the destination path itself an existing directory so
        // `fs::copy` fails for the non-md file specifically (distinct
        // from `copy_tree_fails_when_destination_subdir_is_blocked`,
        // which fails earlier at the serial `create_dir_all` step).
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("logo.png"), b"not really a png").unwrap();
        // Destination already occupied by a directory named like the file.
        fs::create_dir_all(dst.join("logo.png")).unwrap();

        assert!(copy_tree(&src, &dst, None).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn copy_tree_propagates_unreadable_source_subdir() {
        // The recursive collect_entries call fails when a nested
        // source directory can't be listed.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        let locked = src.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let res = copy_tree(&src, &dst, None);

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        // Root bypasses permissions on some CI runners, so tolerate Ok.
        assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
    }

    #[test]
    #[cfg(unix)]
    fn collect_entries_skips_symlinks_and_special_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("real.md"), "---\nt: a\n---\nx").unwrap();
        std::os::unix::fs::symlink(src.join("nowhere.md"), src.join("link.md"))
            .unwrap();

        copy_tree(&src, &dst, None).unwrap();
        assert!(dst.join("real.md").exists());
        assert!(!dst.join("link.md").exists(), "symlinks must be skipped");
    }

    // -----------------------------------------------------------------
    // collect_template_vars / extract_simple_vars — recursion, errors,
    // and every reject shape
    // -----------------------------------------------------------------

    #[test]
    fn collect_template_vars_recurses_into_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path().join("templates");
        fs::create_dir_all(t.join("partials")).unwrap();
        fs::write(t.join("page.html"), "{{ title }}").unwrap();
        fs::write(t.join("partials/nav.html"), "{{ nav_label }}").unwrap();

        let vars = collect_template_vars(&t).unwrap();
        assert!(vars.contains(&"title".to_string()));
        assert!(vars.contains(&"nav_label".to_string()));
    }

    #[test]
    #[cfg(unix)]
    fn collect_template_vars_propagates_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path().join("templates");
        let sub = t.join("locked");
        fs::create_dir_all(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();

        let res = collect_template_vars(&t);

        let _ = fs::set_permissions(&sub, fs::Permissions::from_mode(0o755));
        // Root bypasses permissions on some CI runners, so tolerate Ok.
        assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
    }

    #[test]
    fn extract_simple_vars_rejects_every_non_simple_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path().join("templates");
        fs::create_dir_all(&t).unwrap();
        fs::write(
            t.join("page.html"),
            // empty ref, helper, closing tag, raw-emit, partial,
            // filtered, dotted, spaced, unclosed — none are simple.
            "{{  }}{{#each xs}}{{/each}}{{!raw}}{{>part}}\
             {{ a | upper }}{{ a.b }}{{ a b }}{{ good }}{{ broken",
        )
        .unwrap();

        let vars = collect_template_vars(&t).unwrap();
        assert_eq!(vars, vec!["good".to_string()]);
    }

    #[test]
    fn walk_collect_vars_skips_unreadable_and_non_template_files() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path().join("templates");
        fs::create_dir_all(&t).unwrap();
        // Non-UTF-8 template file: read_to_string fails, silently
        // skipped.
        fs::write(t.join("binary.html"), [0xFF, 0xFE, 0x00]).unwrap();
        // Non-templating extension: never scanned.
        fs::write(t.join("style.css"), "{{ not_a_var }}").unwrap();
        fs::write(t.join("page.html"), "{{ real_var }}").unwrap();

        let vars = collect_template_vars(&t).unwrap();
        assert_eq!(vars, vec!["real_var".to_string()]);
    }

    // -----------------------------------------------------------------
    // inject_template_defaults_recursive — direct error/skip arms
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn inject_defaults_recurses_and_injects_in_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("staged");
        fs::create_dir_all(dir.join("blog")).unwrap();
        fs::write(dir.join("blog/a.md"), "---\ntitle: A\n---\nx").unwrap();

        inject_template_defaults_recursive(
            &dir,
            &["author".to_string()],
            None,
            &[],
        )
        .unwrap();

        let body = fs::read_to_string(dir.join("blog/a.md")).unwrap();
        assert!(body.contains("author:"));
    }

    #[test]
    #[serial_test::parallel(stager_fp)]
    fn inject_defaults_reports_unreadable_markdown() {
        // Non-UTF-8 bytes fail the read inside the parallel pass.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("staged");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.md"), [0xFF, 0xFE]).unwrap();

        let res = inject_template_defaults_recursive(
            &dir,
            &["k".to_string()],
            None,
            &[],
        );
        assert!(res.is_err());
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::parallel(stager_fp)]
    fn inject_defaults_propagates_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("staged");
        let sub = dir.join("locked");
        fs::create_dir_all(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();

        let res = inject_template_defaults_recursive(
            &dir,
            &["k".to_string()],
            None,
            &[],
        );

        let _ = fs::set_permissions(&sub, fs::Permissions::from_mode(0o755));
        assert!(res.err().is_none_or(|e| !format!("{e}").is_empty()));
    }

    #[test]
    #[cfg(unix)]
    fn collect_markdown_files_skips_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("staged");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("real.md"), "---\nt: a\n---\nx").unwrap();
        std::os::unix::fs::symlink(dir.join("nowhere.md"), dir.join("link.md"))
            .unwrap();

        let mut found = Vec::new();
        collect_markdown_files(&dir, &mut found).unwrap();
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn collect_markdown_files_skips_non_markdown_regular_files() {
        // The `else if ft.is_file() && is_markdown(&p)` arm where
        // `is_file()` is true but `is_markdown()` is false is never
        // exercised elsewhere — every fixture used with
        // `collect_markdown_files` / `inject_template_defaults_recursive`
        // only ever contains `.md` files.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("staged");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("real.md"), "---\nt: a\n---\nx").unwrap();
        fs::write(dir.join("notes.txt"), "not markdown").unwrap();
        fs::write(dir.join("style.css"), "body{}").unwrap();

        let mut found = Vec::new();
        collect_markdown_files(&dir, &mut found).unwrap();
        assert_eq!(found, vec![dir.join("real.md")]);
    }

    // -----------------------------------------------------------------
    // is_markdown / find_opening_fence — remaining shapes
    // -----------------------------------------------------------------

    #[test]
    fn is_markdown_accepts_both_extensions() {
        assert!(is_markdown(Path::new("a.md")));
        assert!(is_markdown(Path::new("a.markdown")));
        assert!(!is_markdown(Path::new("a.html")));
    }

    #[test]
    fn find_opening_fence_skips_leading_blank_lines() {
        let (lead, after) =
            find_opening_fence("\n  \n---\ntitle: x\n---\nbody").unwrap();
        assert_eq!(lead, "\n  \n");
        assert!(after.starts_with("title: x"));
    }

    #[test]
    fn find_opening_fence_returns_none_for_blank_only_input() {
        assert!(find_opening_fence("\n\n  \n").is_none());
        assert!(find_opening_fence("").is_none());
    }

    // -----------------------------------------------------------------
    // Fault injection — inject_template_defaults_recursive failpoint
    // -----------------------------------------------------------------

    #[cfg(feature = "test-fault-injection")]
    #[test]
    #[serial_test::serial(stager_fp)]
    fn stage_fault_inject_defaults_returns_err() {
        // RAII guard so a panicking assertion still deactivates the
        // failpoint (mirrors tests/fault_injection.rs).
        struct FailGuard(&'static str);
        impl Drop for FailGuard {
            fn drop(&mut self) {
                let _ = fail::cfg(self.0, "off");
            }
        }
        let _guard = FailGuard("content_stager::inject-defaults");
        fail::cfg("content_stager::inject-defaults", "return")
            .expect("activate failpoint");

        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        let build = tmp.path().join("build");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.md"), "---\ntitle: A\n---\nbody").unwrap();

        let err = stage_content_with_site_defaults(
            &src,
            &build,
            &["title".to_string()],
            None,
            &[],
        )
        .expect_err("failpoint must abort the staging pass");
        assert!(format!("{err}").contains("inject-defaults"));
    }
}
