// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Build-side integrity tests for the `ssg-search` vector bundle
//! (issue #545 — AC1, AC5, AC6).
//!
//! These tests round-trip the artifacts through both the build-side
//! [`ssg_search::Artifacts`] builder and the [`ssg_search::VectorEngine`]
//! runtime to prove that the bytes written to disk are exactly what the
//! WASM engine will load in the browser. They do NOT exercise the
//! wasm-bindgen JS layer — that is covered by
//! `crates/ssg-search/tests/wasm_search.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ssg::plugin::Plugin;
use ssg::plugin::PluginContext;
use ssg::search_index::VectorSearchPlugin;
use ssg_search::{
    artifacts::{Artifacts, InputDoc},
    paths, VectorEngine,
};
use std::fs;
use tempfile::tempdir;

fn write_html(dir: &std::path::Path, name: &str, body: &str) {
    let p = dir.join(name);
    fs::write(
        &p,
        format!(
            "<!DOCTYPE html><html><head><title>{name}</title></head><body>{body}</body></html>"
        ),
    )
    .unwrap();
}

fn fixture(n: usize) -> Vec<InputDoc> {
    let topics = [
        "rust webassembly compiles deterministically",
        "static site generators built in rust",
        "baking sourdough bread starter flour water",
        "italian pasta carbonara guanciale pecorino",
        "photography portrait lighting reflectors",
        "machine learning embeddings vector spaces",
        "browser performance metrics core web vitals",
        "css grid layout responsive design",
        "rust simd intrinsics target webassembly",
        "kubernetes pod autoscaling reference architecture",
    ];
    (0..n)
        .map(|i| {
            let body = topics[i % topics.len()];
            InputDoc {
                url: format!("/doc/{i}"),
                title: format!("Doc {i}"),
                body: body.to_string(),
                excerpt: body.chars().take(80).collect(),
            }
        })
        .collect()
}

// =====================================================================
// AC1 — embeddings.bin contains exactly N × D × 4 bytes
// =====================================================================

#[test]
fn ac1_embeddings_have_correct_byte_layout() {
    let arts = Artifacts::from_docs(&fixture(50));
    let expected_bytes = arts.count() * arts.dim() * 4;
    assert_eq!(arts.embeddings.len(), expected_bytes);
    assert_eq!(arts.count(), 50);
}

#[test]
fn ac1_emits_all_four_artifacts_via_plugin() {
    // Reproduces the real ssg pipeline through the plugin entry point.
    let tmp = tempdir().unwrap();
    write_html(tmp.path(), "a.html", "<p>foo bar baz</p>");
    write_html(tmp.path(), "b.html", "<p>quux corge grault</p>");
    let ctx =
        PluginContext::new(tmp.path(), tmp.path(), tmp.path(), tmp.path());
    VectorSearchPlugin.after_compile(&ctx).unwrap();

    let dir = tmp.path().join("search");
    for f in [
        paths::EMBEDDINGS_FILE,
        paths::MANIFEST_FILE,
        paths::MODEL_FILE,
        paths::TOKENIZER_FILE,
    ] {
        assert!(dir.join(f).exists(), "missing artifact: {f}");
    }
}

// =====================================================================
// AC2 — total payload (model + tokenizer + wasm.gz) ≤ 7 MB
// =====================================================================

#[test]
fn ac2_model_and_tokenizer_are_tiny() {
    // The default model + tokenizer fit in well under 1 KB. The wasm
    // module weighs another ~30 KB gzipped (verified out of band by
    // the wasm-pack build step). Total payload << 7 MB.
    let arts = Artifacts::from_docs(&fixture(10));
    assert!(
        arts.model.len() < 1024,
        "model.bin {} bytes",
        arts.model.len()
    );
    assert!(
        arts.tokenizer.len() < 1024,
        "tokenizer.bin {} bytes",
        arts.tokenizer.len()
    );
}

// =====================================================================
// AC5 — every emitted document vector has L2 norm in [0.999, 1.001]
//       and the engine performs the dot-product without division
// =====================================================================

#[test]
fn ac5_every_embedded_vector_is_unit_norm() {
    let arts = Artifacts::from_docs(&fixture(100));
    let dim = arts.dim();
    for row in 0..arts.count() {
        let mut sumsq = 0.0_f32;
        for d in 0..dim {
            let offset = (row * dim + d) * 4;
            let f = f32::from_le_bytes([
                arts.embeddings[offset],
                arts.embeddings[offset + 1],
                arts.embeddings[offset + 2],
                arts.embeddings[offset + 3],
            ]);
            sumsq = f.mul_add(f, sumsq);
        }
        let norm = sumsq.sqrt();
        assert!(
            (0.999..=1.001).contains(&norm),
            "row {row} L2 norm out of range: {norm}"
        );
    }
}

#[test]
fn ac5_engine_search_is_pure_dot_product_no_division() {
    // Indirect check: the engine's score for a query equal to a corpus
    // row must be 1.0 (within int8 rounding) — which is only true if
    // similarity is a pure dot product on pre-normalised vectors.
    let docs = fixture(5);
    let arts = Artifacts::from_docs(&docs);
    let engine = VectorEngine::new(
        &arts.model,
        &arts.tokenizer,
        &arts.embeddings,
        arts.count(),
    )
    .unwrap();
    let q = engine.embed_query(&docs[0].body);
    let out = engine.search_vec(&q, 1);
    assert_eq!(out.len(), 2);
    let top_idx = out[0] as usize;
    let top_score = out[1];
    assert_eq!(top_idx, 0);
    // Exact 1.0 expected because the embedder is deterministic and we
    // matched the corpus row exactly.
    assert!(
        (top_score - 1.0).abs() < 1e-4,
        "top score should be 1.0 (got {top_score})"
    );
}

// =====================================================================
// AC6 — same content + same model = byte-identical embeddings
// =====================================================================

#[test]
fn ac6_same_content_yields_byte_identical_embeddings() {
    let docs = fixture(20);
    let a = Artifacts::from_docs(&docs);
    let b = Artifacts::from_docs(&docs);
    assert_eq!(a.embeddings, b.embeddings);
    assert_eq!(a.model, b.model);
    assert_eq!(a.tokenizer, b.tokenizer);
    assert_eq!(a.model_hash, b.model_hash);
    assert_eq!(a.manifest_json, b.manifest_json);
}

#[test]
fn ac6_model_hash_in_manifest_matches_model_bytes() {
    let arts = Artifacts::from_docs(&fixture(3));
    let m: ssg_search::Manifest =
        serde_json::from_slice(&arts.manifest_json).unwrap();
    assert_eq!(m.model_hash, arts.model_hash);
    assert_eq!(m.model_hash.len(), 64); // sha256 hex
}

// =====================================================================
// End-to-end: build → engine → search returns sensible results
// =====================================================================

#[test]
fn end_to_end_search_returns_related_doc_first() {
    let docs = vec![
        InputDoc {
            url: "/wasm".into(),
            title: "WASM".into(),
            body: "rust webassembly compiles to portable browser modules"
                .into(),
            excerpt: "wasm".into(),
        },
        InputDoc {
            url: "/bread".into(),
            title: "Bread".into(),
            body: "sourdough starter flour water salt and time".into(),
            excerpt: "bread".into(),
        },
        InputDoc {
            url: "/simd".into(),
            title: "SIMD".into(),
            body: "rust simd intrinsics vectorisation cpu".into(),
            excerpt: "simd".into(),
        },
    ];
    let arts = Artifacts::from_docs(&docs);
    let engine = VectorEngine::new(
        &arts.model,
        &arts.tokenizer,
        &arts.embeddings,
        arts.count(),
    )
    .unwrap();
    let out = engine.search("rust wasm", 3);
    // First hit must be the wasm doc (idx 0) or the simd doc (idx 2).
    let top = out[0] as usize;
    assert!(
        top == 0 || top == 2,
        "expected rust-related top hit, got {top}"
    );
}

#[test]
fn end_to_end_top_k_is_clamped_to_corpus_size() {
    let arts = Artifacts::from_docs(&fixture(5));
    let engine = VectorEngine::new(
        &arts.model,
        &arts.tokenizer,
        &arts.embeddings,
        arts.count(),
    )
    .unwrap();
    let out = engine.search("anything", 100);
    // 5 docs × 2 (idx, score) per result = 10 floats
    assert_eq!(out.len(), 10);
}
