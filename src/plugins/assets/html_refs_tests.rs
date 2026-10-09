// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Regression tests for #798: fingerprinted references are rewritten per
//! element, and only `<script src>` and `<link href>` gain SRI attributes.

use super::{rewrite_asset_refs, AssetInfo};
use std::collections::HashMap;

fn manifest() -> HashMap<String, AssetInfo> {
    [
        ("style.css", "style.abc12345.css"),
        ("css/style.css", "css/style.5eed5eed.css"),
        ("app.js", "app.0badf00d.js"),
        ("images/logo.png", "images/logo.c0ffee00.png"),
    ]
    .into_iter()
    .map(|(name, fp)| {
        (
            name.to_string(),
            AssetInfo {
                fingerprinted: fp.to_string(),
                sri: format!("sha384-{name}"),
            },
        )
    })
    .collect()
}

fn rewrite(html: &str) -> String {
    rewrite_asset_refs(html, &manifest()).expect("rewrite succeeds")
}

/// The body of the first `<script type="application/ld+json">` block.
fn json_ld(html: &str) -> serde_json::Value {
    let body = html
        .split(r#"<script type="application/ld+json">"#)
        .nth(1)
        .and_then(|s| s.split("</script>").next())
        .expect("ld+json block");
    serde_json::from_str(body).expect("JSON-LD still parses")
}

#[test]
fn json_ld_meta_and_img_are_renamed_without_sri() {
    let out = rewrite(concat!(
        r#"<meta property="og:image" content="https://example.com/images/logo.png">"#,
        r#"<img src="/images/logo.png" alt="">"#,
        r#"<script type="application/ld+json">"#,
        r#"{"logo": "https://example.com/images/logo.png", "url": "/app.js"}"#,
        "</script>",
    ));
    assert!(
        out.contains(
            r#"content="https://example.com/images/logo.c0ffee00.png">"#
        ),
        "{out}"
    );
    assert!(
        out.contains(r#"<img src="/images/logo.c0ffee00.png" alt="">"#),
        "{out}"
    );
    let ld = json_ld(&out);
    assert_eq!(ld["logo"], "https://example.com/images/logo.c0ffee00.png");
    assert_eq!(ld["url"], "/app.0badf00d.js");
    assert!(!out.contains("integrity"), "{out}");
}

#[test]
fn stylesheet_and_script_src_gain_sri() {
    let out = rewrite(concat!(
        r#"<link rel="stylesheet" href="/style.css">"#,
        r#"<script src="/app.js" defer></script>"#,
    ));
    assert!(out.contains(r#"href="/style.abc12345.css""#), "{out}");
    assert!(out.contains(r#"integrity="sha384-style.css""#), "{out}");
    assert!(out.contains(r#"src="/app.0badf00d.js""#), "{out}");
    assert!(out.contains(r#"integrity="sha384-app.js""#), "{out}");
    assert_eq!(
        out.matches(r#"crossorigin="anonymous""#).count(),
        2,
        "{out}"
    );
}

#[test]
fn other_attributes_of_a_script_tag_gain_no_sri() {
    let out = rewrite(
        r#"<script src="/app.js" data-poster="/images/logo.png"></script>"#,
    );
    assert!(
        out.contains(r#"data-poster="/images/logo.c0ffee00.png""#),
        "{out}"
    );
    assert_eq!(out.matches("integrity=").count(), 1, "{out}");
    assert!(!out.contains("sha384-images/logo.png"), "{out}");
}

#[test]
fn icon_link_is_renamed_without_sri() {
    let out = rewrite(r#"<link rel="icon" href="/images/logo.png">"#);
    assert_eq!(out, r#"<link rel="icon" href="/images/logo.c0ffee00.png">"#);
}

#[test]
fn preload_and_modulepreload_gain_sri() {
    let out = rewrite(concat!(
        r#"<link rel="preload" as="style" href="/style.css">"#,
        r#"<link rel="modulepreload" href="/app.js">"#,
    ));
    assert_eq!(out.matches("integrity=").count(), 2, "{out}");
}

#[test]
fn authored_integrity_is_replaced_not_duplicated() {
    let out = rewrite(
        r#"<script src="/app.js" integrity="sha384-stale" crossorigin="use-credentials"></script>"#,
    );
    assert_eq!(out.matches("integrity=").count(), 1, "{out}");
    assert!(out.contains(r#"integrity="sha384-app.js""#), "{out}");
    assert!(out.contains(r#"crossorigin="use-credentials""#), "{out}");
}

#[test]
fn longest_matching_path_wins() {
    for _ in 0..16 {
        let out = rewrite(r#"<link rel="stylesheet" href="/css/style.css">"#);
        assert!(out.contains(r#"href="/css/style.5eed5eed.css""#), "{out}");
        assert!(out.contains(r#"integrity="sha384-css/style.css""#), "{out}");
    }
}

#[test]
fn single_quoted_and_query_string_references_are_renamed() {
    let out = rewrite("<script src='/app.js?v=2'></script>");
    assert!(out.contains("/app.0badf00d.js?v=2"), "{out}");
    assert!(out.contains(r#"integrity="sha384-app.js""#), "{out}");
}

#[test]
fn inline_script_and_style_bodies_gain_no_attributes() {
    let html = concat!(
        "<script>import(\"/app.js\"); const s = 'x /style.css y';</script>",
        "<style>.a { background: url(\"/images/logo.png\") }</style>",
    );
    let out = rewrite(html);
    assert!(out.contains("import(\"/app.0badf00d.js\")"), "{out}");
    assert!(out.contains("'x /style.css y'"), "{out}");
    assert!(out.contains("url(\"/images/logo.c0ffee00.png\")"), "{out}");
    assert!(!out.contains("integrity"), "{out}");
}

#[test]
fn unreferenced_markup_is_byte_identical() {
    let html = "<!doctype html><p class=x>\"/style.css\" &amp; text</p><!-- <link href=\"/style.css\"> -->";
    assert_eq!(rewrite(html), html);
}
