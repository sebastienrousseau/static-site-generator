// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::DraftPlugin`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::drafts::DraftPlugin;
use ssg::plugin::Plugin;

#[test]
fn drafts_plugin_name_is_stable() {
    assert!(
        !DraftPlugin::new(false).name().is_empty(),
        "should not be empty"
    );
}

#[test]
fn drafts_plugin_with_include_flag_constructs() {
    let _ = DraftPlugin::new(true);
}
