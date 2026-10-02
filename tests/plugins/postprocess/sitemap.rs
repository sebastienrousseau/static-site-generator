// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::plugin::Plugin;
use ssg::postprocess::SitemapFixPlugin;

#[test]
fn sitemap_fix_plugin_name_is_stable() {
    assert!(!SitemapFixPlugin.name().is_empty(), "should not be empty");
}
