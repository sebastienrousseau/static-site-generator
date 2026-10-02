// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::plugin::Plugin;
use ssg::seo::{validate_jsonld, JsonLdConfig, JsonLdPlugin};

#[test]
fn jsonld_plugin_constructs_with_explicit_config() {
    let p = JsonLdPlugin::new(JsonLdConfig {
        base_url: "https://example.com".into(),
        org_name: "Example Org".into(),
        breadcrumbs: false,
    });
    assert!(!p.name().is_empty(), "should not be empty");
}

#[test]
fn validate_jsonld_returns_empty_for_no_script() {
    let errors = validate_jsonld("<html><body></body></html>");
    assert!(errors.is_empty(), "{:?}", errors);
}

#[test]
fn validate_jsonld_flags_malformed_json() {
    let bad = r#"<html><script type="application/ld+json">{ not json }</script></html>"#;
    let errors = validate_jsonld(bad);
    assert!(!errors.is_empty(), "should not be empty");
}
