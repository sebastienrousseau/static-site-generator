// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::og_image`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::og_image::{generate_og_svg, OgImagePlugin};
use ssg::plugin::Plugin;

#[test]
fn og_image_plugin_name_is_stable() {
    let p = OgImagePlugin::new("https://example.com");
    assert!(!p.name().is_empty(), "should not be empty");
}

#[test]
fn generate_og_svg_produces_valid_svg_root() {
    let svg = generate_og_svg("Hello", "Site", "#000", "#fff");
    assert!(svg.contains("<svg"));
    assert!(svg.contains("</svg>"));
}

#[test]
fn generate_og_svg_escapes_user_content() {
    let svg = generate_og_svg("<script>", "Site", "#000", "#fff");
    assert!(!svg.contains("<script>"));
    assert!(svg.contains("&lt;script&gt;") || svg.contains("&amp;"));
}
