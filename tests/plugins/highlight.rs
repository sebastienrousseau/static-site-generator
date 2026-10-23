// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::HighlightPlugin`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::highlight::HighlightPlugin;
use ssg::plugin::Plugin;

#[test]
fn highlight_plugin_name_is_stable() {
    let p = HighlightPlugin::new("base16-ocean.dark");
    assert!(!p.name().is_empty(), "should not be empty");
}

/// Returns the `href` of every stylesheet `<link>` in `html`.
fn stylesheet_hrefs(html: &str) -> Vec<String> {
    html.split("<link ")
        .skip(1)
        .filter(|tag| tag.contains("rel=\"stylesheet\""))
        .filter_map(|tag| tag.split("href=\"").nth(1))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect()
}

/// The highlighting stylesheet a page links is the file the build wrote.
///
/// Regression for #790: every `after_compile` hook runs before any
/// `transform_html`, so `FingerprintPlugin` had already renamed
/// `highlight.css` to `highlight.<hash>.css` by the time the link was
/// injected, and pages pointed at a file that did not exist.
#[test]
fn highlighted_page_links_the_published_stylesheet() {
    use ssg::cmd::SsgConfig;
    use ssg::pipeline::register_default_plugins;
    use ssg::plugin::{PluginContext, PluginManager};
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let site = dir.path().join("site");
    fs::create_dir_all(&site).unwrap();
    let head = "<!DOCTYPE html><html lang=\"en\"><head><title>T</title></head>";
    fs::write(
        site.join("index.html"),
        format!(
            "{head}<body><pre><code class=\"language-go\">package main\
             </code></pre></body></html>"
        ),
    )
    .unwrap();
    fs::write(
        site.join("plain.html"),
        format!("{head}<body><p>No code here.</p></body></html>"),
    )
    .unwrap();

    let mut plugins = PluginManager::new();
    register_default_plugins(&mut plugins, &SsgConfig::default(), false, None);
    let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
    plugins.run_after_compile(&ctx).unwrap();
    plugins.run_fused_transforms(&ctx).unwrap();

    let page = fs::read_to_string(site.join("index.html")).unwrap();
    let hrefs: Vec<_> = stylesheet_hrefs(&page)
        .into_iter()
        .filter(|h| h.starts_with("/highlight"))
        .collect();
    assert_eq!(hrefs.len(), 1, "one highlight stylesheet link: {page}");
    let href = &hrefs[0];
    assert!(
        site.join(href.trim_start_matches('/')).is_file(),
        "{href} is linked but not written: {page}"
    );
    assert_ne!(href, "/highlight.css", "link is not fingerprinted");
    let tag = page
        .split("<link ")
        .find(|t| t.contains(href.as_str()))
        .unwrap();
    assert!(tag.contains("integrity=\"sha384-"), "no SRI: {tag}");

    let plain = fs::read_to_string(site.join("plain.html")).unwrap();
    assert!(!plain.contains("highlight"), "plain page links it: {plain}");
}
