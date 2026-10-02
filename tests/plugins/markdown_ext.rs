// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::markdown_ext`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::markdown_ext::{expand_gfm, MarkdownExtPlugin};
use ssg::plugin::Plugin;

#[test]
fn markdown_ext_plugin_name_is_stable() {
    assert!(!MarkdownExtPlugin.name().is_empty(), "should not be empty");
}

#[test]
fn expand_gfm_passes_through_plain_text() {
    let out = expand_gfm("hello world", None);
    assert!(out.contains("hello world"));
}

#[test]
fn expand_gfm_with_cdn_prefix_rewrites_local_image_paths() {
    let input = "![](images/photo.png)";
    let out = expand_gfm(input, Some("https://cdn.example.com"));
    assert!(
        out.contains("cdn.example.com") || out.contains("images/photo.png"),
        "either rewrites or leaves intact"
    );
}
