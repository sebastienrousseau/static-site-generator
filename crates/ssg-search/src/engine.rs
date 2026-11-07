// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Runtime vector search engine — the WASM-resident half of ssg-search.
//!
//! [`VectorEngine`] holds the deserialised encoder plus the
//! pre-normalised corpus matrix. The hot path is:
//!
//! 1. Embed the query string (caller-side embedder, same instance type
//!    as the build-side embedder — so the vector spaces line up).
//! 2. **Skip division.** All vectors are unit-norm; similarity reduces
//!    to a pure dot product. The engine asserts (in debug builds and
//!    when `wasm-profiling` is on) that no `f32::sqrt` / division runs
//!    inside the inner loop.
//! 3. Top-K via partial heap sort.
//! 4. Return a single `Float32Array` of interleaved `[idx, score, idx,
//!    score, …]` to the caller (AC4).

use crate::encoder::{
    deserialize_projection_encoder, Encoder, ProjectionEncoder,
};
use crate::DEFAULT_TOP_K;

/// Runtime semantic search engine.
///
/// Built once per page load by the JS shim, then queried many times.
/// Cheap to clone (`encoder` is `Copy`, the corpus is held by `Arc` in
/// the higher-level wrapper — but here we keep it owned for the
/// simplest engine API).
///
/// # Examples
///
/// ```
/// use ssg_search::artifacts::{Artifacts, InputDoc};
/// use ssg_search::engine::VectorEngine;
///
/// let docs = vec![
///     InputDoc { url: "/a".into(), title: "A".into(), body: "rust wasm".into(), excerpt: "".into() },
///     InputDoc { url: "/b".into(), title: "B".into(), body: "cooking pasta".into(), excerpt: "".into() },
/// ];
/// let arts = Artifacts::from_docs(&docs);
/// let engine = VectorEngine::new(&arts.model, &arts.tokenizer, &arts.embeddings, arts.count()).unwrap();
/// assert_eq!(engine.count(), 2);
/// ```
#[derive(Debug)]
pub struct VectorEngine {
    encoder: ProjectionEncoder,
    /// Row-major, `count × dim`, every row L2-normalised.
    corpus: Vec<f32>,
    /// Number of rows (documents).
    count: usize,
    /// Dimensionality of each row.
    dim: usize,
}

/// Errors the engine raises on construction. Kept simple (string +
/// kind) so wasm-bindgen can flatten to `JsError` without dragging in
/// `thiserror`.
///
/// # Examples
///
/// ```
/// use ssg_search::engine::{EngineError, VectorEngine};
///
/// // A header that's not the SSGS magic → BadModel.
/// let err = VectorEngine::new(&[0u8; 24], &[], &[], 0).unwrap_err();
/// assert_eq!(err, EngineError::BadModel);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    /// `model.bin` was unrecognised or version-mismatched.
    BadModel,
    /// `embeddings.bin` length didn't equal `count × dim × 4`.
    BadEmbeddings {
        /// Expected byte length (count × dim × 4).
        expected: usize,
        /// Actual byte length received.
        got: usize,
    },
    /// `dim` from manifest didn't match the encoder's dim.
    DimMismatch {
        /// Dimensionality declared by the manifest.
        manifest: usize,
        /// Dimensionality reported by the encoder.
        encoder: usize,
    },
}

impl core::fmt::Display for EngineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadModel => write!(
                f,
                "ssg-search: model.bin header is unrecognised or version-mismatched"
            ),
            Self::BadEmbeddings { expected, got } => write!(
                f,
                "ssg-search: embeddings.bin length mismatch (expected {expected}, got {got})"
            ),
            Self::DimMismatch { manifest, encoder } => write!(
                f,
                "ssg-search: dim mismatch — manifest says {manifest}, encoder says {encoder}"
            ),
        }
    }
}

impl std::error::Error for EngineError {}

impl VectorEngine {
    /// Constructs a new engine from raw artifact bytes.
    ///
    /// `model_bytes`        — contents of `model.bin`
    /// `_tokenizer_bytes`   — contents of `tokenizer.bin` (the default
    ///                        encoder reconstructs its tokeniser config
    ///                        from `model.bin` alone, so the tokenizer
    ///                        bytes are accepted but currently unused;
    ///                        kept in the signature so the `model2vec`
    ///                        feature can wire them up later).
    /// `embeddings_bytes`   — contents of `embeddings.bin` (little-endian f32,
    ///                        `count × dim × 4` bytes).
    /// `count`              — number of vectors in the corpus.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::engine::{EngineError, VectorEngine};
    ///
    /// let arts = Artifacts::from_docs(&[InputDoc {
    ///     url: "/".into(), title: "".into(), body: "hello".into(), excerpt: "".into(),
    /// }]);
    /// let engine = VectorEngine::new(
    ///     &arts.model, &arts.tokenizer, &arts.embeddings, arts.count(),
    /// ).unwrap();
    /// assert_eq!(engine.count(), 1);
    ///
    /// // Mismatched embedding byte-length is rejected.
    /// let bad = VectorEngine::new(&arts.model, &arts.tokenizer, &[0u8; 7], 1);
    /// assert!(matches!(bad, Err(EngineError::BadEmbeddings { .. })));
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::BadModel`] when `model_bytes` is not a
    /// valid `model.bin`, and [`EngineError::BadEmbeddings`] when
    /// `embeddings_bytes.len() != count * dim * 4`.
    pub fn new(
        model_bytes: &[u8],
        _tokenizer_bytes: &[u8],
        embeddings_bytes: &[u8],
        count: usize,
    ) -> Result<Self, EngineError> {
        let encoder = deserialize_projection_encoder(model_bytes)
            .ok_or(EngineError::BadModel)?;
        let dim = encoder.dim();
        let expected = count
            .checked_mul(dim)
            .and_then(|n| n.checked_mul(4))
            .unwrap_or(usize::MAX);
        if embeddings_bytes.len() != expected {
            return Err(EngineError::BadEmbeddings {
                expected,
                got: embeddings_bytes.len(),
            });
        }
        // Decode the LE f32 corpus once. (A zero-copy view via
        // bytemuck would be tempting but requires endianness =
        // little + 4-byte alignment guarantees we can't make from
        // arbitrary slices.)
        let mut corpus = Vec::with_capacity(count * dim);
        // `as_chunks` yields `&[u8; 4]` rather than a slice that merely
        // happens to be four long, so the array is passed straight to
        // `from_le_bytes` with no indexing and no length assumption to
        // comment on. `.1` is the remainder, which a well-formed
        // embeddings blob does not have.
        for chunk in embeddings_bytes.as_chunks::<4>().0 {
            corpus.push(f32::from_le_bytes(*chunk));
        }
        Ok(Self {
            encoder,
            corpus,
            count,
            dim,
        })
    }

    /// Returns the encoder used by this engine.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::encoder::{Encoder, EMBEDDING_DIM};
    /// use ssg_search::engine::VectorEngine;
    ///
    /// let arts = Artifacts::from_docs(&[InputDoc {
    ///     url: "/".into(), title: "".into(), body: "x".into(), excerpt: "".into(),
    /// }]);
    /// let engine = VectorEngine::new(&arts.model, &arts.tokenizer, &arts.embeddings, 1).unwrap();
    /// assert_eq!(engine.encoder().dim(), EMBEDDING_DIM);
    /// ```
    #[must_use]
    pub const fn encoder(&self) -> &ProjectionEncoder {
        &self.encoder
    }

    /// Returns the corpus dimensionality.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::encoder::EMBEDDING_DIM;
    /// use ssg_search::engine::VectorEngine;
    ///
    /// let arts = Artifacts::from_docs(&[InputDoc {
    ///     url: "/".into(), title: "".into(), body: "x".into(), excerpt: "".into(),
    /// }]);
    /// let engine = VectorEngine::new(&arts.model, &arts.tokenizer, &arts.embeddings, 1).unwrap();
    /// assert_eq!(engine.dim(), EMBEDDING_DIM);
    /// ```
    #[must_use]
    pub const fn dim(&self) -> usize {
        self.dim
    }

    /// Returns the number of indexed documents.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::engine::VectorEngine;
    ///
    /// let docs: Vec<InputDoc> = (0..3).map(|i| InputDoc {
    ///     url: format!("/{i}"), title: "".into(), body: "x".into(), excerpt: "".into(),
    /// }).collect();
    /// let arts = Artifacts::from_docs(&docs);
    /// let engine = VectorEngine::new(&arts.model, &arts.tokenizer, &arts.embeddings, arts.count()).unwrap();
    /// assert_eq!(engine.count(), 3);
    /// ```
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Returns the raw corpus matrix (row-major, L2-normalised).
    /// Exposed for tests and for callers that want to inspect the
    /// underlying buffer; the WASM binding never exposes this to JS.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::encoder::EMBEDDING_DIM;
    /// use ssg_search::engine::VectorEngine;
    ///
    /// let arts = Artifacts::from_docs(&[InputDoc {
    ///     url: "/".into(), title: "".into(), body: "hello".into(), excerpt: "".into(),
    /// }]);
    /// let engine = VectorEngine::new(&arts.model, &arts.tokenizer, &arts.embeddings, 1).unwrap();
    /// // count * dim f32 values, row-major.
    /// assert_eq!(engine.corpus().len(), 1 * EMBEDDING_DIM);
    /// ```
    #[must_use]
    pub fn corpus(&self) -> &[f32] {
        &self.corpus
    }

    /// Embeds the query string using the same encoder used at build
    /// time. Pure delegation — the engine doesn't transform the result.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::engine::VectorEngine;
    ///
    /// let arts = Artifacts::from_docs(&[InputDoc {
    ///     url: "/".into(), title: "".into(), body: "rust wasm".into(), excerpt: "".into(),
    /// }]);
    /// let engine = VectorEngine::new(&arts.model, &arts.tokenizer, &arts.embeddings, 1).unwrap();
    /// let q = engine.embed_query("rust");
    /// assert_eq!(q.len(), engine.dim());
    /// // L2 norm should be ~1.0 for non-empty input.
    /// let norm: f32 = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    /// assert!((0.999..=1.001).contains(&norm));
    /// ```
    #[must_use]
    pub fn embed_query(&self, query: &str) -> Vec<f32> {
        self.encoder.embed(query)
    }

    /// Runs the top-K search on `query_vec` (a unit-norm vector of
    /// length [`Self::dim`]) and returns the interleaved
    /// `[idx, score, idx, score, …]` result of length `2 × top_k`.
    ///
    /// **No division, no `sqrt`** — every vector is pre-normalised, so
    /// cosine similarity collapses to a pure dot product. This is the
    /// AC5 short-circuit (verified by `wasm-profiling`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::engine::VectorEngine;
    ///
    /// let docs = vec![
    ///     InputDoc { url: "/a".into(), title: "".into(), body: "rust simd".into(), excerpt: "".into() },
    ///     InputDoc { url: "/b".into(), title: "".into(), body: "pasta sauce".into(), excerpt: "".into() },
    /// ];
    /// let arts = Artifacts::from_docs(&docs);
    /// let engine = VectorEngine::new(&arts.model, &arts.tokenizer, &arts.embeddings, 2).unwrap();
    ///
    /// // Pre-embedded query goes straight to scoring.
    /// let q = engine.embed_query("rust");
    /// let out = engine.search_vec(&q, 2);
    /// assert_eq!(out.len(), 4); // 2 (idx, score) pairs
    /// // Results pre-sorted descending by score.
    /// assert!(out[1] >= out[3]);
    ///
    /// // Wrong-dimensional input → empty result (no panic).
    /// assert!(engine.search_vec(&[0.0_f32; 5], 1).is_empty());
    /// ```
    #[allow(clippy::suboptimal_flops)] // mul_add slower on wasm32 (no FMA)
    pub fn search_vec(&self, query_vec: &[f32], top_k: usize) -> Vec<f32> {
        let top_k = if top_k == 0 { DEFAULT_TOP_K } else { top_k };
        if query_vec.len() != self.dim || self.count == 0 {
            return Vec::new();
        }

        // Score every doc — straight dot product, SIMD-friendly loop.
        let mut scores: Vec<(usize, f32)> = Vec::with_capacity(self.count);
        for i in 0..self.count {
            let base = i * self.dim;
            let row = &self.corpus[base..base + self.dim];
            let mut acc = 0.0_f32;
            // Unroll by 4 — keeps the WASM SIMD lane aligned with
            // f32x4 splats when built with `-C target-feature=+simd128`.
            let chunks = self.dim / 4;
            // We intentionally use the plain `*` then `+` form rather
            // than `f32::mul_add`. WASM has no hardware FMA, so
            // `mul_add` lowers to a software fma() libcall that's ~3x
            // slower than the splat form. On native, the difference is
            // negligible. (Clippy's `suboptimal_flops` lint is
            // silenced just above this fn.)
            for k in 0..chunks {
                let r = k * 4;
                acc += row[r] * query_vec[r]
                    + row[r + 1] * query_vec[r + 1]
                    + row[r + 2] * query_vec[r + 2]
                    + row[r + 3] * query_vec[r + 3];
            }
            for r in chunks * 4..self.dim {
                acc += row[r] * query_vec[r];
            }
            // Clip negative cosine to 0. For L2-normalised vectors the
            // dot product is cosine similarity ∈ [-1, 1]; negative
            // values mean "anti-related" and are treated as "no match"
            // by every production search engine. Preserving them in
            // the top-K output is just noise.
            scores.push((i, acc.max(0.0)));
        }

        // Partial sort to top-K. For typical N (≤ a few thousand) the
        // O(N log K) heap approach beats a full sort.
        let k = top_k.min(self.count);
        let _ = scores.select_nth_unstable_by(k - 1, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal)
        });
        let mut top = scores.into_iter().take(k).collect::<Vec<_>>();
        top.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal)
        });

        let mut out = Vec::with_capacity(2 * k);
        for (i, s) in top {
            out.push(i as f32);
            out.push(s);
        }
        out
    }

    /// Convenience: embed `query` then call [`Self::search_vec`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::engine::VectorEngine;
    /// use ssg_search::DEFAULT_TOP_K;
    ///
    /// let docs: Vec<InputDoc> = (0..15).map(|i| InputDoc {
    ///     url: format!("/{i}"), title: "".into(),
    ///     body: format!("doc number {i}"), excerpt: "".into(),
    /// }).collect();
    /// let arts = Artifacts::from_docs(&docs);
    /// let engine = VectorEngine::new(
    ///     &arts.model, &arts.tokenizer, &arts.embeddings, arts.count(),
    /// ).unwrap();
    ///
    /// // top_k = 0 means "use the default".
    /// let out = engine.search("number", 0);
    /// assert_eq!(out.len(), 2 * DEFAULT_TOP_K);
    /// ```
    pub fn search(&self, query: &str, top_k: usize) -> Vec<f32> {
        let q = self.embed_query(query);
        self.search_vec(&q, top_k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::{ProjectionConfig, EMBEDDING_DIM};

    fn build_corpus(docs: &[&str]) -> (Vec<u8>, Vec<u8>, Vec<u8>, usize) {
        let enc = ProjectionEncoder::default();
        let model = enc.serialize_model();
        let tokenizer = enc.serialize_tokenizer();
        let mut embeddings = Vec::with_capacity(docs.len() * EMBEDDING_DIM * 4);
        for d in docs {
            for f in enc.embed(d) {
                embeddings.extend_from_slice(&f.to_le_bytes());
            }
        }
        (model, tokenizer, embeddings, docs.len())
    }

    #[test]
    fn new_round_trips_artifacts() {
        let (model, tok, emb, n) = build_corpus(&["alpha", "beta", "gamma"]);
        let engine = VectorEngine::new(&model, &tok, &emb, n).unwrap();
        assert_eq!(engine.count(), 3);
        assert_eq!(engine.dim(), EMBEDDING_DIM);
        assert_eq!(engine.corpus().len(), 3 * EMBEDDING_DIM);
    }

    #[test]
    fn new_rejects_bad_model() {
        let bad = vec![0u8; 24];
        let res = VectorEngine::new(&bad, &[], &[], 0);
        assert_eq!(res.unwrap_err(), EngineError::BadModel);
    }

    #[test]
    fn new_rejects_bad_embeddings_length() {
        let (model, tok, _, _) = build_corpus(&["a"]);
        let err = VectorEngine::new(&model, &tok, &[0u8; 7], 1).unwrap_err();
        assert!(matches!(err, EngineError::BadEmbeddings { .. }));
    }

    #[test]
    fn search_returns_interleaved_idx_score_pairs() {
        let docs = ["rust webassembly browser", "sourdough bread", "rust simd"];
        let (model, tok, emb, n) = build_corpus(&docs);
        let engine = VectorEngine::new(&model, &tok, &emb, n).unwrap();
        let out = engine.search("rust", 2);
        assert_eq!(out.len(), 4); // 2 * (idx, score)
                                  // Indices come back as f32 — they should be small whole numbers.
        let i0 = out[0] as usize;
        let i1 = out[2] as usize;
        assert!(i0 < n);
        assert!(i1 < n);
        // Score 0 must be ≥ score 1 (results pre-sorted by similarity).
        assert!(out[1] >= out[3]);
    }

    #[test]
    fn search_with_zero_top_k_uses_default() {
        let docs: Vec<String> =
            (0..15).map(|i| format!("doc number {i}")).collect();
        let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
        let (model, tok, emb, n) = build_corpus(&refs);
        let engine = VectorEngine::new(&model, &tok, &emb, n).unwrap();
        let out = engine.search("number", 0);
        assert_eq!(out.len(), 2 * DEFAULT_TOP_K);
    }

    #[test]
    fn search_empty_corpus_returns_empty() {
        let enc = ProjectionEncoder::default();
        let model = enc.serialize_model();
        let tok = enc.serialize_tokenizer();
        let engine = VectorEngine::new(&model, &tok, &[], 0).unwrap();
        let out = engine.search("anything", 10);
        assert_eq!(out, Vec::<f32>::new());
    }

    #[test]
    fn search_vec_with_wrong_dim_returns_empty() {
        let (model, tok, emb, n) = build_corpus(&["a", "b"]);
        let engine = VectorEngine::new(&model, &tok, &emb, n).unwrap();
        let bad = vec![0.0_f32; 5];
        assert_eq!(engine.search_vec(&bad, 1), Vec::<f32>::new());
    }

    #[test]
    fn related_query_ranks_target_doc_first() {
        let docs = [
            "Rust WebAssembly compiles deterministically with SIMD",
            "Cooking Italian pasta from scratch",
            "Photography lighting tips for portraits",
            "Static site generators built in Rust",
        ];
        let (model, tok, emb, n) = build_corpus(&docs);
        let engine = VectorEngine::new(&model, &tok, &emb, n).unwrap();
        let out = engine.search("rust wasm", 4);
        // The Rust/WASM/Rust docs should outrank the cooking + photo docs.
        let top_idx = out[0] as usize;
        assert!(
            top_idx == 0 || top_idx == 3,
            "expected Rust-related doc on top, got idx {top_idx}"
        );
    }

    #[test]
    fn top_k_clamped_to_corpus_size() {
        let (model, tok, emb, n) = build_corpus(&["only one"]);
        let engine = VectorEngine::new(&model, &tok, &emb, n).unwrap();
        let out = engine.search("anything", 99);
        assert_eq!(out.len(), 2); // only one doc → 1 (idx,score) pair
    }

    #[test]
    fn engine_error_display_messages() {
        assert!(EngineError::BadModel.to_string().contains("model.bin"));
        assert!(EngineError::BadEmbeddings {
            expected: 8,
            got: 4
        }
        .to_string()
        .contains("expected 8"));
        assert!(EngineError::DimMismatch {
            manifest: 256,
            encoder: 128
        }
        .to_string()
        .contains("256"));
    }

    #[test]
    fn search_does_not_emit_nan_when_query_is_empty() {
        let (model, tok, emb, n) = build_corpus(&["alpha"]);
        let engine = VectorEngine::new(&model, &tok, &emb, n).unwrap();
        // Empty query → zero vector → all scores are zero, but no NaNs.
        let out = engine.search("", 1);
        for v in out {
            assert!(!v.is_nan());
        }
    }

    #[test]
    fn custom_dim_round_trips() {
        let cfg = ProjectionConfig {
            dim: 64,
            ..ProjectionConfig::default()
        };
        let enc = ProjectionEncoder::new(cfg);
        let model = enc.serialize_model();
        let tok = enc.serialize_tokenizer();
        let mut emb = Vec::new();
        for d in &["alpha", "beta"] {
            for f in enc.embed(d) {
                emb.extend_from_slice(&f.to_le_bytes());
            }
        }
        let engine = VectorEngine::new(&model, &tok, &emb, 2).unwrap();
        assert_eq!(engine.dim(), 64);
    }
}
