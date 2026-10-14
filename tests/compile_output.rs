// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a compiled page carries from staticdatagen's front matter and
//! meta tag handling.
//!
//! ssg compiles Markdown through `staticdatagen`, which reads front
//! matter and renders meta tags with `metadata-gen`. staticdatagen
//! 0.0.22 (metadata-gen 0.0.8) changed four things a page shows; these
//! tests pin them at ssg's own entry point, [`ssg::compile_site`]:
//!
//! - Open Graph tags carry `property=`, as the protocol specifies, not
//!   `name=`;
//! - `&`, `<` and `>` in a meta tag value are escaped;
//! - JSON front matter is no longer rendered as the page's first
//!   paragraph;
//! - YAML front matter after a UTF-8 byte order mark is recognised
//!   instead of failing the build.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// A layout that shows the primary and Open Graph meta tags.
const LAYOUT: &str = "<html><head><title>{{title}}</title>{{!primary}}\
                      {{!opengraph}}</head><body>{{content}}</body></html>";

/// Compiles one page, `<name>.md` holding `source`, and returns the
/// HTML written for it, or the build error as text.
fn compile_page(name: &str, source: &str) -> Result<String, String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let [content, build, site, templates] =
        ["content", "build", "site", "templates"].map(|d| {
            let path = tmp.path().join(d);
            fs::create_dir_all(&path).expect("create a site directory");
            path
        });
    fs::write(content.join(format!("{name}.md")), source)
        .expect("write the page");
    fs::write(templates.join("page.html"), LAYOUT).expect("write the layout");

    ssg::compile_site(&build, &content, &site, &templates)
        .map_err(|e| format!("{e:#}"))?;
    let page = find_page(&site, name).expect("the page is written");
    Ok(fs::read_to_string(page).expect("read the page"))
}

/// The `<name>/index.html` under `dir`, searched for recursively.
fn find_page(dir: &Path, name: &str) -> Option<PathBuf> {
    let want = Path::new(name).join("index.html");
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.ends_with(&want) {
                return Some(path);
            }
        }
    }
    None
}

/// A YAML page with Open Graph keys and markup characters in its
/// values.
const OG_PAGE: &str = "---\ntitle: Fish\nlayout: page\n\
                       description: Fish & Chips <b>\n\
                       og:title: Fish & Chips\nog:type: article\n\
                       permalink: https://example.com/og\n---\nBody.\n";

#[test]
fn open_graph_tags_use_the_property_attribute() {
    let html = compile_page("og", OG_PAGE).expect("the page compiles");
    assert!(html.contains(r#"<meta property="og:type" content="article">"#));
    assert!(html.contains(r#"<meta property="og:title""#));
    assert!(!html.contains(r#"name="og:"#), "{html}");
}

#[test]
fn meta_tag_values_escape_markup() {
    let html = compile_page("og", OG_PAGE).expect("the page compiles");
    assert!(
        html.contains(
            r#"<meta name="description" content="Fish &amp; Chips &lt;b&gt;">"#
        ),
        "{html}"
    );
    assert!(html.contains(r#"content="Fish &amp; Chips">"#), "{html}");
}

#[test]
fn json_front_matter_is_not_rendered_as_body_text() {
    let html = compile_page(
        "json",
        "{\"title\": \"Data\", \"layout\": \"page\", \
         \"description\": \"json\"}\nBody of json.\n",
    )
    .expect("the page compiles");
    assert!(html.contains("<title>Data</title>"), "{html}");
    assert!(html.contains("<p>Body of json.</p>"), "{html}");
    assert!(!html.contains("&quot;layout&quot;"), "{html}");
}

#[test]
fn front_matter_after_a_byte_order_mark_compiles() {
    let html = compile_page(
        "bom",
        "\u{feff}---\ntitle: Marked\nlayout: page\n\
         description: bom\n---\nBody of bom.\n",
    )
    .expect("front matter after a BOM is recognised");
    assert!(html.contains("<title>Marked</title>"), "{html}");
    assert!(html.contains("<p>Body of bom.</p>"), "{html}");
}
