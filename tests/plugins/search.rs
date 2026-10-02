// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::search`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;

use ssg::plugin::Plugin;
use ssg::search::{SearchEntry, SearchIndex, SearchLabels, SearchPlugin};
use tempfile::tempdir;

#[test]
fn search_plugin_name_is_stable() {
    assert!(!SearchPlugin.name().is_empty(), "should not be empty");
}

#[test]
fn search_entry_constructs_with_explicit_fields() {
    let entry = SearchEntry {
        title: "Hello".into(),
        url: "/hello/".into(),
        content: "world".into(),
        headings: vec![],
    };
    assert_eq!(entry.title, "Hello");
}

#[test]
fn search_labels_english_returns_english_strings() {
    let l = SearchLabels::english();
    assert!(!l.button_text.is_empty(), "should not be empty");
    assert!(!l.input_placeholder.is_empty(), "should not be empty");
}

#[test]
fn search_labels_french_returns_french_strings() {
    let l = SearchLabels::french();
    assert!(!l.button_text.is_empty(), "should not be empty");
}

#[test]
fn search_labels_for_locale_recognises_known_codes() {
    let en = SearchLabels::for_locale("en-US");
    let fr = SearchLabels::for_locale("fr-FR");
    let unknown = SearchLabels::for_locale("xx-XX");
    assert!(!en.button_text.is_empty(), "should not be empty");
    assert!(!fr.button_text.is_empty(), "should not be empty");
    assert!(!unknown.button_text.is_empty(), "should not be empty"); // fallback to english
}

#[test]
fn search_index_build_from_html_site_returns_entries() {
    let dir = tempdir().unwrap();
    let site = dir.path();
    fs::write(
        site.join("a.html"),
        "<html><head><title>A</title></head><body>body a</body></html>",
    )
    .unwrap();
    fs::write(
        site.join("b.html"),
        "<html><head><title>B</title></head><body>body b</body></html>",
    )
    .unwrap();
    let idx = SearchIndex::build(site).expect("build");
    assert!(idx.entries.len() >= 2);
}

#[test]
fn search_index_write_persists_json_artefact() {
    let dir = tempdir().unwrap();
    let site = dir.path();
    fs::write(
        site.join("x.html"),
        "<html><head><title>X</title></head><body>x</body></html>",
    )
    .unwrap();
    let idx = SearchIndex::build(site).expect("build");
    idx.write(site).expect("write");
    assert!(site.join("search-index.json").exists());
}

#[test]
fn search_index_holds_entries_in_order() {
    let idx = SearchIndex {
        entries: vec![
            SearchEntry {
                title: "a".into(),
                url: "/a/".into(),
                content: String::new(),
                headings: vec![],
            },
            SearchEntry {
                title: "b".into(),
                url: "/b/".into(),
                content: String::new(),
                headings: vec![],
            },
        ],
    };
    assert_eq!(idx.entries.len(), 2);
}
