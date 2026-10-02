// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::FingerprintPlugin`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::assets::FingerprintPlugin;
use ssg::plugin::Plugin;

#[test]
fn fingerprint_plugin_name_is_stable() {
    assert!(!FingerprintPlugin.name().is_empty(), "should not be empty");
}
