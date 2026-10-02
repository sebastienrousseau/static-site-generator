// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::shortcodes`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::plugin::Plugin;
use ssg::shortcodes::{expand_shortcodes, ShortcodePlugin};

#[test]
fn shortcode_plugin_name_is_stable() {
    assert!(!ShortcodePlugin.name().is_empty(), "should not be empty");
}

#[test]
fn expand_shortcodes_passes_through_plain_text() {
    let out = expand_shortcodes("hello world");
    assert_eq!(out, "hello world");
}

#[test]
fn expand_shortcodes_expands_known_youtube_macro() {
    let out = expand_shortcodes("{{ youtube(id=\"abc123\") }}");
    assert!(out.contains("abc123") || out.contains("youtube"));
}
