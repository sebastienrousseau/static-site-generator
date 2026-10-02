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
