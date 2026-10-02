// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Vector-search artifact emitter (issue #545).
//!
//! Walks the compiled site for HTML files, runs the `ssg-search`
//! encoder over each page's visible text, and writes the four-file
//! WASM-loadable bundle to `<site>/search/`:
//!
//!   - `embeddings.bin`  — pre-normalised f32 vectors, little-endian.
//!   - `manifest.json`   — row index → `{url, title, excerpt}` map.
//!   - `model.bin`       — encoder weights / config (with magic header).
//!   - `tokenizer.bin`   — tokeniser config (with magic header).
//!
//! Intended to run **alongside** [`crate::search::SearchPlugin`]
//! (which is a separate keystroke-based lexical search). The two
//! coexist — `search-index.json` powers the modal autocomplete,
//! `search/embeddings.bin` powers the semantic ranked results
//! returned by the WASM engine.
//!
//! ## Why a separate plugin
//!
//! The vector bundle is large (one f32 per dim per doc) so it ships
//! to the browser separately and lazily — never inlined into HTML —
//! and the worker fetches it the first time the user types. The
//! existing `SearchPlugin` is on every page; this plugin is opt-in
//! via [`crate::plugin::PluginManager::register`].

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::search::SearchIndex;
use ssg_search::{
    paths::{EMBEDDINGS_FILE, MANIFEST_FILE, MODEL_FILE, TOKENIZER_FILE},
    ArtifactsBuilder,
};
use std::fs;

/// Short excerpt length in characters — fits a single search-result
/// snippet line in typical layouts.
const EXCERPT_CHARS: usize = 160;

/// Plugin that builds the `<site>/search/` vector bundle.
///
/// # Examples
///
/// ```
/// use ssg::plugin::Plugin;
/// use ssg::search_index::VectorSearchPlugin;
/// assert_eq!(VectorSearchPlugin.name(), "vector-search");
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct VectorSearchPlugin;

impl Plugin for VectorSearchPlugin {
    fn name(&self) -> &'static str {
        "vector-search"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }
        if ctx.dry_run {
            return Ok(());
        }

        // Re-use SearchIndex's HTML-walking logic so we get title +
        // body text via lol_html (same filters that exclude
        // <script>/<style>/<nav>/<footer>/<head>).
        let index = SearchIndex::build(&ctx.site_dir)?;
        if index.is_empty() {
            return Ok(());
        }

        let mut builder = ArtifactsBuilder::default();
        for e in &index.entries {
            let excerpt = truncate_chars(&e.content, EXCERPT_CHARS);
            let _ = builder.add_doc(ssg_search::artifacts::InputDoc {
                url: e.url.clone(),
                title: e.title.clone(),
                body: format!("{}\n{}", e.title, e.content),
                excerpt,
            });
        }

        let artifacts = builder.build();

        // Materialise the four files under <site>/search/.
        let dir = ctx.site_dir.join("search");
        fs::create_dir_all(&dir).with_path(&dir)?;

        let emb_path = dir.join(EMBEDDINGS_FILE);
        fs::write(&emb_path, &artifacts.embeddings).with_path(&emb_path)?;

        let man_path = dir.join(MANIFEST_FILE);
        let manifest_json = stamp_embeddings_hash(
            &artifacts.manifest_json,
            &artifacts.embeddings,
        );
        fs::write(&man_path, manifest_json).with_path(&man_path)?;

        let model_path = dir.join(MODEL_FILE);
        fs::write(&model_path, &artifacts.model).with_path(&model_path)?;

        let tok_path = dir.join(TOKENIZER_FILE);
        fs::write(&tok_path, &artifacts.tokenizer).with_path(&tok_path)?;

        log::info!(
            "[vector-search] Wrote {} docs ({} bytes embeddings) to {}",
            artifacts.count(),
            artifacts.embeddings.len(),
            dir.display()
        );
        Ok(())
    }
}

/// Adds an `embeddings_sha256` field to the manifest JSON so the
/// `search_index` audit gate can verify `embeddings.bin` integrity
/// (the gate reads `manifest.json#embeddings_sha256` and compares it
/// to the SHA-256 of the binary — see
/// `src/audit/gates/search_index.rs`). Returns the manifest verbatim
/// if it fails to parse (defensive; `ssg-search` always emits valid
/// JSON).
fn stamp_embeddings_hash(manifest_json: &[u8], embeddings: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let Ok(mut manifest) =
        serde_json::from_slice::<serde_json::Value>(manifest_json)
    else {
        return manifest_json.to_vec();
    };
    let Some(obj) = manifest.as_object_mut() else {
        return manifest_json.to_vec();
    };

    let mut hasher = Sha256::new();
    hasher.update(embeddings);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Infallible on String; ignore the Result per fmt::Write docs.
        let _ = write!(hex, "{byte:02x}");
    }

    let _ = obj.insert(
        "embeddings_sha256".to_string(),
        serde_json::Value::String(hex),
    );
    serialize_manifest_value(&manifest)
        .unwrap_or_else(|_| manifest_json.to_vec())
}

/// Serialize the stamped manifest with a fault-injection hook so tests
/// can drive the defensive fallback in [`stamp_embeddings_hash`]
/// (pretty-printing a `Value` parsed from `ssg-search`'s own valid
/// JSON output, plus one inserted hex-string field, cannot fail in
/// practice).
fn serialize_manifest_value(
    manifest: &serde_json::Value,
) -> serde_json::Result<Vec<u8>> {
    fail_point!("search_index::manifest-serialize", |_| Err(
        <serde_json::Error as serde::ser::Error>::custom(
            "injected: search_index::manifest-serialize"
        )
    ));
    serde_json::to_vec_pretty(manifest)
}

/// Returns the first `max_chars` Unicode scalar values of `s`. (Plain
/// `s[..max]` slicing on bytes would split multibyte sequences.)
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    fn ctx(p: &Path) -> PluginContext {
        PluginContext::new(p, p, p, p)
    }

    fn write_html(dir: &Path, name: &str, body: &str) {
        let p = dir.join(name);
        fs::write(
            &p,
            format!(
                "<!DOCTYPE html><html><head><title>{name}</title></head><body>{body}</body></html>"
            ),
        )
        .unwrap();
    }

    #[test]
    fn plugin_name() {
        assert_eq!(VectorSearchPlugin.name(), "vector-search");
    }

    #[test]
    fn plugin_is_noop_on_missing_dir() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("not-here");
        let c = ctx(&missing);
        VectorSearchPlugin.after_compile(&c).unwrap();
        assert!(!missing.exists());
    }

    #[test]
    fn plugin_is_noop_in_dry_run() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "page.html", "<p>hello</p>");
        let c = ctx(tmp.path()).with_dry_run(true);
        VectorSearchPlugin.after_compile(&c).unwrap();
        assert!(!tmp.path().join("search").exists());
    }

    #[test]
    fn plugin_is_noop_on_empty_corpus() {
        let tmp = tempdir().unwrap();
        let c = ctx(tmp.path());
        VectorSearchPlugin.after_compile(&c).unwrap();
        // No HTML → SearchIndex empty → no search/ dir is created.
        assert!(!tmp.path().join("search").exists());
    }

    #[test]
    fn plugin_emits_four_artifacts() {
        let tmp = tempdir().unwrap();
        write_html(
            tmp.path(),
            "a.html",
            "<p>rust webassembly compiles to portable browser modules</p>",
        );
        write_html(
            tmp.path(),
            "b.html",
            "<p>baking sourdough bread starter flour</p>",
        );
        let c = ctx(tmp.path());
        VectorSearchPlugin.after_compile(&c).unwrap();

        let dir = tmp.path().join("search");
        assert!(dir.exists());
        assert!(dir.join("embeddings.bin").exists());
        assert!(dir.join("manifest.json").exists());
        assert!(dir.join("model.bin").exists());
        assert!(dir.join("tokenizer.bin").exists());
    }

    #[test]
    fn embeddings_size_is_n_x_d_x_4() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "a.html", "<p>foo bar baz</p>");
        write_html(tmp.path(), "b.html", "<p>quux corge grault</p>");
        let c = ctx(tmp.path());
        VectorSearchPlugin.after_compile(&c).unwrap();
        let emb = fs::read(tmp.path().join("search/embeddings.bin")).unwrap();
        assert_eq!(emb.len(), 2 * 256 * 4);
    }

    #[test]
    fn manifest_has_one_entry_per_html() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "x.html", "<p>alpha</p>");
        write_html(tmp.path(), "y.html", "<p>beta</p>");
        let c = ctx(tmp.path());
        VectorSearchPlugin.after_compile(&c).unwrap();
        let json = fs::read_to_string(tmp.path().join("search/manifest.json"))
            .unwrap();
        let m: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(m["count"].as_u64().unwrap(), 2);
        assert_eq!(m["entries"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn manifest_carries_embeddings_sha256_matching_bin() {
        use sha2::{Digest, Sha256};
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "a.html", "<p>hash me</p>");
        let c = ctx(tmp.path());
        VectorSearchPlugin.after_compile(&c).unwrap();

        let emb = fs::read(tmp.path().join("search/embeddings.bin")).unwrap();
        let mut h = Sha256::new();
        h.update(&emb);
        let expected =
            h.finalize()
                .iter()
                .fold(String::with_capacity(64), |mut s, b| {
                    use std::fmt::Write;
                    let _ = write!(s, "{b:02x}");
                    s
                });

        let json = fs::read_to_string(tmp.path().join("search/manifest.json"))
            .unwrap();
        let m: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(m["embeddings_sha256"].as_str().unwrap(), expected);
    }

    #[test]
    fn stamp_embeddings_hash_passes_through_invalid_json() {
        let raw = b"not json".to_vec();
        assert_eq!(stamp_embeddings_hash(&raw, b"x"), raw);
        let arr = b"[1,2]".to_vec();
        assert_eq!(stamp_embeddings_hash(&arr, b"x"), arr);
    }

    #[test]
    fn truncate_chars_respects_unicode_boundaries() {
        let s = "résumé café";
        let t = truncate_chars(s, 6);
        // 6 chars: "résumé" (no panic on multi-byte char)
        assert_eq!(t, "résumé");
    }

    #[test]
    fn truncate_chars_returns_full_string_when_short() {
        let s = "short";
        assert_eq!(truncate_chars(s, 100), "short");
    }

    #[test]
    fn embeddings_byte_identical_across_runs() {
        let tmp = tempdir().unwrap();
        write_html(
            tmp.path(),
            "a.html",
            "<p>identical content produces identical embeddings</p>",
        );
        let c = ctx(tmp.path());
        VectorSearchPlugin.after_compile(&c).unwrap();
        let first = fs::read(tmp.path().join("search/embeddings.bin")).unwrap();
        // Rebuild
        VectorSearchPlugin.after_compile(&c).unwrap();
        let second =
            fs::read(tmp.path().join("search/embeddings.bin")).unwrap();
        assert_eq!(first, second);
    }

    // -------------------------------------------------------------------
    // IO error branches
    // -------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn after_compile_fails_when_site_has_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "a.html", "<p>hello</p>");
        let locked = tmp.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let res = VectorSearchPlugin.after_compile(&ctx(tmp.path()));

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        // Root CI runners bypass perms; only assert when it errored.
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }

    #[test]
    #[serial_test::parallel]
    fn after_compile_fails_when_search_dir_squatted_by_file() {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "a.html", "<p>hello</p>");
        fs::write(tmp.path().join("search"), "not a dir").unwrap();
        let err = VectorSearchPlugin
            .after_compile(&ctx(tmp.path()))
            .unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    /// Squats `search/<name>` with a directory so the corresponding
    /// `fs::write` fails.
    fn assert_write_fails_when_squatted(name: &str) {
        let tmp = tempdir().unwrap();
        write_html(tmp.path(), "a.html", "<p>hello</p>");
        fs::create_dir_all(tmp.path().join("search").join(name)).unwrap();
        let err = VectorSearchPlugin
            .after_compile(&ctx(tmp.path()))
            .unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    fn after_compile_fails_when_embeddings_squatted_by_dir() {
        assert_write_fails_when_squatted(EMBEDDINGS_FILE);
    }

    #[test]
    fn after_compile_fails_when_manifest_squatted_by_dir() {
        assert_write_fails_when_squatted(MANIFEST_FILE);
    }

    #[test]
    fn after_compile_fails_when_model_squatted_by_dir() {
        assert_write_fails_when_squatted(MODEL_FILE);
    }

    #[test]
    fn after_compile_fails_when_tokenizer_squatted_by_dir() {
        assert_write_fails_when_squatted(TOKENIZER_FILE);
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use serial_test::serial;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial]
    fn stamp_embeddings_hash_falls_back_on_injected_serialize_failure() {
        // Drives the defensive `unwrap_or_else` fallback that otherwise
        // cannot be reached: re-serializing a manifest `Value` parsed
        // from `ssg-search`'s own output cannot fail in practice.
        let _guard = FailGuard("search_index::manifest-serialize");
        fail::cfg("search_index::manifest-serialize", "return")
            .expect("activate failpoint");

        let manifest_json = br#"{"count":0,"entries":[]}"#;
        let out = stamp_embeddings_hash(manifest_json, b"embeddings");
        assert_eq!(
            out, manifest_json,
            "injected failure must fall back to the original manifest bytes"
        );
    }
}
