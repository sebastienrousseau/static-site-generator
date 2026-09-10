// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::pipeline`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::path::PathBuf;

use ssg::cmd::SsgConfig;
use ssg::pipeline::{clear_error_message, resolve_build_and_site_dirs};

fn minimal_config() -> SsgConfig {
    SsgConfig::builder()
        .site_name("x".into())
        .base_url("https://example.com".into())
        .site_title("y".into())
        .site_description("z".into())
        .language("en-US".into())
        .content_dir(PathBuf::from("content"))
        .output_dir(PathBuf::from("public"))
        .template_dir(PathBuf::from("templates"))
        .build()
        .expect("config")
}

#[test]
fn clear_error_message_returns_a_string() {
    let _ = clear_error_message();
}

#[test]
fn resolve_build_and_site_dirs_returns_two_paths() {
    let cfg = minimal_config();
    let (build, site) = resolve_build_and_site_dirs(&cfg);
    assert!(!build.as_os_str().is_empty());
    assert!(!site.as_os_str().is_empty());
}

// ---------------------------------------------------------------------
// Permalink derivation through the compile pipeline
// (spec A2/B1, plan §2 item 1.2, issue #586)
// ---------------------------------------------------------------------

/// Recursively lists all files under `dir` (test diagnostics).
fn list_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&cur) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Plan §2 item 1.2 acceptance: a 3-page fixture where ZERO pages carry
/// `permalink:` front matter must build once the site's `base_url` is
/// threaded through `compile_site_with_base_url` — the content stager
/// injects `permalink = derive_permalink(base_url, source_path)` so
/// `rss-gen`'s "channel.link is missing" hard-fail is unreachable, and
/// the compiled feed links agree with the canonical-`<link>` convention
/// (both sides derive through `urls::derive_page_url`).
#[test]
fn compile_with_base_url_derives_permalinks_and_feed_links_agree() {
    use std::fs;

    let tmp = tempfile::tempdir().expect("tempdir");
    let content = tmp.path().join("content");
    let build = tmp.path().join("build");
    let site = tmp.path().join("public");
    let templates = tmp.path().join("templates");
    fs::create_dir_all(&content).expect("mkdir content");
    fs::create_dir_all(&templates).expect("mkdir templates");
    fs::create_dir_all(&build).expect("mkdir build");

    // Zero pages declare `permalink` (or `url`).
    fs::write(
        content.join("index.md"),
        "---\ntitle: \"Home\"\ndescription: \"home\"\n---\nhome body",
    )
    .expect("write index.md");
    fs::write(
        content.join("about.md"),
        "---\ntitle: \"About\"\ndescription: \"about\"\n---\nabout body",
    )
    .expect("write about.md");
    fs::write(
        content.join("contact.md"),
        "---\ntitle: \"Contact\"\ndescription: \"c\"\n---\ncontact body",
    )
    .expect("write contact.md");

    fs::write(
        templates.join("page.html"),
        "<!doctype html><html><body>{{ content }}</body></html>",
    )
    .expect("write template");

    let base_url = "https://example.com";
    ssg::pipeline::compile_site_with_base_url(
        &build,
        &content,
        &site,
        &templates,
        Some(base_url),
    )
    .expect("compile must succeed with derived permalinks");

    // The compiled pages exist under the pretty-URL convention.
    for slug in ["about", "contact"] {
        let out = site.join(slug).join("index.html");
        assert!(
            out.exists(),
            "expected {} to exist; site: {:?}",
            out.display(),
            list_files(&site)
        );
    }

    // The derived permalinks are exactly the shared-code-path URLs …
    let about_url = ssg::urls::derive_permalink(base_url, "about.md");
    let contact_url = ssg::urls::derive_permalink(base_url, "contact.md");
    assert_eq!(about_url, "https://example.com/about/");
    assert_eq!(contact_url, "https://example.com/contact/");

    // … and they agree with the canonical-<link> convention for the
    // same outputs (both derive through urls::derive_page_url).
    assert_eq!(
        about_url,
        ssg::urls::derive_page_url(base_url, "about/index.html")
    );
    assert_eq!(
        contact_url,
        ssg::urls::derive_page_url(base_url, "contact/index.html")
    );

    // The compiled feed carries the derived permalinks — proof the
    // staged `permalink:` injection reached staticdatagen's RSS
    // writer.
    let about_feed = site.join("about").join("rss.xml");
    assert!(
        about_feed.exists(),
        "expected per-page feed at {}; site: {:?}",
        about_feed.display(),
        list_files(&site)
    );
    let feed = fs::read_to_string(&about_feed).expect("read about feed");
    assert!(
        feed.contains(&about_url),
        "feed <link> should equal the derived permalink {about_url}: {feed}"
    );
}

/// Without a base URL nothing can be derived: the legacy
/// `compile_site` entry point stays permalink-free and pages that
/// already declare `permalink` keep working (author values win).
#[test]
fn compile_without_base_url_keeps_author_permalinks_verbatim() {
    use std::fs;

    let tmp = tempfile::tempdir().expect("tempdir");
    let content = tmp.path().join("content");
    let build = tmp.path().join("build");
    let site = tmp.path().join("public");
    let templates = tmp.path().join("templates");
    fs::create_dir_all(&content).expect("mkdir content");
    fs::create_dir_all(&templates).expect("mkdir templates");
    fs::create_dir_all(&build).expect("mkdir build");

    fs::write(
        content.join("page.md"),
        "---\ntitle: \"P\"\ndescription: \"d\"\n\
         permalink: \"https://example.invalid/custom-spot/\"\n---\nbody",
    )
    .expect("write page.md");
    fs::write(
        templates.join("page.html"),
        "<!doctype html><html><body>{{ content }}</body></html>",
    )
    .expect("write template");

    ssg::pipeline::compile_site(&build, &content, &site, &templates)
        .expect("compile must succeed");

    let feed = site.join("page").join("rss.xml");
    assert!(
        feed.exists(),
        "expected feed at {}; site: {:?}",
        feed.display(),
        list_files(&site)
    );
    let feed = fs::read_to_string(&feed).expect("read feed");
    assert!(
        feed.contains("https://example.invalid/custom-spot/"),
        "author permalink must pass through verbatim: {feed}"
    );
}

/// A project with content but no templates used to fail with
/// `I/O error at 'public.build-tmp': ... No such file or directory` —
/// naming the directory being written *to*, and no path for the file
/// that was actually absent. `examples/basic` shipped in exactly that
/// state, so following it verbatim produced an undiagnosable error.
///
/// The build must now name the missing templates and say how to get
/// them.
#[test]
fn a_missing_root_template_is_named_rather_than_reported_as_io_at_the_output_dir(
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let content = root.join("content");
    let templates = root.join("templates");
    std::fs::create_dir_all(&content).expect("content dir");
    // The MiniJinja set alone is not enough: the StaticWeaver stage runs
    // first and reads the template directory root.
    std::fs::create_dir_all(templates.join("tera")).expect("tera dir");
    std::fs::write(content.join("index.md"), "---\ntitle: T\n---\n\nBody\n")
        .expect("write content");

    let err = ssg::pipeline::compile_site(
        &root.join("public.build-tmp"),
        &content,
        &root.join("public"),
        &templates,
    )
    .expect_err("a project with no root templates must not build");

    let msg = err.to_string();
    for name in ["template.html", "index.html", "page.html", "post.html"] {
        assert!(
            msg.contains(name),
            "the error should name the missing {name}, got: {msg}"
        );
    }
    assert!(
        msg.contains("ssg --new"),
        "the error should say how to get the templates, got: {msg}"
    );
    assert!(
        !msg.contains("public.build-tmp"),
        "the error should not point at the output directory, got: {msg}"
    );
}
