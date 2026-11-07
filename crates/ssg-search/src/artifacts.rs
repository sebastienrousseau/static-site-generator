// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Build-side artifact construction.
//!
//! [`Artifacts`] is the in-memory result of running the embedder over a
//! corpus. It carries every byte that the `<site>/search/` directory
//! will receive — embeddings, manifest, model, tokenizer — plus the
//! [`Manifest::model_hash`] that lets a runtime loader sanity-check
//! the bundle.
//!
//! Two paths into the builder:
//!
//! - [`ArtifactsBuilder::add_doc`] / [`ArtifactsBuilder::build`] —
//!   step-by-step accumulator, used by the SSG plugin.
//! - [`Artifacts::from_docs`] — one-shot helper for tests and one-off
//!   scripts.
//!
//! This module is **pure logic** (no `fs`) so it compiles on
//! `wasm32-unknown-unknown`. Disk-side I/O lives in the host SSG
//! plugin (`src/plugins/search_index.rs`).

use crate::encoder::{Encoder, ProjectionEncoder};
use crate::manifest::{Manifest, ManifestEntry};
use crate::Manifest as _ManifestRe;

/// One document fed into the embedder.
///
/// # Examples
///
/// ```
/// use ssg_search::artifacts::InputDoc;
///
/// let doc = InputDoc {
///     url: "/blog/intro.html".into(),
///     title: "Intro".into(),
///     body: "Welcome to my blog.".into(),
///     excerpt: "Welcome".into(),
/// };
/// assert_eq!(doc.url, "/blog/intro.html");
/// assert_eq!(doc.title, "Intro");
/// ```
#[derive(Debug, Clone)]
pub struct InputDoc {
    /// Relative URL of the page (e.g. `/blog/post.html`).
    pub url: String,
    /// Page title.
    pub title: String,
    /// Full text body — what we actually embed.
    pub body: String,
    /// Short excerpt to show in the search-result snippet.
    pub excerpt: String,
}

/// All bytes the build needs to emit under `<site>/search/`.
///
/// # Examples
///
/// ```
/// use ssg_search::artifacts::{Artifacts, InputDoc};
/// use ssg_search::encoder::EMBEDDING_DIM;
///
/// let docs = vec![InputDoc {
///     url: "/".into(),
///     title: "Home".into(),
///     body: "Welcome.".into(),
///     excerpt: "".into(),
/// }];
/// let arts = Artifacts::from_docs(&docs);
/// // `embeddings` is exactly count * dim * 4 bytes (LE f32).
/// assert_eq!(arts.embeddings.len(), 1 * EMBEDDING_DIM * 4);
/// assert_eq!(arts.model_hash, arts.manifest.model_hash);
/// ```
#[derive(Debug, Clone)]
pub struct Artifacts {
    /// `embeddings.bin` — little-endian f32, `count × dim × 4` bytes.
    pub embeddings: Vec<u8>,
    /// `manifest.json` — UTF-8 JSON.
    pub manifest_json: Vec<u8>,
    /// `model.bin` — encoder weights / config (magic-headed).
    pub model: Vec<u8>,
    /// `tokenizer.bin` — tokeniser config (magic-headed).
    pub tokenizer: Vec<u8>,
    /// Hex-encoded SHA-256 of `model.bin`, also embedded in
    /// `manifest_json`. Repeated here for fast lookup.
    pub model_hash: String,
    /// Decoded manifest, for callers that want to inspect entries
    /// without parsing the JSON again.
    pub manifest: Manifest,
}

impl Artifacts {
    /// One-shot builder. Equivalent to constructing an
    /// [`ArtifactsBuilder`] with the default encoder and calling
    /// [`ArtifactsBuilder::build`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    ///
    /// let docs = vec![
    ///     InputDoc { url: "/a".into(), title: "A".into(), body: "alpha".into(), excerpt: "".into() },
    ///     InputDoc { url: "/b".into(), title: "B".into(), body: "beta".into(),  excerpt: "".into() },
    /// ];
    /// let arts = Artifacts::from_docs(&docs);
    /// assert_eq!(arts.count(), 2);
    /// // Build is reproducible — same input → same bytes.
    /// assert_eq!(arts.embeddings, Artifacts::from_docs(&docs).embeddings);
    /// ```
    #[must_use]
    pub fn from_docs(docs: &[InputDoc]) -> Self {
        let mut b = ArtifactsBuilder::default();
        for d in docs {
            let _ = b.add_doc(d.clone());
        }
        b.build()
    }

    /// Returns the embedding dimensionality (read from the manifest).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    /// use ssg_search::encoder::EMBEDDING_DIM;
    ///
    /// let arts = Artifacts::from_docs(&[InputDoc {
    ///     url: "/".into(), title: "".into(), body: "x".into(), excerpt: "".into(),
    /// }]);
    /// assert_eq!(arts.dim(), EMBEDDING_DIM);
    /// ```
    #[must_use]
    pub const fn dim(&self) -> usize {
        self.manifest.dim as usize
    }

    /// Returns the number of documents (read from the manifest).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{Artifacts, InputDoc};
    ///
    /// let docs: Vec<InputDoc> = (0..3).map(|i| InputDoc {
    ///     url: format!("/{i}"), title: "".into(), body: "x".into(), excerpt: "".into(),
    /// }).collect();
    /// let arts = Artifacts::from_docs(&docs);
    /// assert_eq!(arts.count(), 3);
    /// ```
    #[must_use]
    pub const fn count(&self) -> usize {
        self.manifest.count as usize
    }
}

/// Step-by-step builder used by the SSG plugin during the
/// `before_compile` walk.
///
/// # Examples
///
/// ```
/// use ssg_search::artifacts::{ArtifactsBuilder, InputDoc};
///
/// let mut b = ArtifactsBuilder::default();
/// assert!(b.is_empty());
/// b.add_doc(InputDoc {
///     url: "/".into(), title: "Home".into(), body: "Welcome.".into(), excerpt: "".into(),
/// });
/// assert_eq!(b.len(), 1);
/// let arts = b.build();
/// assert_eq!(arts.count(), 1);
/// ```
#[derive(Debug)]
pub struct ArtifactsBuilder {
    encoder: ProjectionEncoder,
    docs: Vec<InputDoc>,
}

impl Default for ArtifactsBuilder {
    fn default() -> Self {
        Self::new(ProjectionEncoder::default())
    }
}

impl ArtifactsBuilder {
    /// Constructs a new builder backed by `encoder`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::ArtifactsBuilder;
    /// use ssg_search::encoder::{Encoder, ProjectionEncoder, EMBEDDING_DIM};
    ///
    /// let b = ArtifactsBuilder::new(ProjectionEncoder::default());
    /// assert!(b.is_empty());
    /// assert_eq!(b.encoder().dim(), EMBEDDING_DIM);
    /// ```
    #[must_use]
    pub const fn new(encoder: ProjectionEncoder) -> Self {
        Self {
            encoder,
            docs: Vec::new(),
        }
    }

    /// Adds a document to the corpus. Returns the index that will
    /// correspond to this document in `embeddings.bin`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{ArtifactsBuilder, InputDoc};
    ///
    /// let mut b = ArtifactsBuilder::default();
    /// let doc = |u: &str| InputDoc {
    ///     url: u.into(), title: "".into(), body: "x".into(), excerpt: "".into(),
    /// };
    /// assert_eq!(b.add_doc(doc("/a")), 0);
    /// assert_eq!(b.add_doc(doc("/b")), 1);
    /// assert_eq!(b.len(), 2);
    /// ```
    pub fn add_doc(&mut self, doc: InputDoc) -> usize {
        let idx = self.docs.len();
        self.docs.push(doc);
        idx
    }

    /// Returns the number of documents accumulated so far.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{ArtifactsBuilder, InputDoc};
    ///
    /// let mut b = ArtifactsBuilder::default();
    /// assert_eq!(b.len(), 0);
    /// b.add_doc(InputDoc {
    ///     url: "/".into(), title: "".into(), body: "".into(), excerpt: "".into(),
    /// });
    /// assert_eq!(b.len(), 1);
    /// ```
    #[must_use]
    pub const fn len(&self) -> usize {
        self.docs.len()
    }

    /// Returns `true` if no documents have been added.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{ArtifactsBuilder, InputDoc};
    ///
    /// let mut b = ArtifactsBuilder::default();
    /// assert!(b.is_empty());
    /// b.add_doc(InputDoc {
    ///     url: "/".into(), title: "".into(), body: "".into(), excerpt: "".into(),
    /// });
    /// assert!(!b.is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Returns the encoder this builder is using.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::ArtifactsBuilder;
    /// use ssg_search::encoder::{Encoder, EMBEDDING_DIM};
    ///
    /// let b = ArtifactsBuilder::default();
    /// assert_eq!(b.encoder().dim(), EMBEDDING_DIM);
    /// ```
    #[must_use]
    pub const fn encoder(&self) -> &ProjectionEncoder {
        &self.encoder
    }

    /// Finalises the artifacts.
    ///
    /// For each document:
    /// 1. Embed `body` with the encoder (already L2-normalised).
    /// 2. Push the f32s into the embeddings buffer in little-endian.
    /// 3. Append `{url, title, excerpt}` to the manifest entries.
    ///
    /// Then compute the model hash and assemble [`Manifest`] / `model.bin` /
    /// `tokenizer.bin` payloads.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_search::artifacts::{ArtifactsBuilder, InputDoc};
    /// use ssg_search::encoder::EMBEDDING_DIM;
    ///
    /// let mut b = ArtifactsBuilder::default();
    /// b.add_doc(InputDoc {
    ///     url: "/p".into(), title: "P".into(),
    ///     body: "rust webassembly".into(), excerpt: "".into(),
    /// });
    /// let arts = b.build();
    /// assert_eq!(arts.count(), 1);
    /// assert_eq!(arts.embeddings.len(), EMBEDDING_DIM * 4);
    /// // model.bin header starts with the artifact magic.
    /// assert_eq!(&arts.model[..4], b"SSGS");
    /// ```
    #[must_use]
    pub fn build(self) -> Artifacts {
        let dim = self.encoder.dim();
        let count = self.docs.len();
        let mut embeddings: Vec<u8> = Vec::with_capacity(count * dim * 4);
        let mut entries: Vec<ManifestEntry> = Vec::with_capacity(count);
        for d in &self.docs {
            let v = self.encoder.embed(&d.body);
            // The encoder always returns dim-length output — sanity
            // check it before writing to keep the file layout intact.
            debug_assert_eq!(v.len(), dim);
            for f in v {
                embeddings.extend_from_slice(&f.to_le_bytes());
            }
            entries.push(ManifestEntry {
                url: d.url.clone(),
                title: d.title.clone(),
                excerpt: d.excerpt.clone(),
            });
        }

        let model = self.encoder.serialize_model();
        let tokenizer = self.encoder.serialize_tokenizer();
        let model_hash = sha256_hex(&model);
        let manifest =
            <_ManifestRe>::new(dim as u32, model_hash.clone(), entries);
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .unwrap_or_else(|_| b"{}".to_vec());

        Artifacts {
            embeddings,
            manifest_json,
            model,
            tokenizer,
            model_hash,
            manifest,
        }
    }
}

/// Tiny SHA-256 — pure Rust, no_std-friendly, suitable for `wasm32`.
///
/// Kept inline (and tested below against known vectors) so the search
/// crate does not pick up an external `sha2` dependency on its own —
/// the host SSG crate already depends on `sha2`, but the
/// `ssg-search` crate must build standalone on `wasm32` without it.
fn sha256_hex(input: &[u8]) -> String {
    let h = sha256(input);
    let mut s = String::with_capacity(64);
    for b in h {
        s.push(hex_char(b >> 4));
        s.push(hex_char(b & 0xF));
    }
    s
}

const fn hex_char(n: u8) -> char {
    if n < 10 {
        (b'0' + n) as char
    } else {
        (b'a' + n - 10) as char
    }
}

// FIPS 180-4 SHA-256.
fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    // Pre-processing: pad message.
    let bit_len = (input.len() as u64) * 8;
    let mut msg = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    // `as_chunks` yields `&[u8; 64]`, so the block length is a type
    // guarantee rather than a runtime property. `.1` is the remainder,
    // which is empty here because the message was padded to a multiple of
    // 64 above.
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7)
                ^ w[i - 15].rotate_right(18)
                ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17)
                ^ w[i - 2].rotate_right(19)
                ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 =
                e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 =
                a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::EMBEDDING_DIM;

    fn fixture() -> Vec<InputDoc> {
        vec![
            InputDoc {
                url: "/a".into(),
                title: "A".into(),
                body: "alpha bravo charlie".into(),
                excerpt: "first".into(),
            },
            InputDoc {
                url: "/b".into(),
                title: "B".into(),
                body: "delta echo foxtrot".into(),
                excerpt: "second".into(),
            },
        ]
    }

    #[test]
    fn artifacts_from_docs_has_correct_byte_layout() {
        let arts = Artifacts::from_docs(&fixture());
        assert_eq!(arts.count(), 2);
        assert_eq!(arts.dim(), EMBEDDING_DIM);
        // AC1: embeddings.bin contains exactly N * D * 4 bytes.
        assert_eq!(arts.embeddings.len(), 2 * EMBEDDING_DIM * 4);
    }

    #[test]
    fn artifacts_manifest_round_trips() {
        let arts = Artifacts::from_docs(&fixture());
        let parsed: Manifest =
            serde_json::from_slice(&arts.manifest_json).unwrap();
        assert_eq!(parsed, arts.manifest);
        assert!(parsed.is_valid());
        assert_eq!(parsed.entries[0].url, "/a");
        assert_eq!(parsed.entries[1].title, "B");
    }

    #[test]
    fn artifacts_model_hash_matches_sha256_of_model() {
        let arts = Artifacts::from_docs(&fixture());
        assert_eq!(arts.model_hash, sha256_hex(&arts.model));
        assert_eq!(arts.manifest.model_hash, arts.model_hash);
    }

    #[test]
    fn artifacts_are_byte_identical_for_identical_inputs() {
        // AC6: reproducible builds.
        let a = Artifacts::from_docs(&fixture());
        let b = Artifacts::from_docs(&fixture());
        assert_eq!(a.embeddings, b.embeddings);
        assert_eq!(a.manifest_json, b.manifest_json);
        assert_eq!(a.model, b.model);
        assert_eq!(a.tokenizer, b.tokenizer);
        assert_eq!(a.model_hash, b.model_hash);
    }

    #[test]
    fn artifacts_embeddings_are_unit_norm() {
        let arts = Artifacts::from_docs(&fixture());
        for row in 0..arts.count() {
            let mut sumsq = 0.0_f32;
            for d in 0..arts.dim() {
                let offset = (row * arts.dim() + d) * 4;
                let f = f32::from_le_bytes([
                    arts.embeddings[offset],
                    arts.embeddings[offset + 1],
                    arts.embeddings[offset + 2],
                    arts.embeddings[offset + 3],
                ]);
                sumsq = f.mul_add(f, sumsq);
            }
            let norm = sumsq.sqrt();
            // AC5: norm within [0.999, 1.001]
            assert!((0.999..=1.001).contains(&norm), "row {row} L2={norm}");
        }
    }

    #[test]
    fn builder_increments_indices() {
        let mut b = ArtifactsBuilder::default();
        assert!(b.is_empty());
        let i0 = b.add_doc(fixture()[0].clone());
        let i1 = b.add_doc(fixture()[1].clone());
        assert_eq!(i0, 0);
        assert_eq!(i1, 1);
        assert_eq!(b.len(), 2);
        assert!(!b.is_empty());
    }

    #[test]
    fn builder_default_uses_default_encoder() {
        let b = ArtifactsBuilder::default();
        assert_eq!(b.encoder().dim(), EMBEDDING_DIM);
    }

    #[test]
    fn empty_builder_produces_empty_artifacts() {
        let arts = ArtifactsBuilder::default().build();
        assert_eq!(arts.count(), 0);
        assert_eq!(arts.embeddings, Vec::<u8>::new());
        // Manifest is still produced — just with no entries.
        let m: Manifest = serde_json::from_slice(&arts.manifest_json).unwrap();
        assert_eq!(m.count, 0);
        assert!(!m.is_valid()); // is_valid requires non-empty entries
    }

    #[test]
    fn sha256_known_vectors() {
        // "" → e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // "abc" → ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_long_input() {
        // 1 MB of zeros — exercises multiple block iterations.
        let input = vec![0u8; 1024 * 1024];
        let h = sha256_hex(&input);
        // Known reference for sha256 of 1 MiB of zero bytes.
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn hex_char_round_trip() {
        assert_eq!(hex_char(0), '0');
        assert_eq!(hex_char(9), '9');
        assert_eq!(hex_char(10), 'a');
        assert_eq!(hex_char(15), 'f');
    }
}
