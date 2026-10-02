// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::sbom::SbomPlugin` (top-level
//! `CycloneDX` SBOM emitter).

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::plugin::Plugin;
use ssg::sbom::SbomPlugin;

#[test]
fn sbom_plugin_name_is_stable() {
    assert!(!SbomPlugin.name().is_empty(), "should not be empty");
}
