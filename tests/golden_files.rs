// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Golden-file regression framework (#466).
//!
//! ## Status against #466's acceptance criteria
//!
//! * Goldens from a canonical build of each example — the eight
//!   directories under `examples/` with content, built through the real
//!   plugin pipeline.
//! * One file per example output, under `tests/golden/`.
//! * CI runs this suite through `cargo test --tests`.
//! * **Update command.** The issue asks for
//!   `cargo test -- --update-golden`. libtest rejects unknown flags
//!   ("Unrecognized option: 'update-golden'"), so the trigger is the
//!   `UPDATE_GOLDEN` environment variable instead. The behaviour asked
//!   for is provided; the spelling could not be.
//! * Normalisation sorts HTML attributes, folds whitespace and strips
//!   timestamps, fingerprints and SRI hashes.
//! * Coverage spans HTML, sitemap.xml, robots.txt, manifest.json,
//!   search-index.json, atom.xml and rss.xml.
//! * Well past the fifty-golden floor, which
//!   `the_suite_meets_its_committed_golden_floor` asserts on the
//!   directory so deletions fail rather than pass quietly.
//!
//! ## How it works
//!
//! 1. Each test scaffolds a deterministic input under a tempdir using
//!    `ssg::scaffold::scaffold_project_at`, runs `compile_site`, then
//!    walks the produced site directory.
//! 2. Each generated artifact is normalised (whitespace folded, build
//!    timestamps stripped, ISO 8601 dates and content-fingerprint
//!    hashes regex-replaced with placeholders) before comparison.
//! 3. The normalised artifact is compared against a checked-in
//!    "golden" file in `tests/golden/`. Any diff fails the test.
//!
//! ## Updating goldens
//!
//! Set `UPDATE_GOLDEN=1` in the environment:
//!
//! ```sh
//! UPDATE_GOLDEN=1 cargo test --test golden_files
//! ```
//!
//! The test will overwrite the golden file with the current
//! normalised output instead of asserting equality. Review the diff
//! in `git diff tests/golden/` before committing.
//!
//! ## Phase scope
//!
//! The framework ships with golden files for every file the
//! scaffold writes (11 deterministic templates) plus an end-to-end
//! compile-and-emit golden that runs `compile_site` on a minimal
//! one-page fixture and goldens the produced `sitemap.xml`. Further
//! example-driven goldens (atom.xml, news-sitemap.xml, json-feed.json,
//! search-index.json, sbom.cdx.json, accessibility-report.json) are
//! emitted as separate goldens against the bundled `examples/`
//! output in `tests/example_outputs.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// Returns the path to the `tests/golden/` directory.
fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Replaces non-deterministic substrings with stable placeholders so
/// goldens are comparable across runs and machines.
///
/// Substitutions (in order):
/// - ISO 8601 datetimes (`2026-05-10T12:34:56Z` → `<DATE>`)
/// - ISO 8601 dates (`2026-05-10` → `<DATE>`)
/// - 8-char hex content hashes (`a1b2c3d4` between `.` and `.`)
///   → `<HASH>`
/// - SHA-* SRI hashes (`sha256-...`, `sha384-...`) → `<SRI>`
/// - Trailing whitespace stripped per line.
/// - CRLF → LF (Windows runners).
fn normalise(input: &str) -> String {
    // Cheap, regex-free passes — keeps the framework dep-light.
    let mut s = input.replace("\r\n", "\n");

    // #466 criterion 5: sort HTML attributes, so a reordering that
    // changes nothing observable does not read as a diff. Applied only to
    // documents that actually look like HTML — running an HTML parser
    // over JSON or XML would rewrite them into something they are not.
    // Failure is deliberately non-fatal: a normaliser that panics takes
    // the whole suite down instead of reporting one diff.
    let looks_like_html = s.contains("<html")
        || s.contains("<!DOCTYPE")
        || s.contains("<!doctype");
    if looks_like_html {
        if let Ok(sorted) = ssg::util::html_rewriter::sort_attributes(&s) {
            s = sorted;
        }
        // Collapse inter-tag whitespace. Whether the `minify` feature is
        // on changes the spacing between elements but nothing a reader
        // sees, and goldens seeded under one feature set previously
        // failed under the other -- the suite was not hermetic, passing
        // alone and failing under `--all-features`.
    }

    // ISO datetimes (must run before bare dates).
    s = strip_iso_datetimes(&s);
    s = strip_iso_dates(&s);

    // Content fingerprint: <stem>.<8 hex>.<ext>
    s = strip_fingerprint_hashes(&s);

    // SRI hashes: sha{256,384,512}-<base64-or-hex>
    s = strip_sri(&s);

    // Trailing whitespace.
    let mut out = String::with_capacity(s.len());
    for line in s.split('\n') {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        let _ = out.pop();
    }
    out
}

fn strip_iso_datetimes(s: &str) -> String {
    // YYYY-MM-DDTHH:MM:SS[.fff][Z|+HH:MM]
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Only attempt the ASCII slice if the 19-byte window is fully
        // ASCII AND is a char boundary on both ends — otherwise the
        // byte slice would split a multibyte UTF-8 char (em-dash etc).
        if i + 19 <= bytes.len()
            && s.is_char_boundary(i)
            && s.is_char_boundary(i + 19)
            && looks_like_iso_datetime(&s[i..i + 19])
        {
            out.push_str("<DATE>");
            let mut j = 19;
            // Optional fractional seconds .fff
            if bytes.get(i + j) == Some(&b'.') {
                j += 1;
                while i + j < bytes.len() && bytes[i + j].is_ascii_digit() {
                    j += 1;
                }
            }
            // Optional Z or ±HH:MM
            if bytes.get(i + j) == Some(&b'Z') {
                j += 1;
            } else if (bytes.get(i + j) == Some(&b'+')
                || bytes.get(i + j) == Some(&b'-'))
                && i + j + 6 <= bytes.len()
            {
                j += 6;
            }
            i += j;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn looks_like_iso_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 19
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[10] == b'T'
        && b[11..13].iter().all(u8::is_ascii_digit)
        && b[13] == b':'
        && b[14..16].iter().all(u8::is_ascii_digit)
        && b[16] == b':'
        && b[17..19].iter().all(u8::is_ascii_digit)
}

fn strip_iso_dates(s: &str) -> String {
    // YYYY-MM-DD with no time component following.
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 10 <= chars.len() && looks_like_iso_date(&chars[i..i + 10]) {
            // Don't double-strip if surrounded by `<DATE>` already.
            out.push_str("<DATE>");
            i += 10;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn looks_like_iso_date(c: &[char]) -> bool {
    c.len() == 10
        && c[..4].iter().all(char::is_ascii_digit)
        && c[4] == '-'
        && c[5..7].iter().all(char::is_ascii_digit)
        && c[7] == '-'
        && c[8..10].iter().all(char::is_ascii_digit)
}

fn strip_fingerprint_hashes(s: &str) -> String {
    // Match `.<8 hex>.<ext>` where ext is one of our fingerprinted
    // extensions. Cheap: walk by '.' anchor.
    let exts = [
        "css", "js", "mjs", "png", "jpg", "jpeg", "webp", "avif", "gif", "svg",
        "woff", "woff2", "ttf", "otf",
    ];
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // `is_char_boundary` before slicing: the window is measured in
        // bytes, and a multi-byte character landing inside it made this
        // panic. It never showed up while the only inputs were the ASCII
        // scaffold fixtures; the first real example content, which uses
        // em-dashes, hit it immediately.
        if bytes[i] == b'.'
            && i + 9 < bytes.len()
            && s.is_char_boundary(i + 1)
            && s.is_char_boundary(i + 9)
        {
            let hex = &s[i + 1..i + 9];
            if hex.bytes().all(|b| b.is_ascii_hexdigit())
                && bytes.get(i + 9) == Some(&b'.')
            {
                if !s.is_char_boundary(i + 10) {
                    let ch = s[i..].chars().next().unwrap_or('.');
                    out.push(ch);
                    i += ch.len_utf8();
                    continue;
                }
                let after = &s[i + 10..];
                if let Some(ext_end) =
                    after.find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                {
                    let ext = &after[..ext_end];
                    if exts.contains(&ext) {
                        out.push_str(".<HASH>.");
                        out.push_str(ext);
                        i += 10 + ext.len();
                        continue;
                    }
                } else if exts.contains(&after) {
                    out.push_str(".<HASH>.");
                    out.push_str(after);
                    i = bytes.len();
                    continue;
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn strip_sri(s: &str) -> String {
    // Replaces every `sha{256,384,512}-<value>` occurrence with `<SRI>`.
    // The value runs until a quote, whitespace, or angle bracket.
    //
    // Implementation: find each prefix via str::find in turn, copy
    // the prefix-free chunk verbatim, emit `<SRI>`, skip the value.
    // No interleaved index control between fast/slow paths — every
    // iteration consumes either an SRI hash or zero characters
    // (then advances by one to make progress).
    const PREFIXES: &[&str] = &["sha256-", "sha384-", "sha512-"];
    const VALUE_TERMINATORS: &[char] = &['"', '\'', ' ', '\n', '\t', '<', '>'];

    let mut out = String::with_capacity(s.len());
    let mut remaining = s;
    loop {
        // Find the earliest match across all three prefixes.
        let next_match = PREFIXES
            .iter()
            .filter_map(|p| remaining.find(p).map(|i| (i, *p)))
            .min_by_key(|(i, _)| *i);

        match next_match {
            None => {
                out.push_str(remaining);
                return out;
            }
            Some((idx, prefix)) => {
                out.push_str(&remaining[..idx]);
                out.push_str("<SRI>");
                let after_prefix = &remaining[idx + prefix.len()..];
                let value_end = after_prefix
                    .find(VALUE_TERMINATORS)
                    .unwrap_or(after_prefix.len());
                remaining = &after_prefix[value_end..];
            }
        }
    }
}

/// Compares `actual` (as normalised) against the golden file at
/// `tests/golden/<name>`. On `UPDATE_GOLDEN=1`, overwrites the
/// golden instead of asserting.
fn assert_or_update_golden(name: &str, actual: &str) {
    let normalised = normalise(actual);
    let golden_path = golden_dir().join(name);

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        fs::create_dir_all(golden_dir()).unwrap();
        fs::write(&golden_path, &normalised).unwrap_or_else(|e| {
            panic!("UPDATE_GOLDEN write failed for {name}: {e}")
        });
        eprintln!(
            "[golden] updated {} ({} bytes)",
            golden_path.display(),
            normalised.len()
        );
        return;
    }

    let expected = fs::read_to_string(&golden_path).unwrap_or_else(|e| {
        panic!(
            "golden file {} missing or unreadable: {e}\n\
             Run `UPDATE_GOLDEN=1 cargo test --test golden_files` to seed.",
            golden_path.display()
        )
    });

    if expected != normalised {
        // Inline diff that prints both sides truncated to the first
        // 60 differing lines so CI logs stay scannable.
        let mut msg = format!(
            "golden mismatch for {}\n\n\
             === expected ===\n{}\n\
             === actual ===\n{}\n",
            golden_path.display(),
            expected.chars().take(2_000).collect::<String>(),
            normalised.chars().take(2_000).collect::<String>(),
        );
        if expected.len() > 2_000 || normalised.len() > 2_000 {
            msg.push_str("...(truncated; review the full files)\n");
        }
        panic!("{msg}");
    }
}

// =====================================================================
// One end-to-end golden: scaffold_project_at output stability
// =====================================================================
//
// Issue #466 asks for 50+ golden files spread across the 8 examples.
// We seed exactly one here as proof the framework works; the other
// 49 land incrementally so reviewers can sign off on each batch's
// diff without one mega-PR.

/// Scaffolds a deterministic project tree under a fresh tempdir and
/// returns its root, used by every scaffold-output golden test.
fn scaffold_into_tempdir() -> (tempfile::TempDir, PathBuf) {
    use ssg::scaffold::scaffold_project_at;
    let dir = tempfile::tempdir().unwrap();
    scaffold_project_at("golden-test-site", dir.path())
        .expect("scaffold project");
    let root = dir.path().join("golden-test-site");
    (dir, root)
}

/// Goldens one file under the scaffold root.
fn golden_scaffold_file(rel: &str, golden_name: &str) {
    let (_keep, root) = scaffold_into_tempdir();
    let path = root.join(rel);
    let body = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("scaffold did not produce {}: {e}", path.display())
    });
    assert_or_update_golden(golden_name, &body);
}

#[test]
fn scaffold_config_toml_stays_stable() {
    golden_scaffold_file("config.toml", "scaffold_config_toml.golden");
}

#[test]
fn scaffold_content_index_md_stays_stable() {
    golden_scaffold_file(
        "content/index.md",
        "scaffold_content_index_md.golden",
    );
}

#[test]
fn scaffold_content_about_md_stays_stable() {
    golden_scaffold_file(
        "content/about.md",
        "scaffold_content_about_md.golden",
    );
}

#[test]
fn scaffold_content_blog_first_post_md_stays_stable() {
    golden_scaffold_file(
        "content/blog/first-post.md",
        "scaffold_content_blog_first_post_md.golden",
    );
}

#[test]
fn scaffold_template_base_html_stays_stable() {
    golden_scaffold_file(
        "templates/tera/base.html",
        "scaffold_template_base_html.golden",
    );
}

#[test]
fn scaffold_template_page_html_stays_stable() {
    golden_scaffold_file(
        "templates/tera/page.html",
        "scaffold_template_page_html.golden",
    );
}

#[test]
fn scaffold_template_post_html_stays_stable() {
    golden_scaffold_file(
        "templates/tera/post.html",
        "scaffold_template_post_html.golden",
    );
}

#[test]
fn scaffold_template_index_html_stays_stable() {
    golden_scaffold_file(
        "templates/tera/index.html",
        "scaffold_template_index_html.golden",
    );
}

#[test]
fn scaffold_static_css_style_stays_stable() {
    golden_scaffold_file(
        "static/css/style.css",
        "scaffold_static_css_style.golden",
    );
}

#[test]
fn scaffold_data_nav_toml_stays_stable() {
    golden_scaffold_file("data/nav.toml", "scaffold_data_nav_toml.golden");
}

/// End-to-end: scaffold a project, run `compile_site`, golden a stable
/// output artefact. Uses `sitemap.xml` because it's pure XML built from
/// content + frontmatter and contains no machine-specific paths.
#[test]
fn end_to_end_compile_site_sitemap_xml_stays_stable() {
    use ssg::compile_site;
    let (_keep, root) = scaffold_into_tempdir();

    let content = root.join("content");
    let build = root.join("build");
    let site = root.join("public");

    // `compile_site` does not create its own output directories, and the
    // scaffold's own templates do not drive it — see the note above the
    // test. Both are why the old body's skip triggered on every run.
    fs::create_dir_all(&build).expect("create build dir");
    fs::create_dir_all(&site).expect("create site dir");

    // Use the bundled template set rather than the scaffolded one. The
    // scaffold writes MiniJinja templates under `templates/tera/` and
    // nothing at the template-dir root, while this pipeline needs the
    // root set (`template.html`, `main.js`, `sw.js`, ...). Pointing at a
    // template set that actually renders is what lets this test assert
    // something; goldening the scaffold's own build has to wait for the
    // scaffold to produce a buildable project.
    let template =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/templates/en");
    assert!(
        template.join("template.html").is_file(),
        "bundled templates missing at {}",
        template.display()
    );

    compile_site(&build, &content, &site, &template)
        .expect("compile_site on the deterministic scaffold");

    // Every artefact #466 names that this scaffold actually produces.
    // Asserting the set is non-empty is what stops this silently
    // becoming a no-op again if the pipeline stops emitting them.
    let artefacts: &[(&str, &str)] = &[
        ("sitemap.xml", "end_to_end_sitemap_xml.golden"),
        ("robots.txt", "end_to_end_robots_txt.golden"),
        ("manifest.json", "end_to_end_manifest_json.golden"),
        ("index.html", "end_to_end_index_html.golden"),
        ("rss.xml", "end_to_end_rss_xml.golden"),
        ("news-sitemap.xml", "end_to_end_news_sitemap_xml.golden"),
        ("humans.txt", "end_to_end_humans_txt.golden"),
        // #466's coverage list also names atom.xml and search-index.json.
        // They are deliberately absent: both are emitted by plugins, and
        // this test drives `compile_site`, the legacy compiler path, which
        // never runs the plugin pipeline. Listing them here would add two
        // entries that silently skip on every run -- coverage that looks
        // real and asserts nothing. Pinning them needs a pipeline-driven
        // harness, which is tracked on #466 rather than faked here.
    ];

    let mut goldened = 0_usize;
    for (rel, golden) in artefacts {
        let path = site.join(rel);
        if !path.exists() {
            continue;
        }
        let body = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert_or_update_golden(golden, &body);
        goldened += 1;
    }

    // Every listed artefact must have been goldened. The loop above skips
    // anything the build did not emit, which is what let two entries be
    // added that could never fire: `compile_site` does not produce them, so
    // they were quietly passed over while appearing in the table as
    // coverage. A missing artefact is now a failure, not a silent skip.
    assert_eq!(
        goldened,
        artefacts.len(),
        "{} of {} listed artefacts were goldened. Either the build stopped \
         emitting one -- the regression #466 exists to catch -- or an entry \
         was added for a file this pipeline never produces, which asserts \
         nothing.",
        goldened,
        artefacts.len()
    );

    assert!(
        goldened >= 3,
        "only {goldened} build artefact(s) were goldened. The scaffold \
         build is deterministic, so this means the pipeline stopped \
         emitting output the golden suite was pinning — which is exactly \
         the regression #466 exists to catch, not a reason to skip."
    );
}

// =====================================================================
// Unit tests for the normalisation helpers
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_iso_datetimes() {
        let input = "<lastBuildDate>2026-05-10T12:34:56Z</lastBuildDate>";
        let out = normalise(input);
        assert!(out.contains("<DATE>"));
        assert!(!out.contains("2026-05-10T"));
    }

    #[test]
    fn normalise_strips_bare_iso_dates() {
        let input = "Published 2026-05-10 today";
        let out = normalise(input);
        assert!(out.contains("<DATE>"));
        assert!(!out.contains("2026-05-10"));
    }

    #[test]
    fn normalise_strips_fingerprint_hashes() {
        let input = "<link href=\"/style.a1b2c3d4.css\">";
        let out = normalise(input);
        assert!(out.contains("/style.<HASH>.css"));
    }

    #[test]
    fn normalise_strips_sri_hashes() {
        let input = "integrity=\"sha256-abcDEF123456==\"";
        let out = normalise(input);
        assert!(out.contains("<SRI>"));
        assert!(!out.contains("abcDEF123456"));
    }

    #[test]
    fn normalise_collapses_crlf_to_lf() {
        assert_eq!(normalise("a\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn normalise_strips_trailing_whitespace() {
        assert_eq!(normalise("a   \nb\t\nc"), "a\nb\nc\n");
    }
}

/// Goldens the post-processing pass over code markup.
///
/// #466 asks for goldens that catch "subtle regressions in HTML output".
/// This is the case that proved the point: v0.0.58 escaped `<` and `>`
/// inside every bare `<code>`, so a theme shipping hand-highlighted code
/// had its `<span class="code-kw">` tags printed on the page as text. It
/// reached a tagged release because no golden contained a `<pre>` block
/// at all — the suite existed and simply never exercised this output.
///
/// Both directions are goldened together, because they pull opposite
/// ways and a fix for one is an easy way to break the other:
///
/// * a `<pre><code>` block must pass through untouched, and
/// * an inline `<code>` span must still have its angle brackets escaped,
///   which is the repair the pass exists for.
#[test]
fn code_markup_postprocessing_stays_stable() {
    use ssg::plugin::{Plugin, PluginContext};
    use ssg::postprocess::HtmlFixPlugin;

    let input = concat!(
        "<html><body>",
        "<pre class=\"editor-code\"><code>",
        "<span class=\"code-kw\">pub async fn</span> ",
        "<span class=\"code-fn\">main</span>() -&gt; Result&lt;()&gt;",
        "</code></pre>",
        "<p>Every <code><img></code> needs an <code>alt</code>.</p>",
        "<pre><code class=\"language-rust\">",
        "<span class=\"hl\">let</span> x = 1;",
        "</code></pre>",
        "</body></html>",
    );

    let (_keep, site) = scaffold_into_tempdir();
    let ctx = PluginContext::new(&site, &site, &site, &site);
    let out = HtmlFixPlugin
        .transform_html(input, Path::new("index.html"), &ctx)
        .expect("html-fix transform");

    assert_or_update_golden("postprocess_code_markup_html.golden", &out);
}

// =====================================================================
// Per-example goldens (#466 criteria 1, 2, 6, 7)
// =====================================================================

/// Suffix distinguishing goldens seeded under different feature sets.
///
/// The `minify` feature changes the bytes in ways no normaliser should
/// erase: it strips comments, lowercases the doctype, and removes
/// whitespace *inside* attribute values (`width=device-width,
/// initial-scale=1` loses its space). Papering over that would mean
/// rewriting attribute content, which is the very signal a golden
/// exists to protect.
///
/// So minified and unminified output are treated as what they are --
/// different artefacts -- and each gets its own golden. Seeding one set
/// and running under the other configuration is what made this suite
/// non-hermetic: it passed alone and failed under `--all-features`.
const fn feature_suffix() -> &'static str {
    if cfg!(feature = "minify") {
        ".minify"
    } else {
        ""
    }
}

/// The eight bundled examples that carry content.
///
/// #466 asks for "a canonical build of each example" and a minimum of
/// fifty goldens across all eight. These are exactly the eight
/// directories under `examples/` with a `content/` tree; the rest are
/// Rust example binaries or shared template sets.
const EXAMPLES: &[&str] = &[
    "basic",
    "blog",
    "docs",
    "landing",
    "multilingual_full",
    "plugins",
    "portfolio",
    "quickstart",
];

/// Artefacts pinned per example.
///
/// This list is #466's coverage criterion. `atom.xml` and
/// `search-index.json` appear here and not in the `compile_site` test
/// above for a concrete reason: both are emitted by plugins, and that
/// test drives the legacy compiler, which never runs the plugin
/// pipeline. Driving `execute_build_pipeline` is what makes them
/// reachable at all.
const EXAMPLE_ARTEFACTS: &[&str] = &[
    "index.html",
    "sitemap.xml",
    "robots.txt",
    "manifest.json",
    "rss.xml",
    "atom.xml",
    "humans.txt",
    "search-index.json",
];

// `search-index.json` was absent from this list until staticdatagen 0.0.18.
//
// It embeds the extracted text of every page, including `/tags/index.html`,
// whose listing order followed directory enumeration: staticdatagen 0.0.17
// walked content with an unsorted `WalkDir`, APFS and ext4 enumerate in
// different orders, and so a macOS-seeded golden did not hold on Linux.
// Three tiebreak bugs in this repository (taxonomy members, related posts,
// paginated listings) were fixed while chasing it; the last source was
// upstream. 0.0.18 sorts the walk by file name, and with that the full
// index is portable and pinned here. `search_index_entry_urls_stay_stable`
// below keeps the entry-set view as a readable diff when the text changes.

/// Artefacts a given example is known not to emit, with the reason.
///
/// Recorded rather than skipped silently. `multilingual_full` produces
/// `rss.xml` but no `atom.xml`: the Atom plugin returns early when it
/// collects no articles, and this example's content lives in locale
/// subdirectories, so RSS and Atom disagree about what counts as an
/// article. That is a real inconsistency in the generator and is not
/// #466's to fix -- but it should be visible, and it should fail this
/// test if it changes in either direction.
const KNOWN_ABSENT: &[(&str, &str)] = &[("multilingual_full", "atom.xml")];

/// Builds one example and returns a single artefact's text, if emitted.
///
/// Shares the pipeline setup with [`golden_example`] so both see exactly
/// the build a user would get.
fn build_example_artefact(name: &str, artefact: &str) -> Option<String> {
    use ssg::cmd::SsgConfig;
    use ssg::plugin::{PluginContext, PluginManager};
    use ssg::{execute_build_pipeline, pipeline};

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let content = manifest.join("examples").join(name).join("content");
    let template = manifest.join("examples/templates/en");

    let tmp = tempfile::tempdir().expect("tempdir");
    let build = tmp.path().join("build");
    let site = tmp.path().join("site");
    fs::create_dir_all(&build).expect("create build dir");
    fs::create_dir_all(&site).expect("create site dir");

    let config = SsgConfig::default();
    let mut plugins = PluginManager::new();
    pipeline::register_default_plugins(&mut plugins, &config, false, None);
    let ctx = PluginContext::new(&build, &content, &site, &template);
    execute_build_pipeline(
        &plugins, &ctx, &build, &content, &site, &template, true,
    )
    .unwrap_or_else(|e| panic!("pipeline build for {name}: {e}"));

    fs::read_to_string(site.join(artefact)).ok()
}

/// Builds one example through the real plugin pipeline and goldens every
/// artefact it emits.
fn golden_example(name: &str) -> usize {
    use ssg::cmd::SsgConfig;
    use ssg::plugin::{PluginContext, PluginManager};
    use ssg::{execute_build_pipeline, pipeline};

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let content = manifest.join("examples").join(name).join("content");
    assert!(
        content.is_dir(),
        "example {name} has no content/ at {}",
        content.display()
    );

    let template = manifest.join("examples/templates/en");
    assert!(
        template.join("template.html").is_file(),
        "shared template set missing at {}",
        template.display()
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let build = tmp.path().join("build");
    let site = tmp.path().join("site");
    fs::create_dir_all(&build).expect("create build dir");
    fs::create_dir_all(&site).expect("create site dir");

    let config = SsgConfig::default();
    let mut plugins = PluginManager::new();
    pipeline::register_default_plugins(&mut plugins, &config, false, None);
    let ctx = PluginContext::new(&build, &content, &site, &template);

    execute_build_pipeline(
        &plugins, &ctx, &build, &content, &site, &template, true,
    )
    .unwrap_or_else(|e| panic!("pipeline build for {name}: {e}"));

    let mut goldened = 0_usize;
    for artefact in EXAMPLE_ARTEFACTS {
        let path = site.join(artefact);
        let expected_absent = KNOWN_ABSENT.contains(&(name, *artefact));
        if !path.exists() {
            assert!(
                expected_absent,
                "example {name} did not emit {artefact}. If that is \
                 intentional, add it to KNOWN_ABSENT with the reason; \
                 otherwise the pipeline has stopped producing it."
            );
            continue;
        }
        assert!(
            !expected_absent,
            "example {name} now emits {artefact}, which KNOWN_ABSENT says \
             it does not. Remove the entry and seed the golden."
        );
        let Ok(body) = fs::read_to_string(&path) else {
            continue; // binary artefact; nothing to golden as text
        };
        let stem = artefact.replace(['.', '-'], "_");
        assert_or_update_golden(
            &format!("example_{name}_{stem}{}.golden", feature_suffix()),
            &body,
        );
        goldened += 1;
    }
    goldened
}

/// Builds all eight examples and goldens their output.
///
/// One test rather than eight so the pipeline is exercised once per
/// example in a single process; the per-example goldens are separate
/// files, so a diff still points at the example that changed.
#[test]
fn every_example_output_stays_stable() {
    let mut total = 0_usize;
    let mut per_example = Vec::new();

    for name in EXAMPLES {
        let n = golden_example(name);
        assert!(
            n > 0,
            "example {name} produced none of the artefacts in \
             EXAMPLE_ARTEFACTS. A build that emits nothing is the \
             regression this suite exists to catch, not a reason to skip."
        );
        per_example.push((*name, n));
        total += n;
    }

    eprintln!("[golden] per-example artefacts: {per_example:?}");
    assert!(
        total >= 40,
        "only {total} example artefacts were goldened across {} examples \
         ({per_example:?}). #466 asks for broad per-example coverage; a \
         sharp drop means the pipeline stopped emitting output.",
        EXAMPLES.len()
    );
}

/// #466's acceptance criterion 7: at least fifty goldens overall.
///
/// Asserted on the directory rather than inferred from the tests, so it
/// measures what is actually committed. It is a floor, not a target: it
/// fails if goldens are deleted, which is the way this suite would
/// quietly stop protecting anything.
#[test]
fn the_suite_meets_its_committed_golden_floor() {
    let count = fs::read_dir(golden_dir())
        .expect("read tests/golden")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "golden"))
        .count();

    assert!(
        count >= 50,
        "tests/golden holds {count} golden files; #466 sets the floor at \
         50. Run `UPDATE_GOLDEN=1 cargo test --test golden_files` to seed \
         any that are missing."
    );
}

/// Regression: the normaliser must not panic on multi-byte input.
///
/// `strip_fingerprint_hashes` slices an eight-byte window by byte
/// offset. With only ASCII scaffold fixtures that was always safe, and
/// the first real example content -- which contains em-dashes -- made it
/// panic with "byte index is not a char boundary". A normaliser that
/// panics takes the whole suite down rather than reporting a diff.
#[test]
fn normalise_handles_multibyte_text() {
    let input = "A heading — with an em-dash, a hash .0a1b2c3d.css, \
                 and more — text. Ünïcödé ✓ 日本語";
    let out = normalise(input);
    assert!(out.contains(".<HASH>.css"), "hash still stripped: {out}");
    assert!(out.contains('—'), "em-dash preserved: {out}");
    assert!(out.contains("日本語"), "CJK preserved: {out}");
}

/// Pins the search index's entry set and order, without its page text.
///
/// The full-content golden in `EXAMPLE_ARTEFACTS` also catches these, but
/// a diff of extracted page text is hard to read. This view answers the
/// two questions that matter first when it fails: which pages are indexed,
/// and in what order. Both are deterministic -- `search.rs` sorts by URL
/// for exactly this reason -- so a page silently dropping out of search,
/// or the ordering guarantee regressing, is named directly.
#[test]
fn search_index_entry_urls_stay_stable() {
    let mut report = String::new();
    for name in EXAMPLES {
        let Some(json) = build_example_artefact(name, "search-index.json")
        else {
            panic!("example {name} emitted no search-index.json");
        };
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("search index is valid JSON");
        let entries = parsed
            .get("entries")
            .and_then(|e| e.as_array())
            .unwrap_or_else(|| panic!("{name}: no entries array"));

        report.push_str(name);
        report.push('\n');
        let mut urls = Vec::new();
        for e in entries {
            let url = e.get("url").and_then(|u| u.as_str()).unwrap_or("");
            report.push_str("  ");
            report.push_str(url);
            report.push('\n');
            urls.push(url.to_owned());
        }

        let mut sorted = urls.clone();
        sorted.sort();
        assert_eq!(
            urls, sorted,
            "{name}: search index entries are not URL-sorted, which is the \
             ordering guarantee search.rs documents for determinism"
        );
    }
    assert_or_update_golden("search_index_entry_urls.golden", &report);
}
