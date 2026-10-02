// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::livereload`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::livereload::{css_reload_message, LiveReloadPlugin};
use ssg::plugin::Plugin;

#[test]
fn livereload_plugin_default_uses_known_port() {
    let p = LiveReloadPlugin::new();
    assert_eq!(p.port(), 35729);
}

#[test]
fn livereload_plugin_with_port_overrides_default() {
    let p = LiveReloadPlugin::with_port(9999);
    assert_eq!(p.port(), 9999);
}

#[test]
fn livereload_plugin_default_trait_matches_new() {
    let p = LiveReloadPlugin::default();
    assert_eq!(p.port(), 35729);
}

#[test]
fn livereload_plugin_name_is_stable() {
    assert!(
        !LiveReloadPlugin::new().name().is_empty(),
        "should not be empty"
    );
}

#[test]
fn css_reload_message_contains_path() {
    let msg = css_reload_message("css/theme.css");
    assert!(msg.contains("css/theme.css") || msg.contains("theme.css"));
}

#[test]
fn css_reload_message_is_nonempty_for_root_path() {
    let msg = css_reload_message("style.css");
    assert!(!msg.is_empty(), "should not be empty");
}
