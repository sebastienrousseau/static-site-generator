// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::PaginationPlugin`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::pagination::PaginationPlugin;
use ssg::plugin::Plugin;

#[test]
fn pagination_plugin_default_constructs() {
    let p = PaginationPlugin::default();
    assert!(!p.name().is_empty(), "should not be empty");
}
