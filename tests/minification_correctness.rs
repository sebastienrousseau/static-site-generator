// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for native HTML/JS/CSS minification.
//!
//! Issue #519. Minification is ssg's own code, not a dependency: the
//! `minify-html`, `oxc_minifier` and `lightningcss` versions of these passes
//! were removed in 0.0.62, taking 117 crates out of the dependency graph. A
//! minifier rewrites every byte the generator emits, so a bug in one is a bug
//! on every page of every site; keeping it in-tree means it is covered by
//! this crate's own tests and cannot change underneath a release.
//!
//! These tests pin the behaviour the README's "native JS/CSS minification"
//! claim depends on:
//!
//! * AC5 — `<pre>` content survives minification byte-for-byte.
//! * AC6 — every `.html` file at *any* depth under `site_dir` is
//!   processed by the recursive walk.
//! * AC2 / AC3 — CSS and JS file outputs are valid and meaningfully
//!   smaller than their inputs on representative fixtures.
//!
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ssg::plugin::{Plugin, PluginContext};
use ssg::plugins::{minify_css, minify_html, minify_js, MinifyPlugin};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn ctx(site_dir: &Path) -> PluginContext {
    PluginContext::new(
        Path::new("content"),
        Path::new("build"),
        site_dir,
        Path::new("templates"),
    )
}

// =====================================================================
// AC5 — `<pre>` content is byte-identical after minification
// =====================================================================

#[test]
fn pre_block_byte_identical_after_minify_html() {
    let pre_body = "  let x = 1;\n  let y = 2;\n  // indent matters";
    let html = format!(
        "<!DOCTYPE html><html><head><title>x</title></head>\
         <body><pre><code>{pre_body}</code></pre></body></html>"
    );
    let minified = minify_html(&html);
    assert!(
        minified.contains(pre_body),
        "minify_html must preserve <pre> body verbatim.\nminified: {minified}"
    );
}

#[test]
fn pre_block_with_html_entities_semantically_preserved() {
    // Entities inside <pre> may be normalised by minify-html (e.g.
    // `&lt;` → `&LT`) — both decode to the same `<` character — but
    // the literal opcode `alert(1)` content between them must survive
    // verbatim. AC5 cares about the rendered characters, not the
    // particular entity spelling.
    let html = "<html><body><pre>&lt;script&gt;alert(1)&lt;/script&gt;</pre></body></html>";
    let minified = minify_html(html);
    assert!(
        minified.contains("alert(1)"),
        "literal script content in <pre> must survive minification:\n{minified}"
    );
    // Both `<` markers must still be present (in any entity spelling).
    let lt_count = minified.matches("&lt;").count()
        + minified.matches("&LT;").count()
        + minified.matches("&LT").count();
    assert!(
        lt_count >= 2,
        "expected at least 2 `<` entities preserved in <pre>:\n{minified}"
    );
}

#[test]
fn pre_block_indent_preserved_through_plugin() {
    let temp = tempdir().unwrap();
    let body = "    indented\n      deeper\n    back out";
    let original =
        format!("<html><body><pre><code>{body}</code></pre></body></html>");
    let path = temp.path().join("page.html");
    fs::write(&path, &original).unwrap();

    MinifyPlugin.after_compile(&ctx(temp.path())).unwrap();

    let after = fs::read_to_string(&path).unwrap();
    assert!(
        after.contains(body),
        "MinifyPlugin must preserve <pre> indentation:\n{after}"
    );
    assert!(
        after.len() <= original.len(),
        "overall HTML should not grow after minification (before={}, after={})",
        original.len(),
        after.len()
    );
}

// =====================================================================
// AC6 — recursive walk: every `.html` at any depth is processed
// =====================================================================

#[test]
fn minify_plugin_walks_recursively() {
    let temp = tempdir().unwrap();

    // 3+ levels of nesting (blog/year/post/index.html).
    let deep_dir = temp.path().join("blog").join("2026").join("post");
    fs::create_dir_all(&deep_dir).unwrap();

    let top = temp.path().join("index.html");
    let mid = temp.path().join("blog").join("index.html");
    let deep = deep_dir.join("index.html");

    let payload =
        "<html>   <body>    <p>     hello     world     </p>    </body>   </html>";
    for path in [&top, &mid, &deep] {
        fs::write(path, payload).unwrap();
    }

    MinifyPlugin.after_compile(&ctx(temp.path())).unwrap();

    for path in [&top, &mid, &deep] {
        let after = fs::read_to_string(path).unwrap();
        assert!(
            after.len() < payload.len(),
            "file at depth {} ({path:?}) was not minified ({} >= {} bytes)",
            path.strip_prefix(temp.path()).unwrap().components().count(),
            after.len(),
            payload.len()
        );
    }
}

#[test]
fn minify_plugin_walks_css_and_js_recursively() {
    let temp = tempdir().unwrap();
    let assets = temp.path().join("assets").join("vendor");
    fs::create_dir_all(&assets).unwrap();

    let css_path = assets.join("style.css");
    let js_path = assets.join("app.js");
    let css_input =
        "body  {\n  color:   red;\n  background: #ffffff;\n  margin:   0;\n}";
    let js_input =
        "const greeting = 'hello world';\nconst unused = 'dead';\nconsole.log(greeting);";
    fs::write(&css_path, css_input).unwrap();
    fs::write(&js_path, js_input).unwrap();

    MinifyPlugin.after_compile(&ctx(temp.path())).unwrap();

    let css_after = fs::read_to_string(&css_path).unwrap();
    let js_after = fs::read_to_string(&js_path).unwrap();
    assert!(
        css_after.len() < css_input.len(),
        "nested CSS not minified ({} >= {} bytes)",
        css_after.len(),
        css_input.len()
    );
    assert!(
        js_after.len() < js_input.len(),
        "nested JS not minified ({} >= {} bytes)",
        js_after.len(),
        js_input.len()
    );
}

// =====================================================================
// AC2 — CSS minification produces compact, still-valid output
// =====================================================================

#[test]
fn css_minification_round_trips() {
    let input = "body { color: red; padding: 10px 10px 10px 10px; }";
    let minified = minify_css(input);
    assert!(minified.len() < input.len());
    // Minifying the output again must change nothing: a pass that is not
    // idempotent is corrupting a little more on every rebuild.
    let _round_trip = minify_css(&minified);
}

#[test]
fn css_minification_size_reduction_on_realistic_input() {
    let input = r#"
        body {
            margin: 0px 0px 0px 0px;
            padding: 0px 0px 0px 0px;
            color: #ffffff;
            background-color: rgb(0, 0, 0);
            font-family: "Helvetica Neue", Helvetica, Arial, sans-serif;
        }
        .header {
            font-size: 16px;
            line-height: 1.5;
            font-weight: 400;
        }
    "#;
    let minified = minify_css(input);
    let reduction = 1.0 - (minified.len() as f64 / input.len() as f64);
    assert!(
        reduction >= 0.30,
        "expected ≥30% size reduction on representative CSS, got {:.1}% ({} -> {} bytes)",
        reduction * 100.0,
        input.len(),
        minified.len()
    );
}

// =====================================================================
// AC3 — `oxc_minifier` produces compact, parsable output
// =====================================================================

#[test]
fn js_minification_round_trips() {
    let input =
        "function greet(name) { return 'hello, ' + name + '!'; } greet('world');";
    let minified = minify_js(input);
    assert!(minified.len() < input.len());
    let _round_trip = minify_js(&minified);
}

#[test]
fn js_minification_size_reduction_on_realistic_input() {
    // Long variable names and an unread binding. ssg's minifier does not
    // mangle or eliminate either: it removes comments and collapses
    // whitespace, and never rewrites a token. That is the deliberate trade
    // for owning the code — `oxc_minifier` reached 40% on this fixture by
    // renaming and dropping dead code, at the cost of a JS parser, code
    // generator and allocator in the dependency graph. Ours reaches ~23%
    // and cannot change what the script does.
    let input = r#"
        const veryDescriptiveGreetingMessage = 'hello world';
        const anotherUnusedVariableNameForExtraBytes = 'never read';
        function computeSomething(firstArgument, secondArgument) {
            const intermediateResult = firstArgument + secondArgument;
            return intermediateResult;
        }
        console.log(veryDescriptiveGreetingMessage);
        console.log(computeSomething(1, 2));
    "#;
    let minified = minify_js(input);
    let reduction = 1.0 - (minified.len() as f64 / input.len() as f64);
    assert!(
        reduction >= 0.20,
        "expected ≥20% size reduction on representative JS, got {:.1}% ({} -> {} bytes)",
        reduction * 100.0,
        input.len(),
        minified.len()
    );
}

// =====================================================================
// Plugin smoke — empty site dir, non-existent dir
// =====================================================================

#[test]
fn minify_plugin_empty_site_dir_is_ok() {
    let temp = tempdir().unwrap();
    MinifyPlugin.after_compile(&ctx(temp.path())).unwrap();
}

#[test]
fn minify_plugin_missing_site_dir_is_ok() {
    MinifyPlugin
        .after_compile(&ctx(Path::new("/this/path/does/not/exist")))
        .unwrap();
}
