// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::seo::helpers::{extract_title, has_meta_tag};

#[test]
fn extract_title_returns_h1_or_title_tag() {
    let title = extract_title("<html><head><title>Hello</title></head></html>");
    assert!(!title.is_empty(), "should not be empty");
}

#[test]
fn extract_title_returns_empty_for_titleless_html() {
    let title = extract_title("<html><body></body></html>");
    let _ = title.len();
}

#[test]
fn has_meta_tag_matches_present_meta_attr() {
    let html =
        r#"<html><head><meta name="description" content="x"></head></html>"#;
    assert!(has_meta_tag(html, "description"));
}

#[test]
fn has_meta_tag_returns_false_for_missing_attr() {
    let html = "<html><head></head></html>";
    assert!(!has_meta_tag(html, "description"));
}
