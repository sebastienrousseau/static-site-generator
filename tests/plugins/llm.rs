// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::LlmPlugin`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::llm::{LlmConfig, LlmPlugin};
use ssg::plugin::Plugin;

#[test]
fn llm_plugin_constructs_with_default_config() {
    let p = LlmPlugin::new(LlmConfig::default());
    assert!(!p.name().is_empty(), "should not be empty");
}
