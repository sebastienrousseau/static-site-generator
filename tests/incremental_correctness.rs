// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Issue #524 — `ssg build --incremental` correctness.
//!
//! Covers the seven acceptance criteria stated on the issue. Each
//! test runs against a self-contained fixture under a tempdir so the
//! suite stays hermetic and parallel-safe.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ssg::depgraph::{self, DepGraph};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn write(p: &Path, body: &str) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    content: PathBuf,
    template: PathBuf,
    build: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let content = tmp.path().join("content");
        let template = tmp.path().join("templates");
        let build = tmp.path().join("public");
        let cache = tmp.path().join(".ssg-cache");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&template).unwrap();
        fs::create_dir_all(&build).unwrap();
        Self {
            _tmp: tmp,
            content,
            template,
            build,
            cache,
        }
    }
}

// ---------------------------------------------------------------------------
// AC1 — content → page edge is recorded and persisted
// ---------------------------------------------------------------------------

#[test]
fn ac1_content_to_page_edge_is_recorded_and_persisted() {
    let f = Fixture::new();
    write(
        &f.content.join("blog").join("foo.md"),
        "---\nlayout: \"post\"\n---\n# Foo",
    );
    write(&f.template.join("post.html"), "<html>{{title}}</html>");

    let mut graph = DepGraph::new();
    depgraph::populate(&mut graph, &f.content, &f.template, &f.build).unwrap();
    graph.save(&f.cache).unwrap();

    let foo_md = f.content.join("blog").join("foo.md");
    let expected_out = f.build.join("blog").join("foo").join("index.html");
    let outs = graph.outputs_for(&foo_md).expect("output recorded");
    assert!(
        outs.contains(&expected_out),
        "content → output edge missing: {outs:?}"
    );

    // Persisted file exists at the canonical path.
    assert!(f.cache.join(depgraph::DEP_GRAPH_FILE).exists());

    let reloaded = DepGraph::load(&f.cache);
    assert!(reloaded
        .outputs_for(&foo_md)
        .unwrap()
        .contains(&expected_out));
}

// ---------------------------------------------------------------------------
// AC2 — template → page edge is recorded
// ---------------------------------------------------------------------------

#[test]
fn ac2_template_to_page_edge_is_recorded() {
    let f = Fixture::new();
    write(
        &f.content.join("a.md"),
        "---\nlayout: \"post\"\n---\nbody a",
    );
    write(
        &f.content.join("b.md"),
        "---\nlayout: \"post\"\n---\nbody b",
    );
    write(&f.template.join("post.html"), "<html>{{title}}</html>");

    let mut graph = DepGraph::new();
    depgraph::populate(&mut graph, &f.content, &f.template, &f.build).unwrap();

    let post_tpl = f.template.join("post.html");
    let invalidated = graph.invalidated(std::slice::from_ref(&post_tpl));

    let a_md = f.content.join("a.md");
    let b_md = f.content.join("b.md");
    assert!(invalidated.contains(&a_md));
    assert!(invalidated.contains(&b_md));
    assert!(invalidated.contains(&post_tpl));
}

// ---------------------------------------------------------------------------
// AC3 — transitive template edges are tracked (flips the prior assertion)
// ---------------------------------------------------------------------------

#[test]
fn ac3_transitive_template_edges_invalidate_consumers() {
    let f = Fixture::new();
    // base.html ← layout.html ← post.html ← content
    write(&f.template.join("base.html"), "<html>{{body}}</html>");
    write(
        &f.template.join("layout.html"),
        "{{#extends \"base\"}}<body>{{slot}}</body>",
    );
    write(
        &f.template.join("post.html"),
        "{{#extends \"layout\"}}<article>{{title}}</article>",
    );
    write(&f.content.join("a.md"), "---\nlayout: \"post\"\n---\n");
    write(&f.content.join("b.md"), "---\nlayout: \"post\"\n---\n");

    let mut graph = DepGraph::new();
    depgraph::populate(&mut graph, &f.content, &f.template, &f.build).unwrap();

    let base = f.template.join("base.html");
    let invalidated = graph.invalidated(std::slice::from_ref(&base));

    let a_md = f.content.join("a.md");
    let b_md = f.content.join("b.md");
    assert!(
        invalidated.contains(&a_md),
        "transitive base.html → page edge missing (a): {invalidated:?}"
    );
    assert!(
        invalidated.contains(&b_md),
        "transitive base.html → page edge missing (b): {invalidated:?}"
    );
}

// ---------------------------------------------------------------------------
// AC4 — incremental warm cache short-circuits the rebuild
// ---------------------------------------------------------------------------

#[test]
fn ac4_warm_cache_diff_is_empty_when_nothing_changed() {
    let f = Fixture::new();
    write(
        &f.content.join("post.md"),
        "---\nlayout: \"post\"\n---\nbody",
    );
    write(&f.template.join("post.html"), "<html>{{title}}</html>");

    let mut graph = DepGraph::new();
    depgraph::populate(&mut graph, &f.content, &f.template, &f.build).unwrap();
    graph.save(&f.cache).unwrap();

    let warm = DepGraph::load(&f.cache);

    let t0 = Instant::now();
    let current = depgraph::current_hashes(&f.content, &f.template).unwrap();
    let diff = warm.diff(&current);
    let elapsed = t0.elapsed();

    assert!(diff.is_empty(), "warm cache must report no diff");
    assert!(
        elapsed < std::time::Duration::from_millis(200),
        "warm cache diff must be <200ms, was {elapsed:?}"
    );
}

#[test]
fn ac4_single_content_edit_invalidates_only_that_page() {
    let f = Fixture::new();
    write(
        &f.content.join("a.md"),
        "---\nlayout: \"post\"\n---\nbody-a",
    );
    write(
        &f.content.join("b.md"),
        "---\nlayout: \"post\"\n---\nbody-b",
    );
    write(&f.template.join("post.html"), "<html>{{title}}</html>");

    let mut graph = DepGraph::new();
    depgraph::populate(&mut graph, &f.content, &f.template, &f.build).unwrap();

    // Simulate editing a.md only.
    write(
        &f.content.join("a.md"),
        "---\nlayout: \"post\"\n---\nbody-a-EDITED",
    );

    let current = depgraph::current_hashes(&f.content, &f.template).unwrap();
    let diff = graph.diff(&current);

    let a_md = f.content.join("a.md");
    assert_eq!(diff.changed, vec![a_md.clone()]);
    assert!(diff.deleted.is_empty(), "{:?}", diff.deleted);

    let invalidated = graph.invalidated_outputs(&diff.changed);
    let expected_out = f.build.join("a").join("index.html");
    assert_eq!(invalidated, vec![expected_out]);
    // b.md must NOT be in the invalidated set.
    let b_out = f.build.join("b").join("index.html");
    assert!(!invalidated.contains(&b_out));
}

// ---------------------------------------------------------------------------
// AC5 — delete sweep
// ---------------------------------------------------------------------------

#[test]
fn ac5_deleted_source_is_reported_in_diff() {
    let f = Fixture::new();
    write(&f.content.join("foo.md"), "---\nlayout: \"post\"\n---\n");
    write(&f.template.join("post.html"), "<html></html>");

    let mut graph = DepGraph::new();
    depgraph::populate(&mut graph, &f.content, &f.template, &f.build).unwrap();

    // Delete the source.
    fs::remove_file(f.content.join("foo.md")).unwrap();

    let current = depgraph::current_hashes(&f.content, &f.template).unwrap();
    let diff = graph.diff(&current);

    let foo_md = f.content.join("foo.md");
    assert!(diff.deleted.contains(&foo_md));

    let stale_outputs = graph.invalidated_outputs(&diff.deleted);
    let expected_out = f.build.join("foo").join("index.html");
    assert!(stale_outputs.contains(&expected_out));
}

// ---------------------------------------------------------------------------
// AC6 — cache poisoning resistance
// ---------------------------------------------------------------------------

#[test]
fn ac6_corrupted_cache_falls_back_to_empty_graph() {
    let f = Fixture::new();
    fs::create_dir_all(&f.cache).unwrap();
    fs::write(
        f.cache.join(depgraph::DEP_GRAPH_FILE),
        "{{{ this is not valid json",
    )
    .unwrap();

    let graph = DepGraph::load(&f.cache);
    assert_eq!(
        graph.page_count(),
        0,
        "corrupted cache must not panic and must yield an empty graph"
    );
}

#[test]
fn ac6_truncated_cache_falls_back_to_empty_graph() {
    let f = Fixture::new();
    fs::create_dir_all(&f.cache).unwrap();
    fs::write(f.cache.join(depgraph::DEP_GRAPH_FILE), "{").unwrap();

    let graph = DepGraph::load(&f.cache);
    assert_eq!(graph.page_count(), 0);
}

#[test]
fn ac6_wrong_schema_version_falls_back_to_empty_graph() {
    let f = Fixture::new();
    fs::create_dir_all(&f.cache).unwrap();
    // Schema version 999 is in the future and unparseable by current ssg.
    fs::write(
        f.cache.join(depgraph::DEP_GRAPH_FILE),
        r#"{"version":999,"deps":{},"outputs":{},"hashes":{}}"#,
    )
    .unwrap();

    let graph = DepGraph::load(&f.cache);
    assert_eq!(graph.page_count(), 0);
}

// ---------------------------------------------------------------------------
// AC7 — Plugin::needs_all_files() defaults to true
// ---------------------------------------------------------------------------

#[test]
fn ac7_plugin_needs_all_files_defaults_to_true() {
    use ssg::error::SsgError;
    use ssg::plugin::{Plugin, PluginContext};

    #[derive(Debug)]
    struct NoopPlugin;
    impl Plugin for NoopPlugin {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn before_compile(&self, _: &PluginContext) -> Result<(), SsgError> {
            Ok(())
        }
    }

    assert!(
        NoopPlugin.needs_all_files(),
        "default impl must return true so SEO/SBOM/search keep working"
    );
}

#[test]
fn ac7_per_file_plugins_can_opt_out() {
    use ssg::error::SsgError;
    use ssg::plugin::{Plugin, PluginContext};

    #[derive(Debug)]
    struct PerFile;
    impl Plugin for PerFile {
        fn name(&self) -> &'static str {
            "per-file"
        }
        fn before_compile(&self, _: &PluginContext) -> Result<(), SsgError> {
            Ok(())
        }
        fn needs_all_files(&self) -> bool {
            false
        }
    }

    assert!(!PerFile.needs_all_files());
}

// ---------------------------------------------------------------------------
// End-to-end: warm-cache rebuild is fast
// ---------------------------------------------------------------------------

#[test]
fn warm_cache_path_round_trip_is_sub_200ms() {
    let f = Fixture::new();
    // 100 pages — keeps the test snappy but still exercises the
    // sha256 + walk code paths under realistic load.
    for i in 0..100 {
        write(
            &f.content.join(format!("p-{i}.md")),
            &format!(
                "---\nlayout: \"post\"\ntitle: \"Page {i}\"\n---\n# Page {i}"
            ),
        );
    }
    write(&f.template.join("post.html"), "<html>{{title}}</html>");

    let mut graph = DepGraph::new();
    depgraph::populate(&mut graph, &f.content, &f.template, &f.build).unwrap();
    graph.save(&f.cache).unwrap();

    let t0 = Instant::now();
    let warm = DepGraph::load(&f.cache);
    let current = depgraph::current_hashes(&f.content, &f.template).unwrap();
    let diff = warm.diff(&current);
    let elapsed = t0.elapsed();

    assert!(diff.is_empty());
    assert!(
        elapsed < std::time::Duration::from_millis(200),
        "warm cache hot-path took {elapsed:?}, expected <200ms"
    );
}
