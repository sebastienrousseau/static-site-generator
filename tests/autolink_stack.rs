// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Regression test for GHSA-xg9p-p4jc-c46g through ssg's own
//! Markdown path.
//!
//! comrak 0.54 and earlier recursed once per bare email autolink in a
//! text node, so a page of a few thousand addresses overflowed the
//! stack. ssg reaches comrak through `staticdatagen` (its own comrak,
//! and `html-generator` -> `mdx-gen` -> comrak), so the advisory is
//! ssg's whenever those crates resolve to an affected comrak.
//!
//! A stack overflow aborts the process instead of panicking, which is
//! why this test has a binary of its own: on a regression the abort
//! takes down only this file's run, and `cargo test` reports it as a
//! SIGABRT rather than a failed assertion.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

/// Bare email addresses in one paragraph. Enough that one stack frame
/// per autolink cannot fit in [`STACK_BYTES`].
const EMAILS: usize = 3_000;

/// Small enough that recursion per autolink cannot fit, large enough
/// for comrak's ordinary work and for compiling a one-page site.
const STACK_BYTES: usize = 512 * 1024;

/// One paragraph of [`EMAILS`] distinct bare addresses.
fn emails() -> String {
    (0..EMAILS).fold(String::new(), |mut out, i| {
        let _ = write!(out, "user{i}@example.com ");
        out
    })
}

/// Compiles one page whose body is `body` with [`ssg::compile_site`]
/// and returns the HTML it wrote for that page.
fn compile_page(body: &str) -> String {
    let tmp = tempfile::tempdir().expect("tempdir");
    let [content, build, site, templates] =
        ["content", "build", "site", "templates"].map(|d| {
            let path = tmp.path().join(d);
            fs::create_dir_all(&path).expect("create a site directory");
            path
        });
    fs::write(
        content.join("page.md"),
        format!(
            "---\ntitle: Addresses\nlayout: page\n\
             description: Bare addresses\n\
             permalink: https://example.com/page\n---\n{body}\n"
        ),
    )
    .expect("write the page");
    fs::write(
        templates.join("page.html"),
        "<html><body>{{content}}</body></html>",
    )
    .expect("write the layout");

    ssg::compile_site(&build, &content, &site, &templates)
        .expect("the page compiles");
    fs::read_to_string(page_html(&site)).expect("read the page")
}

/// The `index.html` the page compiled to, searched for under `dir`.
fn page_html(dir: &Path) -> PathBuf {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("read a site directory") {
            let path = entry.expect("read a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.ends_with("page/index.html") {
                return path;
            }
        }
    }
    panic!("no page/index.html under {}", dir.display())
}

/// Every bare address in a page is rendered as a link, on a thread
/// whose stack could not hold one frame per address.
#[test]
fn bare_email_autolinks_compile_on_a_small_stack() {
    let html = std::thread::Builder::new()
        .name("autolink-512k".to_owned())
        .stack_size(STACK_BYTES)
        .spawn(|| compile_page(&emails()))
        .expect("spawn the compile thread")
        .join()
        .expect("the compile thread panicked");
    assert_eq!(html.matches("mailto:user").count(), EMAILS);
}
