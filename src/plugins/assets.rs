// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Asset optimization: fingerprinting, SRI hashes, and basic minification.
//!
//! Provides cache-busting via content-hash filenames and Subresource
//! Integrity attributes for CSS and JS files.

use crate::cmd::SriAlgorithm;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// Plugin that fingerprints CSS/JS assets and rewrites HTML references.
///
/// Runs in `after_compile`:
/// 1. Hash each `.css` and `.js` file (SHA-256, first 8 hex chars)
/// 2. Rename: `style.css` → `style.a1b2c3d4.css`
/// 3. Rewrite all HTML `<link>` and `<script>` references
/// 4. Add `integrity` and `crossorigin` attributes (SRI)
///
/// The SRI digest algorithm defaults to SHA-384 and is configurable
/// via `[security] sri_algorithm` in `ssg.toml` (v0.0.47 plan §3
/// item 2.3). The cache-busting filename fingerprint stays SHA-256
/// regardless — it is not a security control.
#[derive(Debug, Clone, Copy)]
pub struct FingerprintPlugin;

impl Plugin for FingerprintPlugin {
    fn name(&self) -> &'static str {
        "fingerprint"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        let all_assets = collect_assets(&ctx.site_dir)?;
        if all_assets.is_empty() {
            return Ok(());
        }

        // Three-pass fingerprinting (resolves the CSS-url() problem
        // surfaced in the v0.0.39 audit):
        //
        //   1. Hash + rename non-CSS assets first (images, fonts,
        //      JS). Build the first-stage manifest.
        //   2. Walk every CSS file. Patch any `url(...)` references
        //      that resolve to an entry in the first-stage manifest
        //      so they point at the new fingerprinted name. THEN
        //      hash + rename the CSS — its SRI hash now covers the
        //      post-rewrite content.
        //   3. Walk every HTML file and rewrite `<link href>`,
        //      `<script src>`, `<img src>`, etc. against the full
        //      manifest, attaching `integrity` + `crossorigin` for
        //      CSS/JS where SRI is meaningful.
        //
        // Without this split, CSS `url(/images/logo.png)` would
        // 404 after `logo.png` was renamed to `logo.<hash>.png`.

        let (css_files, non_css): (Vec<_>, Vec<_>) = all_assets
            .into_iter()
            .partition(|p| p.extension().is_some_and(|e| e == "css"));

        // `[security] sri_algorithm` from ssg.toml; absent config ⇒
        // SHA-384 (v0.0.47 plan §3 item 2.3).
        let sri_algorithm = ctx
            .config
            .as_ref()
            .map_or_else(SriAlgorithm::default, |c| c.security.sri_algorithm);

        let mut manifest =
            fingerprint_assets(&non_css, &ctx.site_dir, sri_algorithm)?;

        for css_path in &css_files {
            rewrite_css_urls_inplace(css_path, &ctx.site_dir, &manifest)?;
        }

        let css_manifest =
            fingerprint_assets(&css_files, &ctx.site_dir, sri_algorithm)?;
        manifest.extend(css_manifest);

        rewrite_html_references(&ctx.site_dir, &manifest)?;

        log::info!(
            "[fingerprint] Processed {} asset(s) across {} CSS + {} other",
            manifest.len(),
            css_files.len(),
            manifest.len() - css_files.len()
        );
        Ok(())
    }
}

/// Fingerprints all asset files: computes hash, renames, and builds the manifest.
fn fingerprint_assets(
    assets: &[PathBuf],
    site_dir: &Path,
    sri_algorithm: SriAlgorithm,
) -> Result<HashMap<String, AssetInfo>, SsgError> {
    let mut manifest = HashMap::new();

    for asset_path in assets {
        let info = fingerprint_file(asset_path, site_dir, sri_algorithm)?;
        let _ = manifest.insert(info.0, info.1);
    }

    Ok(manifest)
}

/// Fingerprints a single asset file: hash, rename, return (`old_rel`, `AssetInfo`).
fn fingerprint_file(
    asset_path: &Path,
    site_dir: &Path,
    sri_algorithm: SriAlgorithm,
) -> Result<(String, AssetInfo), SsgError> {
    let mut content = fs::read(asset_path).with_path(asset_path)?;
    let ext = asset_path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut minified = false;

    if ext == "css" {
        if let Ok(css_str) = std::str::from_utf8(&content) {
            content = minify_css(css_str).into_bytes();
            minified = true;
        }
    } else if ext == "js" || ext == "mjs" {
        if let Ok(js_str) = std::str::from_utf8(&content) {
            content = minify_js(js_str).into_bytes();
            minified = true;
        }
    }

    let hash = sha256_hex(&content);
    let short_hash = &hash[..8];

    let stem = asset_path.file_stem().unwrap_or_default().to_string_lossy();
    let new_name = format!("{stem}.{short_hash}.{ext}");
    let new_path = asset_path.with_file_name(&new_name);

    let sri = sri_algorithm.integrity(&content);

    if minified {
        fs::write(&new_path, &content).with_path(&new_path)?;
    } else {
        let _ = fs::copy(asset_path, &new_path).with_path(asset_path)?;
    }

    // Fingerprinting is a rename, not a copy. Leaving the source in place
    // shipped every stylesheet and script twice — once fingerprinted and
    // minified, once not — and the unfingerprinted copy stayed reachable at
    // a stable URL, so anything still pointing at it received content that
    // no `integrity` attribute covered.
    //
    // Guarded against the degenerate case where the hash lands on the name
    // the file already has, which would otherwise delete the asset.
    if new_path != asset_path {
        // The failpoint precedes the removal so an injected error exercises
        // the same branch a real `remove_file` failure would: the
        // fingerprinted copy is already on disk, and the original must be
        // left in place rather than half-removed.
        fail_point!("assets::remove-original", |_| Err(SsgError::Validation {
            field: "assets".to_string(),
            message: "injected: assets::remove-original".to_string(),
        }));
        fs::remove_file(asset_path).with_path(asset_path)?;
    }

    let rel_old = asset_path
        .strip_prefix(site_dir)
        .unwrap_or(asset_path)
        .to_string_lossy()
        .replace('\\', "/");
    let rel_new = new_path
        .strip_prefix(site_dir)
        .unwrap_or(&new_path)
        .to_string_lossy()
        .replace('\\', "/");

    Ok((
        rel_old,
        AssetInfo {
            fingerprinted: rel_new,
            sri,
        },
    ))
}

/// Rewrites HTML files to use fingerprinted asset references.
fn rewrite_html_references(
    site_dir: &Path,
    manifest: &HashMap<String, AssetInfo>,
) -> Result<(), SsgError> {
    let html_files = collect_html_files(site_dir)?;
    for html_path in &html_files {
        let html = fs::read_to_string(html_path).with_path(html_path)?;
        let rewritten = rewrite_asset_refs(&html, manifest);
        if rewritten != html {
            fs::write(html_path, rewritten).with_path(html_path)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct AssetInfo {
    fingerprinted: String,
    sri: String,
}

/// Rewrites every `url(...)` reference in a CSS file in place,
/// pointing each one at the fingerprinted name from `manifest` if
/// the URL resolves to a known asset.
///
/// Resolution rules:
///
/// - `url(/foo.png)` — absolute from the site root; lookup key is
///   `foo.png`.
/// - `url(./foo.png)`, `url(../images/foo.png)` — resolved against
///   the CSS file's parent directory, then made site-relative.
/// - `url(images/foo.png)` (bare, no `/` or `./`) — same as the
///   relative case above.
/// - URLs containing `://` (full URLs) and `data:` URIs are left
///   untouched.
///
/// Output URLs are written as **absolute, site-rooted paths**
/// (`/foo.<hash>.png`) regardless of the original form. This is
/// valid CSS and unambiguous; it deliberately trades a tiny bit of
/// stylistic preservation for correctness.
///
/// Quote forms handled: `url(x)`, `url("x")`, `url('x')`. URLs with
/// query strings or fragments (e.g. `url(foo.svg#icon)`,
/// `url(foo.css?v=1)`) preserve the suffix on the rewritten URL.
fn rewrite_css_urls(
    css: &str,
    css_path: &Path,
    site_dir: &Path,
    manifest: &HashMap<String, AssetInfo>,
) -> String {
    let css_dir = css_path.parent().unwrap_or(css_path);
    let mut out = String::with_capacity(css.len());
    let mut remaining = css;

    while let Some(idx) = remaining.find("url(") {
        out.push_str(&remaining[..idx]);
        let after_open = &remaining[idx + 4..]; // past "url("
        let Some(close_idx) = after_open.find(')') else {
            // Unterminated url(...) — leave the rest unchanged.
            out.push_str("url(");
            out.push_str(after_open);
            return out;
        };
        let raw = &after_open[..close_idx];
        let rest = &after_open[close_idx + 1..];

        // Strip optional surrounding quotes.
        let trimmed = raw.trim();
        let (quote, inner) = if let Some(s) = trimmed.strip_prefix('"') {
            ('"', s.strip_suffix('"').unwrap_or(s))
        } else if let Some(s) = trimmed.strip_prefix('\'') {
            ('\'', s.strip_suffix('\'').unwrap_or(s))
        } else {
            ('\0', trimmed)
        };

        // Split off ?query or #fragment so we don't try to resolve them.
        let (url, suffix) = if let Some(i) = inner.find(['?', '#']) {
            (&inner[..i], &inner[i..])
        } else {
            (inner, "")
        };

        let resolved = resolve_css_url(url, css_dir, site_dir);
        let hit = resolved.and_then(|key| manifest.get(&key).map(|i| (key, i)));

        out.push_str("url(");
        if let Some((_, info)) = hit {
            // Emit absolute /<fingerprinted>(suffix).
            let new_url = format!("/{}{}", info.fingerprinted, suffix);
            if quote != '\0' {
                out.push(quote);
            }
            out.push_str(&new_url);
            if quote != '\0' {
                out.push(quote);
            }
        } else {
            // No manifest hit — emit the original verbatim.
            out.push_str(raw);
        }
        out.push(')');

        remaining = rest;
    }

    out.push_str(remaining);
    out
}

/// Resolves a CSS URL to a site-relative manifest key.
///
/// Returns `None` for full URLs (`http://`, `https://`, `//`),
/// `data:` URIs, and paths that escape the site directory.
fn resolve_css_url(
    url: &str,
    css_dir: &Path,
    site_dir: &Path,
) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("data:")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("//")
    {
        return None;
    }

    // Build the absolute on-disk path.
    let candidate = if let Some(stripped) = trimmed.strip_prefix('/') {
        site_dir.join(stripped)
    } else {
        css_dir.join(trimmed)
    };

    // Logical canonicalisation: collapse `..` and `.` without
    // touching the filesystem so non-existent (already-renamed)
    // targets still resolve.
    let mut components: Vec<&std::ffi::OsStr> = Vec::new();
    for c in candidate.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = components.pop();
            }
            std::path::Component::Normal(s) => components.push(s),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                components.clear();
            }
        }
    }
    let mut resolved = PathBuf::new();
    for c in components {
        resolved.push(c);
    }

    // The manifest stores keys relative to site_dir (no leading slash).
    let site_components: Vec<&std::ffi::OsStr> = site_dir
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    let resolved_components: Vec<&std::ffi::OsStr> = resolved
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();

    if resolved_components.len() < site_components.len()
        || resolved_components[..site_components.len()] != site_components[..]
    {
        return None;
    }

    let rel: PathBuf = resolved_components[site_components.len()..]
        .iter()
        .collect();
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Reads, rewrites, and writes a CSS file in place if any of its
/// `url(...)` references resolve to a manifest entry.
fn rewrite_css_urls_inplace(
    css_path: &Path,
    site_dir: &Path,
    manifest: &HashMap<String, AssetInfo>,
) -> Result<(), SsgError> {
    let css = fs::read_to_string(css_path).with_path(css_path)?;
    let rewritten = rewrite_css_urls(&css, css_path, site_dir, manifest);
    if rewritten != css {
        fs::write(css_path, rewritten).with_path(css_path)?;
    }
    Ok(())
}

/// Rewrites asset references in HTML and adds SRI attributes.
fn rewrite_asset_refs(
    html: &str,
    manifest: &HashMap<String, AssetInfo>,
) -> String {
    let mut result = html.to_string();
    for (old_path, info) in manifest {
        // Direct matches: "styles.css" and "/styles.css"
        let old_ref = format!("\"{old_path}\"");
        let old_ref_slash = format!("\"/{old_path}\"");
        let new_ref = format!(
            "\"{}\" integrity=\"{}\" crossorigin=\"anonymous\"",
            info.fingerprinted, info.sri
        );
        let new_ref_slash = format!(
            "\"/{}\" integrity=\"{}\" crossorigin=\"anonymous\"",
            info.fingerprinted, info.sri
        );

        result = result.replace(&old_ref, &new_ref);
        result = result.replace(&old_ref_slash, &new_ref_slash);

        // Scoped sub-path matches: e.g. "/swiftdev/styles.css" -> "/swiftdev/styles.hash.css"
        let old_suffix = format!("/{old_path}\"");
        let new_suffix = format!(
            "/{}\" integrity=\"{}\" crossorigin=\"anonymous\"",
            info.fingerprinted, info.sri
        );
        result = result.replace(&old_suffix, &new_suffix);
    }
    result
}

/// SHA-256 hash as a 64-char hex string.
///
/// Used only for the cache-busting fingerprint suffix
/// (`name.<hash>.ext`); the first 8 hex characters of this output are
/// taken as the short content fingerprint. The `integrity` attribute
/// is computed separately via [`SriAlgorithm::integrity`] (SHA-384 by
/// default — v0.0.47 plan §3 item 2.3), so the filename fingerprint
/// deliberately stays SHA-256: it is a cache key, not a security
/// control.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Minimal CSS minifier that removes comments and compresses whitespace.
fn minify_css(css: &str) -> String {
    let mut result = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    let mut in_comment = false;
    let mut in_string = None;

    while let Some(ch) = chars.next() {
        if in_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                let _ = chars.next();
                in_comment = false;
            }
            continue;
        }

        if let Some(quote) = in_string {
            result.push(ch);
            if ch == quote {
                let mut backslashes = 0;
                let mut temp = result.len() as isize - 2;
                while temp >= 0 && result.as_bytes()[temp as usize] == b'\\' {
                    backslashes += 1;
                    temp -= 1;
                }
                if backslashes % 2 == 0 {
                    in_string = None;
                }
            }
            continue;
        }

        if ch == '/' && chars.peek() == Some(&'*') {
            let _ = chars.next();
            in_comment = true;
            continue;
        }

        if ch == '\'' || ch == '"' {
            in_string = Some(ch);
            result.push(ch);
            continue;
        }

        if ch.is_whitespace() {
            result.push(' ');
            continue;
        }

        result.push(ch);
    }

    let mut clean = String::with_capacity(result.len());
    let chars: Vec<char> = result.chars().collect();
    let mut i = 0;
    // Quote state is tracked here too. The first pass copies string contents
    // verbatim, but this pass then walked the same buffer without knowing
    // where strings were, so `content: "  two  spaces  "` came out as
    // `content:"twospaces"` — the minifier rewriting authored text.
    let mut in_string: Option<char> = None;
    while i < chars.len() {
        let ch = chars[i];

        if let Some(q) = in_string {
            clean.push(ch);
            // A quote closes the string unless it is backslash-escaped.
            if ch == q {
                let mut back = 0usize;
                let mut k = i;
                while k > 0 && chars[k - 1] == '\\' {
                    back += 1;
                    k -= 1;
                }
                if back.is_multiple_of(2) {
                    in_string = None;
                }
            }
            i += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            in_string = Some(ch);
            clean.push(ch);
            i += 1;
            continue;
        }

        if ch == ' ' {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = if i + 1 < chars.len() {
                Some(chars[i + 1])
            } else {
                None
            };

            // `+` counts as a word character here for one reason: CSS math
            // functions require whitespace around `+` and `-`, and dropping it
            // does not merely lengthen the output, it invalidates the
            // declaration. `clamp(2.07rem, 1.75rem + 1.6vw, 3.13rem)` became
            // `clamp(2.07rem,1.75rem+1.6vw,3.13rem)`, which every browser
            // rejects — so every fluid type step in every theme silently fell
            // back and headings rendered at the body size.
            //
            // `*` is in both sets for the same class of reason, found the
            // hard way. It needs no space inside `calc()`, but in a
            // selector it is the universal selector and the space before
            // it is a descendant combinator: `#btn *` became `#btn*`,
            // which matches nothing. Three themes override the search
            // widget with `#ssg-search-btn * { color: … !important }`,
            // and all three overrides were silently discarded — the
            // widget kept the generator's own colour and failed AAA
            // contrast against the theme's surface. Keeping the space in
            // `calc(2 * 3)` costs two bytes and is valid.
            //
            // `-` was already in both sets and so was never affected;
            // `/` needs no surrounding space and stays collapsible.
            // Keeping the space in a `.a + .b` selector too costs two
            // bytes and is valid.
            // Media and support queries put a required space either side of
            // their combinators: `@media (a) and (b)` collapsed to
            // `@media(a)and(b)`, which is invalid and silently disables the
            // whole query. Only single-condition queries are in the themes
            // today, so nothing shipped broken — but the first compound query
            // anyone wrote would have.
            let joins_prelude_tokens = match (prev, next) {
                (Some(')'), Some(n)) => n.is_ascii_alphabetic(),
                (Some(p), Some('(')) => p.is_ascii_alphabetic(),
                _ => false,
            };

            let is_needed = joins_prelude_tokens
                || match (prev, next) {
                    (Some(p), Some(n)) => {
                        let is_p_word = p.is_alphanumeric()
                            || p == '-'
                            || p == '+'
                            || p == '_'
                            || p == '#'
                            || p == '.'
                            || p == '@'
                            || p == '%'
                            || p == '*'
                            || p == '$';
                        let is_n_word = n.is_alphanumeric()
                            || n == '-'
                            || n == '+'
                            || n == '_'
                            || n == '#'
                            || n == '.'
                            || n == '@'
                            || n == '%'
                            || n == '*'
                            || n == '$';
                        is_p_word && is_n_word
                    }
                    _ => false,
                };
            if is_needed {
                clean.push(' ');
            }
        } else {
            clean.push(ch);
        }
        i += 1;
    }

    clean.trim().to_string()
}

/// Minimal JS minifier that removes comments and compresses whitespace/newlines safely.
fn minify_js(js: &str) -> String {
    let mut result = String::with_capacity(js.len());
    let mut chars = js.chars().peekable();
    let mut in_multi_comment = false;
    let mut in_single_comment = false;
    let mut in_string = None;

    while let Some(ch) = chars.next() {
        if in_multi_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                let _ = chars.next();
                in_multi_comment = false;
            }
            continue;
        }

        if in_single_comment {
            if ch == '\n' || ch == '\r' {
                in_single_comment = false;
                result.push('\n');
            }
            continue;
        }

        if let Some(quote) = in_string {
            result.push(ch);
            if ch == quote {
                let mut backslashes = 0;
                let mut temp = result.len() as isize - 2;
                while temp >= 0 && result.as_bytes()[temp as usize] == b'\\' {
                    backslashes += 1;
                    temp -= 1;
                }
                if backslashes % 2 == 0 {
                    in_string = None;
                }
            }
            continue;
        }

        if ch == '/' {
            if chars.peek() == Some(&'*') {
                let _ = chars.next();
                in_multi_comment = true;
                continue;
            } else if chars.peek() == Some(&'/') {
                let _ = chars.next();
                in_single_comment = true;
                continue;
            }
        }

        if ch == '\'' || ch == '"' || ch == '`' {
            in_string = Some(ch);
            result.push(ch);
            continue;
        }

        if ch.is_whitespace() {
            if ch == '\n' || ch == '\r' {
                if !result.ends_with('\n') && !result.is_empty() {
                    result.push('\n');
                }
            } else if !result.ends_with(' ')
                && !result.ends_with('\n')
                && !result.is_empty()
            {
                result.push(' ');
            }
            continue;
        }

        result.push(ch);
    }

    let mut clean = String::with_capacity(result.len());
    let chars: Vec<char> = result.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == ' ' || ch == '\n' {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = if i + 1 < chars.len() {
                Some(chars[i + 1])
            } else {
                None
            };

            let is_needed = match (prev, next) {
                (Some(p), Some(n)) => {
                    let is_p_word = p.is_alphanumeric() || p == '_' || p == '$';
                    let is_n_word = n.is_alphanumeric() || n == '_' || n == '$';
                    is_p_word && is_n_word
                }
                _ => false,
            };
            if is_needed {
                clean.push(ch);
            }
        } else {
            clean.push(ch);
        }
        i += 1;
    }
    clean.trim().to_string()
}

/// Asset extensions we content-fingerprint. Matches the
/// "content-addressable asset pipeline" intent of issue #468:
/// CSS/JS for code, common raster + vector image formats for art,
/// font formats for typography. Each gets a `name.hash.ext` rename
/// and an SRI hash; deploy configs serve them with
/// `Cache-Control: public, max-age=31536000, immutable`.
const FINGERPRINTED_EXTENSIONS: &[&str] = &[
    "css", "js", "mjs", "png", "jpg", "jpeg", "webp", "avif", "gif", "svg",
    "woff", "woff2", "ttf", "otf",
];

/// Collects every fingerprintable asset from site dir.
/// Directories whose assets must keep their authored filenames.
///
/// `_islands/` holds the island loader and the component bundles it pulls in
/// with a *dynamic* `import()` built at runtime from the component name.
/// A static rewriter cannot see that construction, so fingerprinting the
/// bundles renamed the files without updating the only thing that resolves
/// them — every island 404'd on hydration. The loader tag in the HTML has
/// the same problem, since the islands plugin emits it after this pass.
///
/// These files are already effectively immutable per build; skipping them
/// costs a cache-busting opportunity and buys a working feature.
const UNFINGERPRINTED_DIRS: &[&str] = &["_islands"];

fn collect_assets(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    let all = crate::walk::walk_files_multi(dir, FINGERPRINTED_EXTENSIONS)?;
    Ok(all
        .into_iter()
        .filter(|path| {
            !path.components().any(|c| {
                UNFINGERPRINTED_DIRS
                    .iter()
                    .any(|d| c.as_os_str() == std::ffi::OsStr::new(d))
            })
        })
        .collect())
}

fn collect_html_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files(dir, "html")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_minify_css() {
        let input = "body {\n  color: red;\n  background-color: #ffffff; /* comment */\n}";
        let expected = "body{color:red;background-color:#ffffff;}";
        assert_eq!(minify_css(input), expected);
    }

    #[test]
    fn test_minify_js() {
        let input = "const x = 5; // comment\n/* multi\ncomment */\nconst y = 10;\nconsole.log(x + y);";
        let expected = "const x=5;const y=10;console.log(x+y);";
        assert_eq!(minify_js(input), expected);
    }

    #[test]
    fn test_sha256_hex_deterministic() {
        let h1 = sha256_hex(b"hello");
        let h2 = sha256_hex(b"hello");
        assert_eq!(h1, h2);
        // Real SHA-256 is 32 bytes → 64 hex chars.
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_sha256_hex_known_vectors() {
        // Verifies real SHA-256 is in use, not an FNV placeholder.
        // Empty input — well-known SHA-256("") digest.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // "abc" — the canonical NIST test vector.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_sri_default_algorithm_known_vector() {
        // SHA-384("") base64-encoded — the canonical empty-input
        // digest. The default SRI algorithm is SHA-384 (v0.0.47 plan
        // §3 item 2.3).
        assert_eq!(
            SriAlgorithm::default().integrity(b""),
            "sha384-OLBgp1GsljhM2TJ+sbHjaiH9txEUvgdDTAzHv2P24donTt6/529l+9Ua0vFImLlb"
        );
    }

    #[test]
    fn test_sri_sha256_override_known_vector() {
        // SHA-256("") base64-encoded is the canonical 47DEQpj8... value.
        assert_eq!(
            SriAlgorithm::Sha256.integrity(b""),
            "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
    }

    #[test]
    fn test_sha256_hex_varies() {
        let h1 = sha256_hex(b"hello");
        let h2 = sha256_hex(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    #[serial_test::parallel(assets_failpoint)]
    fn test_fingerprint_plugin() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        // Create a CSS file
        fs::write(site.join("style.css"), "body { color: red; }").unwrap();

        // Create HTML that references it
        let html = r#"<html><head><link rel="stylesheet" href="style.css"></head><body></body></html>"#;
        fs::write(site.join("index.html"), html).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        FingerprintPlugin.after_compile(&ctx).unwrap();

        // Original file should be gone
        assert!(!site.join("style.css").exists());

        // Fingerprinted file should exist
        let entries: Vec<_> = fs::read_dir(&site)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("style.")
                    && e.path().extension().is_some_and(|e| e == "css")
            })
            .collect();
        assert_eq!(entries.len(), 1);

        // HTML should reference the fingerprinted file with a SHA-384
        // integrity attribute (the default — v0.0.47 plan §3 item 2.3).
        let output = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(output.contains("integrity=\"sha384-"));
        assert!(output.contains("crossorigin=\"anonymous\""));
        assert!(!output.contains("href=\"style.css\""));
    }

    #[test]
    #[serial_test::parallel(assets_failpoint)]
    fn default_sri_is_sha384_with_exact_known_vector() {
        // End-to-end through the plugin with NO config: the JS body
        // survives minification byte-for-byte ("console.log(1);" has
        // no removable whitespace/comments), so the emitted integrity
        // attribute must be exactly base64(SHA-384("console.log(1);")).
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("app.js"), "console.log(1);").unwrap();
        fs::write(
            site.join("index.html"),
            r#"<html><head><script src="app.js"></script></head></html>"#,
        )
        .unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        FingerprintPlugin.after_compile(&ctx).unwrap();

        let html = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(
            html.contains(
                "integrity=\"sha384-JawyHuhqEMFMvdtX+VHylbI0hfJp2F7nvwFVRqqfuOoK5oW7TG/7V11Zs7zeFWIE\""
            ),
            "expected exact SHA-384 SRI vector; got: {html}"
        );
    }

    #[test]
    #[serial_test::parallel(assets_failpoint)]
    fn sri_algorithm_config_override_emits_sha256() {
        // `[security] sri_algorithm = "sha256"` back-compat knob
        // (v0.0.47 plan §3 item 2.3): the exact SHA-256 vector for
        // "console.log(1);" must be emitted instead of SHA-384.
        use crate::cmd::{SecurityConfig, SsgConfig};

        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("app.js"), "console.log(1);").unwrap();
        fs::write(
            site.join("index.html"),
            r#"<html><head><script src="app.js"></script></head></html>"#,
        )
        .unwrap();

        let config = SsgConfig::builder()
            .security(SecurityConfig {
                sri_algorithm: SriAlgorithm::Sha256,
            })
            .build()
            .unwrap();
        let ctx = PluginContext::with_config(
            dir.path(),
            dir.path(),
            &site,
            dir.path(),
            config,
        );
        FingerprintPlugin.after_compile(&ctx).unwrap();

        let html = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(
            html.contains(
                "integrity=\"sha256-NcFG924SlHfGQGG8hFEeEJDz1NgFlxPmZj3Us1sfdkI=\""
            ),
            "expected exact SHA-256 SRI vector; got: {html}"
        );
        assert!(!html.contains("sha384-"), "override must win: {html}");
    }

    #[test]
    fn name_returns_static_fingerprint_identifier() {
        assert_eq!(FingerprintPlugin.name(), "fingerprint");
    }

    #[test]
    fn after_compile_missing_site_dir_returns_ok() {
        // Line 34: `!ctx.site_dir.exists()` early return.
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let ctx =
            PluginContext::new(dir.path(), dir.path(), &missing, dir.path());
        FingerprintPlugin.after_compile(&ctx).unwrap();
        assert!(!missing.exists());
    }

    #[test]
    fn after_compile_no_assets_short_circuits() {
        // Line 40: `assets.is_empty()` early return — site with
        // HTML but no CSS/JS.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("index.html"), "<p></p>").unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        FingerprintPlugin.after_compile(&ctx).unwrap();
        // HTML untouched.
        assert_eq!(
            fs::read_to_string(site.join("index.html")).unwrap(),
            "<p></p>"
        );
    }

    #[test]
    #[serial_test::parallel(assets_failpoint)]
    fn after_compile_fingerprint_absolute_path_href() {
        // Covers the `old_ref_slash` variant (with leading /) in
        // rewrite_asset_refs — absolute-path stylesheet links.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("app.js"), "console.log(1);").unwrap();
        fs::write(
            site.join("index.html"),
            r#"<html><head><script src="/app.js"></script></head></html>"#,
        )
        .unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        FingerprintPlugin.after_compile(&ctx).unwrap();
        let html = fs::read_to_string(site.join("index.html")).unwrap();
        // Default algorithm is SHA-384 (v0.0.47 plan §3 item 2.3).
        assert!(html.contains("integrity=\"sha384-"));
    }

    #[test]
    fn collect_assets_picks_up_fingerprintable_extensions() {
        // Issue #468 widened the fingerprinted set from {css, js}
        // to also include images and fonts. css/js/png/woff2 are in;
        // html/txt/md are out.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.css"), "").unwrap();
        fs::write(dir.path().join("b.js"), "").unwrap();
        fs::write(dir.path().join("c.html"), "").unwrap();
        fs::write(dir.path().join("d.png"), "").unwrap();
        fs::write(dir.path().join("e.woff2"), "").unwrap();
        fs::write(dir.path().join("f.txt"), "").unwrap();
        let files = collect_assets(dir.path()).unwrap();
        // 4 fingerprintable: css, js, png, woff2.
        assert_eq!(files.len(), 4);
    }

    #[test]
    fn collect_assets_recurses_into_subdirectories() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("vendor");
        fs::create_dir(&nested).unwrap();
        fs::write(dir.path().join("top.css"), "").unwrap();
        fs::write(nested.join("lib.js"), "").unwrap();
        let files = collect_assets(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn collect_html_files_filters_non_html() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.html"), "").unwrap();
        fs::write(dir.path().join("b.css"), "").unwrap();
        let files = collect_html_files(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn sha256_hex_produces_64_hex_chars() {
        assert_eq!(sha256_hex(b"abc").len(), 64);
        assert_eq!(sha256_hex(b"").len(), 64);
    }

    #[test]
    fn sri_integrity_is_nonempty_for_input() {
        assert!(!SriAlgorithm::default().integrity(b"hello").is_empty());
    }

    #[test]
    fn sri_integrity_payload_lengths_per_algorithm() {
        // "sha384-" (7) + SHA-384 → 48 raw bytes → base64 = 64 chars.
        assert_eq!(SriAlgorithm::Sha384.integrity(b"hello").len(), 7 + 64);
        // "sha256-" (7) + SHA-256 → 32 raw bytes → base64 = 44 chars.
        assert_eq!(SriAlgorithm::Sha256.integrity(b"hello").len(), 7 + 44);
        // "sha512-" (7) + SHA-512 → 64 raw bytes → base64 = 88 chars.
        assert_eq!(SriAlgorithm::Sha512.integrity(b"hello").len(), 7 + 88);
    }

    #[test]
    fn test_rewrite_asset_refs() {
        let mut manifest = HashMap::new();
        let _ = manifest.insert(
            "style.css".to_string(),
            AssetInfo {
                fingerprinted: "style.abc12345.css".to_string(),
                sri: "sha384-xyz".to_string(),
            },
        );

        let html = r#"<link rel="stylesheet" href="style.css">"#;
        let result = rewrite_asset_refs(html, &manifest);
        assert!(result.contains("style.abc12345.css"));
        assert!(result.contains("integrity=\"sha384-xyz\""));
    }

    // ── CSS url() rewriting (resolves audit item #2) ───────────────

    fn css_manifest() -> HashMap<String, AssetInfo> {
        let mut m = HashMap::new();
        let _ = m.insert(
            "images/logo.png".to_string(),
            AssetInfo {
                fingerprinted: "images/logo.deadbeef.png".to_string(),
                sri: String::new(),
            },
        );
        let _ = m.insert(
            "fonts/sans.woff2".to_string(),
            AssetInfo {
                fingerprinted: "fonts/sans.cafef00d.woff2".to_string(),
                sri: String::new(),
            },
        );
        m
    }

    #[test]
    fn rewrite_css_urls_handles_absolute_path() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("assets/style.css");
        let css = "body { background: url(/images/logo.png); }";
        let out = rewrite_css_urls(css, &css_path, dir.path(), &css_manifest());
        assert!(out.contains("url(/images/logo.deadbeef.png)"));
        assert!(!out.contains("logo.png)"));
    }

    #[test]
    fn rewrite_css_urls_handles_relative_path() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("assets/style.css");
        let css = "body { background: url(../images/logo.png); }";
        let out = rewrite_css_urls(css, &css_path, dir.path(), &css_manifest());
        assert!(out.contains("url(/images/logo.deadbeef.png)"));
    }

    #[test]
    fn rewrite_css_urls_handles_double_quotes() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("style.css");
        let css = r#"body { background: url("/images/logo.png"); }"#;
        let out = rewrite_css_urls(css, &css_path, dir.path(), &css_manifest());
        assert!(out.contains(r#"url("/images/logo.deadbeef.png")"#));
    }

    #[test]
    fn rewrite_css_urls_handles_single_quotes() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("style.css");
        let css = "body { background: url('/images/logo.png'); }";
        let out = rewrite_css_urls(css, &css_path, dir.path(), &css_manifest());
        assert!(out.contains("url('/images/logo.deadbeef.png')"));
    }

    #[test]
    fn rewrite_css_urls_preserves_query_and_fragment() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("style.css");
        let css = "@font-face { src: url(/fonts/sans.woff2?v=1#hint); }";
        let out = rewrite_css_urls(css, &css_path, dir.path(), &css_manifest());
        assert!(out.contains("/fonts/sans.cafef00d.woff2?v=1#hint"));
    }

    #[test]
    fn rewrite_css_urls_skips_external_and_data_urls() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("style.css");
        let css = r#"
            a { background: url(https://cdn.example.com/x.png); }
            b { background: url(//cdn.example.com/y.png); }
            c { background: url(data:image/svg+xml,<svg/>); }
        "#;
        let out = rewrite_css_urls(css, &css_path, dir.path(), &css_manifest());
        // All three URLs are left untouched.
        assert!(out.contains("https://cdn.example.com/x.png"));
        assert!(out.contains("//cdn.example.com/y.png"));
        assert!(out.contains("data:image/svg+xml"));
    }

    #[test]
    fn rewrite_css_urls_no_change_when_url_not_in_manifest() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("style.css");
        let css = "body { background: url(/images/missing.png); }";
        let out = rewrite_css_urls(css, &css_path, dir.path(), &css_manifest());
        assert_eq!(out, css);
    }

    #[test]
    fn rewrite_css_urls_unterminated_url_does_not_panic() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("style.css");
        let css = "body { background: url(/images/logo.png";
        let out = rewrite_css_urls(css, &css_path, dir.path(), &css_manifest());
        assert!(!out.is_empty());
    }

    #[test]
    #[serial_test::parallel(assets_failpoint)]
    fn after_compile_rewrites_css_url_to_fingerprinted_image() {
        // End-to-end: drop a CSS file referencing a PNG, run the
        // plugin, and confirm the produced CSS points at the
        // fingerprinted PNG name.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(site.join("images")).unwrap();
        // 1×1 transparent PNG (the smallest valid PNG bytes).
        let png_bytes: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00,
            0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89,
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63,
            0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60,
            0x82,
        ];
        fs::write(site.join("images/logo.png"), png_bytes).unwrap();
        fs::write(
            site.join("style.css"),
            "body { background: url(/images/logo.png); }",
        )
        .unwrap();
        fs::write(
            site.join("index.html"),
            r#"<html><head><link rel="stylesheet" href="style.css"></head><body></body></html>"#,
        )
        .unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        FingerprintPlugin.after_compile(&ctx).unwrap();

        // Find the renamed CSS and verify its url() points at the
        // renamed PNG (not the original `logo.png` filename).
        let mut css_text = None;
        for entry in fs::read_dir(&site).unwrap().flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "css") {
                css_text = Some(fs::read_to_string(&p).unwrap());
            }
        }
        let css_text = css_text.expect("renamed CSS file present");
        assert!(
            css_text.contains("/images/logo."),
            "rewritten CSS should reference renamed PNG: {css_text}"
        );
        assert!(css_text.contains(".png"), "still ends in .png: {css_text}");
        // Crucial: the URL is no longer the original `/images/logo.png`
        // — it's `/images/logo.<hash>.png`.
        assert!(
            !css_text.contains("/images/logo.png)"),
            "must no longer point at the un-fingerprinted PNG: {css_text}"
        );
    }

    #[test]
    fn test_fingerprint_file_missing_returns_io_error() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing.css");
        let res =
            fingerprint_file(&missing, dir.path(), SriAlgorithm::default());
        assert!(res.is_err());
        let err = res.unwrap_err();
        // Branch-free variant + path check (`matches!`/`if let` would
        // leave never-taken arms as uncovered regions).
        let debug = format!("{err:?}");
        assert!(debug.contains("Io"));
        assert!(debug.contains("missing.css"));
    }

    #[test]
    fn test_rewrite_css_urls_inplace_missing_returns_io_error() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing.css");
        let manifest = HashMap::new();
        let res = rewrite_css_urls_inplace(&missing, dir.path(), &manifest);
        assert!(res.is_err());
        let err = res.unwrap_err();
        // Branch-free variant + path check (see note above).
        let debug = format!("{err:?}");
        assert!(debug.contains("Io"));
        assert!(debug.contains("missing.css"));
    }

    // -------------------------------------------------------------------
    // Minifier edge branches
    // -------------------------------------------------------------------

    /// Everything this minifier emits must still parse as CSS.
    ///
    /// This is the gate that was missing when `clamp(2.07rem, 1.75rem + 1.6vw,
    /// 3.13rem)` became `clamp(...,1.75rem+1.6vw,...)`: valid-looking output,
    /// rejected by every browser, and every heading in nine themes silently
    /// fell back to the body size. `tests/minification_correctness.rs`
    /// round-trips the *other* minifier — the `lightningcss` one behind the
    /// `minify` feature — but this one runs unconditionally in the fingerprint
    /// path and nothing parsed its output.
    ///
    /// Requires the `minify` feature only because that is what pulls in a CSS
    /// parser; CI runs `--all-features`.
    #[cfg(feature = "minify")]
    #[test]
    fn minified_css_always_reparses() {
        use lightningcss::stylesheet::{ParserOptions, StyleSheet};

        // Constructs chosen because each has a whitespace or delimiter rule
        // that a naive collapser gets wrong.
        let corpus = [
            "h1 { font-size: clamp(2.07rem, 1.75rem + 1.6vw, 3.13rem); }",
            "a { width: calc(100% - 2rem); height: calc(10px*2); }",
            "p { margin: 0 -1px 0 -1px; }",
            "@media (min-width: 48rem) and (max-width: 64rem) { .a { color: red } }",
            ".g { grid-template-columns: repeat(auto-fill, minmax(12rem, 1fr)); }",
            ".s { background: url(\"a b.png\"); content: \"a { b } c\"; }",
            ":root { --x: 1px; --y: calc(var(--x) + 2px); }",
            "@supports (display: grid) { .h { display: grid } }",
            ".t { transition: color .2s ease-in-out, background .3s; }",
            "@font-face { font-family: \"X\"; src: url(x.woff2) format(\"woff2\"); }",
            ".n:not(.a, .b) > .c ~ .d + .e { color: #fff }",
            ".u { inset: 0 auto auto 0; aspect-ratio: 16 / 9; }",
        ];

        for css in corpus {
            let out = minify_css(css);
            assert!(
                StyleSheet::parse(&out, ParserOptions::default()).is_ok(),
                "minified output does not parse as CSS\n  in:  {css}\n  out: {out}"
            );
        }
    }

    /// Minifying twice must equal minifying once.
    ///
    /// A non-idempotent pass is a latent corruption: assets are re-minified
    /// on rebuilds, and each pass would degrade the file a little further.

    /// A descendant combinator before the universal selector is a
    /// space that carries meaning. Dropping it turns `#btn *` into
    /// `#btn*`, which parses as garbage and matches nothing — the rule
    /// is not tightened, it is discarded.
    ///
    /// Three published themes override the search widget with exactly
    /// this shape, and all three overrides were being thrown away: the
    /// widget kept the generator's own text colour and failed AAA
    /// contrast against the theme's own surface. Nothing errored; the
    /// rule simply stopped existing.
    #[test]
    fn minify_css_keeps_the_space_before_a_universal_selector() {
        let out = minify_css("#btn, #btn * { color: red !important; }");
        assert!(
            out.contains("#btn *"),
            "the descendant combinator must survive: {out}"
        );
        assert!(
            !out.contains("#btn*"),
            "must not produce the selector-eating form: {out}"
        );
    }

    /// The same character inside `calc()` needs no space, but keeping
    /// one is valid and costs two bytes — far cheaper than the class of
    /// bug above.
    #[test]
    fn minify_css_leaves_calc_with_a_star_valid() {
        let out = minify_css(".a { width: calc(2px * 3); }");
        assert!(
            out.contains("calc(2px * 3)") || out.contains("calc(2px*3)"),
            "calc must stay valid either way: {out}"
        );
    }

    #[test]
    fn minify_css_is_idempotent() {
        let corpus = [
            "h1 { font-size: clamp(2.07rem, 1.75rem + 1.6vw, 3.13rem); }",
            "a { width: calc(100% - 2rem); }",
            "p { margin: 0 -1px; }",
            "/* c */ .x { color: red } /* d */",
            ".s { content: \"a { b } c\"; }",
        ];
        for css in corpus {
            let once = minify_css(css);
            let twice = minify_css(&once);
            assert_eq!(once, twice, "not idempotent for: {css}");
        }
    }

    /// The same for JavaScript: two passes must agree.
    #[test]
    fn minify_js_is_idempotent() {
        let corpus = [
            "const a = 1; // trailing\nconst b = a / 2;",
            "let s = \"a // not a comment\";",
            "function f() { return 1 /* mid */ + 2; }",
            "const re = /ab+c/g;",
        ];
        for js in corpus {
            let once = minify_js(js);
            let twice = minify_js(&once);
            assert_eq!(once, twice, "not idempotent for: {js}");
        }
    }

    /// Minification must never invent or destroy a string literal's contents.
    #[test]
    fn minify_css_preserves_string_literals_verbatim() {
        let out = minify_css(".a::after { content: \"  two  spaces  \"; }");
        assert!(
            out.contains("\"  two  spaces  \""),
            "string literal was rewritten: {out}"
        );
    }

    #[test]
    fn minify_css_keeps_whitespace_around_plus_in_math() {
        // CSS math requires whitespace around `+`. Collapsing it does not
        // shorten the declaration, it voids it: browsers drop the whole
        // value, so every heading fell back to the inherited size.
        let css = "h1 { font-size: clamp(2.07rem, 1.75rem + 1.6vw, 3.13rem); }";
        let out = minify_css(css);
        assert!(
            out.contains("1.75rem + 1.6vw"),
            "whitespace around `+` was dropped: {out}"
        );
    }

    #[test]
    fn minify_css_keeps_whitespace_around_minus_in_math() {
        let css = "a { width: calc(100% - 2rem); }";
        let out = minify_css(css);
        assert!(out.contains("100% - 2rem"), "got: {out}");
    }

    #[test]
    fn minify_css_still_collapses_ordinary_whitespace() {
        let out = minify_css("body   {   color :  red ;  }");
        assert!(!out.contains("  "), "double space survived: {out}");
        assert!(out.contains("red"), "got: {out}");
    }

    #[test]
    fn minify_css_keeps_space_before_negative_value_in_a_list() {
        // `margin: 0 -1px` must not become `0-1px`.
        let out = minify_css("p { margin: 0 -1px; }");
        assert!(out.contains("0 -1px"), "got: {out}");
    }

    #[test]
    fn minify_css_handles_escaped_quote_inside_string() {
        // The escaped quote must not close the string; the final real
        // quote does (even backslash count).
        let input = "a{content:\"x\\\"y\";}";
        let out = minify_css(input);
        assert!(out.contains("\"x\\\"y\""));
    }

    #[test]
    fn minify_js_handles_escaped_quote_inside_string() {
        let input = "const s = \"a\\\"b\";";
        let out = minify_js(input);
        assert!(out.contains("\"a\\\"b\""));
    }

    #[test]
    fn minify_js_preserves_division_operator() {
        // `/` not followed by `*` or `/` is plain division.
        assert_eq!(minify_js("const x = a / b;"), "const x=a/b;");
    }

    #[test]
    fn minify_js_leading_comment_produces_leading_newline_branch() {
        // A file that opens with a line comment pushes '\n' into an
        // empty result, so the clean pass sees whitespace at i == 0
        // (prev == None).
        assert_eq!(minify_js("// c\nvar x = 1;"), "var x=1;");
    }

    #[test]
    fn minify_css_handles_leading_and_trailing_whitespace() {
        // Exercises the clean-up pass's `_ => false` catch-all at both
        // ends: i == 0 (prev == None) and i == last (next == None).
        assert_eq!(minify_css(" body { color: red; } "), "body{color:red;}");
    }

    #[test]
    fn minify_js_trailing_space_after_word_char_is_dropped() {
        // `prev` is a word char but `next == None` (end of input) —
        // the specific `_ => false` catch-all arm, distinct from the
        // ordinary "not both word chars" (Some, Some) case.
        assert_eq!(minify_js(" var x = 1 "), "var x=1");
    }

    // -------------------------------------------------------------------
    // resolve_css_url edge branches
    // -------------------------------------------------------------------

    #[test]
    fn resolve_css_url_relative_css_dir_hits_curdir_and_escape() {
        // A relative css_dir keeps the leading `./` CurDir component,
        // and the resolved path cannot start with the absolute
        // site_dir prefix, so the URL is rejected.
        let site = Path::new("/abs/site");
        let out = resolve_css_url("img.png", Path::new("./css"), site);
        assert!(out.is_none());
    }

    #[test]
    fn resolve_css_url_rejects_paths_escaping_site_dir() {
        let dir = tempdir().unwrap();
        let site = dir.path();
        let css_dir = site.join("css");
        let out = resolve_css_url("/../../etc/passwd", &css_dir, site);
        assert!(out.is_none());
    }

    // -------------------------------------------------------------------
    // fingerprint_file — rename/write error branches
    // -------------------------------------------------------------------

    #[test]
    fn fingerprint_file_write_fails_when_new_path_squatted_by_dir() {
        // For minified CSS the fingerprinted name is deterministic:
        // sha256(minified). A directory squatting it makes fs::write
        // fail.
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("style.css");
        let css = "body { color: red; }";
        fs::write(&css_path, css).unwrap();
        let hash = sha256_hex(minify_css(css).as_bytes());
        let squat = dir.path().join(format!("style.{}.css", &hash[..8]));
        fs::create_dir_all(squat.join("keep")).unwrap();

        let res =
            fingerprint_file(&css_path, dir.path(), SriAlgorithm::default());
        assert!(res.is_err());
    }

    #[test]
    fn fingerprint_file_rename_fails_when_new_path_is_nonempty_dir() {
        // Non-minified assets go through fs::rename, which fails when
        // the target is a non-empty directory.
        let dir = tempdir().unwrap();
        let png_path = dir.path().join("img.png");
        fs::write(&png_path, b"png-bytes").unwrap();
        let hash = sha256_hex(b"png-bytes");
        let squat = dir.path().join(format!("img.{}.png", &hash[..8]));
        fs::create_dir_all(squat.join("keep")).unwrap();

        let res =
            fingerprint_file(&png_path, dir.path(), SriAlgorithm::default());
        assert!(res.is_err());
    }

    #[test]
    fn fingerprint_file_non_utf8_css_is_renamed_not_minified() {
        let dir = tempdir().unwrap();
        let css_path = dir.path().join("bin.css");
        fs::write(&css_path, [0xFF, 0xFE, 0x00, 0x9F]).unwrap();
        let (rel_old, info) =
            fingerprint_file(&css_path, dir.path(), SriAlgorithm::default())
                .unwrap();
        assert_eq!(rel_old, "bin.css");
        assert!(info.fingerprinted.ends_with(".css"));
        assert!(!css_path.exists(), "original renamed away");
    }

    #[test]
    fn fingerprint_file_non_utf8_js_is_renamed_not_minified() {
        let dir = tempdir().unwrap();
        let js_path = dir.path().join("bin.js");
        fs::write(&js_path, [0xFF, 0xFE, 0x00, 0x9F]).unwrap();
        let (rel_old, info) =
            fingerprint_file(&js_path, dir.path(), SriAlgorithm::default())
                .unwrap();
        assert_eq!(rel_old, "bin.js");
        assert!(info.fingerprinted.ends_with(".js"));
    }

    // -------------------------------------------------------------------
    // after_compile / helpers — IO error propagation
    // -------------------------------------------------------------------

    fn plugin_ctx(root: &Path, site: &Path) -> PluginContext {
        PluginContext::new(root, root, site, root)
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_fails_when_site_has_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let locked = site.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let res =
            FingerprintPlugin.after_compile(&plugin_ctx(dir.path(), &site));

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        // Root CI runners bypass perms; only assert when it errored.
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    fn after_compile_propagates_non_css_fingerprint_error() {
        // Squat the PNG's fingerprinted name so stage 1 fails.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("img.png"), b"png-bytes").unwrap();
        let hash = sha256_hex(b"png-bytes");
        let squat = site.join(format!("img.{}.png", &hash[..8]));
        fs::create_dir_all(squat.join("keep")).unwrap();

        let err = FingerprintPlugin
            .after_compile(&plugin_ctx(dir.path(), &site))
            .unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn after_compile_propagates_css_fingerprint_error() {
        // Squat the CSS's fingerprinted name so stage 2 fails after
        // the url() rewrite pass succeeded.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let css = "body { color: blue; }";
        fs::write(site.join("style.css"), css).unwrap();
        let hash = sha256_hex(minify_css(css).as_bytes());
        let squat = site.join(format!("style.{}.css", &hash[..8]));
        fs::create_dir_all(squat.join("keep")).unwrap();

        let err = FingerprintPlugin
            .after_compile(&plugin_ctx(dir.path(), &site))
            .unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_unreadable_css_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let css_path = site.join("style.css");
        fs::write(&css_path, "body{}").unwrap();
        fs::set_permissions(&css_path, fs::Permissions::from_mode(0o000))
            .unwrap();

        let res =
            FingerprintPlugin.after_compile(&plugin_ctx(dir.path(), &site));

        let _ =
            fs::set_permissions(&css_path, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_fails_when_html_is_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("img.png"), b"png-bytes").unwrap();
        let html = site.join("index.html");
        fs::write(&html, "<img src=\"/img.png\">").unwrap();
        fs::set_permissions(&html, fs::Permissions::from_mode(0o000)).unwrap();

        let res =
            FingerprintPlugin.after_compile(&plugin_ctx(dir.path(), &site));

        let _ = fs::set_permissions(&html, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_fails_when_html_is_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("img.png"), b"png-bytes").unwrap();
        let html = site.join("index.html");
        fs::write(&html, "<img src=\"/img.png\">").unwrap();
        fs::set_permissions(&html, fs::Permissions::from_mode(0o444)).unwrap();

        let res =
            FingerprintPlugin.after_compile(&plugin_ctx(dir.path(), &site));

        let _ = fs::set_permissions(&html, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(unix)]
    fn rewrite_html_references_fails_on_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let locked = site.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let res = rewrite_html_references(&site, &HashMap::new());

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    #[cfg(unix)]
    fn rewrite_css_urls_inplace_write_error_on_readonly_css() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path();
        let css_path = site.join("style.css");
        fs::write(&css_path, "a{background:url(/img.png);}").unwrap();
        let mut manifest = HashMap::new();
        let _ = manifest.insert(
            "img.png".to_string(),
            AssetInfo {
                fingerprinted: "img.deadbeef.png".to_string(),
                sri: "sha384-x".to_string(),
            },
        );
        fs::set_permissions(&css_path, fs::Permissions::from_mode(0o444))
            .unwrap();

        let res = rewrite_css_urls_inplace(&css_path, site, &manifest);

        let _ =
            fs::set_permissions(&css_path, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty());
        }
    }

    #[test]
    fn fingerprint_assets_propagates_missing_file_error() {
        let dir = tempdir().unwrap();
        let missing = vec![dir.path().join("nope.css")];
        let res =
            fingerprint_assets(&missing, dir.path(), SriAlgorithm::default());
        assert!(res.is_err());
    }
}

// =========================================================================
// Fault injection — `assets::remove-original` covers the
// `fs::remove_file(asset_path)` failure path for minified CSS/JS
// assets. The rename-based path (non-CSS/JS, or non-UTF-8 content)
// uses `fs::rename` instead and can't hit this failpoint; genuinely
// making the *original* file un-removable after it has already been
// successfully read, minified, and rewritten to its fingerprinted
// name is impractical to construct without fault injection (e.g. a
// concurrent deletion race), so this is the only way to exercise it.
// =========================================================================
#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial_test::serial(assets_failpoint)]
    fn remove_original_failpoint_propagates() {
        let _guard = FailGuard("assets::remove-original");
        fail::cfg("assets::remove-original", "return")
            .expect("activate failpoint");

        let dir = tempdir().unwrap();
        let css_path = dir.path().join("style.css");
        fs::write(&css_path, "body { color: red; }").unwrap();

        let err =
            fingerprint_file(&css_path, dir.path(), SriAlgorithm::default())
                .expect_err("injected removal failure must propagate");
        assert!(
            format!("{err:?}").contains("injected: assets::remove-original")
        );
        // The fingerprinted file was already written before the
        // injected failure; the original is left in place too since
        // removal never ran.
        assert!(css_path.exists());
    }
}
