// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::CspPlugin`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::csp::CspPlugin;
use ssg::plugin::Plugin;

#[test]
fn csp_plugin_name_is_stable() {
    assert!(!CspPlugin.name().is_empty(), "should not be empty");
}
