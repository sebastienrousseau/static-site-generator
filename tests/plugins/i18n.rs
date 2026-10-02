// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::i18n`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use ssg::i18n::{
    negotiate_locale, parse_accept_language, I18nConfig, I18nPlugin,
};
use ssg::plugin::Plugin;

#[test]
fn i18n_plugin_constructs_with_default_config() {
    let p = I18nPlugin::new(I18nConfig::default());
    assert!(!p.name().is_empty(), "should not be empty");
}

#[test]
fn parse_accept_language_extracts_ordered_locales() {
    let langs = parse_accept_language("en-GB,fr;q=0.9,de;q=0.8");
    assert!(!langs.is_empty(), "should not be empty");
    assert_eq!(langs[0], "en-GB");
}

#[test]
fn parse_accept_language_returns_empty_for_empty_header() {
    let langs = parse_accept_language("");
    assert!(langs.is_empty(), "{:?}", langs);
}

#[test]
fn negotiate_locale_picks_available_match() {
    let chosen = negotiate_locale(
        &["fr-FR".to_string(), "en-US".to_string()],
        &["en-US".to_string(), "fr-FR".to_string()],
        "en-US",
    );
    assert!(chosen == "fr-FR" || chosen == "en-US");
}

#[test]
fn negotiate_locale_falls_back_to_default() {
    let chosen = negotiate_locale(
        &["zh-CN".to_string()],
        &["en-US".to_string(), "fr-FR".to_string()],
        "en-US",
    );
    assert_eq!(chosen, "en-US");
}
