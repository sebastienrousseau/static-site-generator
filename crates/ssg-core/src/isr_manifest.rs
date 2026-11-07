// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ISR build manifest — `dist/.ssg/manifest.json`.
//!
//! The manifest maps every output URL to its exact source dependency
//! set (markdown file + layout + partials + data files) plus a content
//! hash of those dependencies. The Edge renderer consults this manifest
//! to (a) find which sources to fetch from KV / Edge Config, and (b)
//! detect cache invalidation when sources change.
//!
//! Shape (canonical, stable):
//!
//! ```json
//! {
//!   "version": 1,
//!   "generated_at": "<rfc3339 timestamp or build-id>",
//!   "default_cache": { "s_maxage": 60, "swr": 86400 },
//!   "entries": {
//!     "/posts/foo/index.html": {
//!       "sources": ["content/posts/foo.md", "templates/post.html"],
//!       "hash": "<sha256-hex>",
//!       "cache": { "s_maxage": 600, "swr": 3600 }
//!     }
//!   }
//! }
//! ```
//!
//! `cache` is omitted at the entry level when the page wants the
//! site-wide default (`default_cache`). Per-route overrides come from
//! frontmatter (`isr.s_maxage`, `isr.swr`).
//!
//! ## Determinism
//!
//! Entries are written in lexicographic URL order so the manifest is
//! byte-stable for a given input set — critical for CDN cache keys
//! and reproducible builds.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Schema version of the manifest. Bump when the on-disk shape changes
/// in a way edge adapters need to detect.
pub const MANIFEST_VERSION: u32 = 1;

/// Default `s-maxage` (seconds) the manifest emits when a page does
/// not override via frontmatter. 60 s tracks the per-route SLA quoted
/// in issue #546.
pub const DEFAULT_S_MAXAGE: u32 = 60;

/// Default `stale-while-revalidate` (seconds). 24 h matches the
/// `Cache-Control: stale-while-revalidate=86400` snippet in the
/// architecture doc.
pub const DEFAULT_SWR: u32 = 86_400;

/// Per-page `Cache-Control` knobs.
///
/// Both fields are seconds, both optional at the entry level — when
/// absent we fall back to the manifest-wide [`Manifest::default_cache`].
///
/// # Examples
///
/// ```
/// use ssg_core::CachePolicy;
///
/// let policy = CachePolicy { s_maxage: 60, swr: 86_400 };
/// assert_eq!(
///     policy.to_cache_control(),
///     "s-maxage=60, stale-while-revalidate=86400",
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachePolicy {
    /// `s-maxage` — how long the CDN may serve the cached response
    /// before considering it stale.
    pub s_maxage: u32,
    /// `stale-while-revalidate` — how long the CDN may continue
    /// serving the stale response while it revalidates in the
    /// background.
    pub swr: u32,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            s_maxage: DEFAULT_S_MAXAGE,
            swr: DEFAULT_SWR,
        }
    }
}

impl CachePolicy {
    /// Renders the policy as a `Cache-Control` header value.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_core::CachePolicy;
    ///
    /// let policy = CachePolicy { s_maxage: 120, swr: 600 };
    /// assert_eq!(
    ///     policy.to_cache_control(),
    ///     "s-maxage=120, stale-while-revalidate=600",
    /// );
    /// ```
    #[must_use]
    pub fn to_cache_control(&self) -> String {
        format!(
            "s-maxage={}, stale-while-revalidate={}",
            self.s_maxage, self.swr
        )
    }
}

/// One entry in the ISR manifest — describes how one URL renders.
///
/// # Examples
///
/// ```
/// use ssg_core::{build_entry, ManifestEntry};
///
/// let entry: ManifestEntry =
///     build_entry(vec!["a.md".into()], &[b"# A"], None);
/// assert_eq!(entry.sources, vec!["a.md"]);
/// assert_eq!(entry.hash.len(), 64);
/// assert!(entry.cache.is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Source dependency keys, in deterministic order. Each entry is
    /// a path-shaped key that resolves through a `ContentProvider`.
    pub sources: Vec<String>,
    /// SHA-256 hex digest of the concatenated dependency bytes.
    /// Used by the Edge runtime to detect that a re-fetch is needed.
    pub hash: String,
    /// Per-page cache override. Omitted from JSON when `None` so the
    /// adapter falls back to the manifest-wide default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<CachePolicy>,
}

/// Complete ISR build manifest — emitted as `dist/.ssg/manifest.json`.
///
/// # Examples
///
/// ```
/// use ssg_core::{build_entry, Manifest};
///
/// let mut m = Manifest::new("build-1");
/// m.insert("/a.html", build_entry(vec!["a.md".into()], &[b"a"], None));
/// assert_eq!(m.len(), 1);
/// assert!(m.get("/a.html").is_some());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version (currently always [`MANIFEST_VERSION`]).
    pub version: u32,
    /// Identifier for the build that produced this manifest. Adapters
    /// use this to detect a stale KV namespace after a deploy.
    pub generated_at: String,
    /// Site-wide cache policy applied when an entry lacks its own
    /// `cache` field.
    pub default_cache: CachePolicy,
    /// URL → entry map. Iteration order is lexicographic.
    pub entries: BTreeMap<String, ManifestEntry>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self::new("unspecified")
    }
}

impl Manifest {
    /// Constructs an empty manifest stamped with `build_id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_core::{Manifest, MANIFEST_VERSION};
    ///
    /// let m = Manifest::new("build-42");
    /// assert_eq!(m.version, MANIFEST_VERSION);
    /// assert_eq!(m.generated_at, "build-42");
    /// assert!(m.is_empty());
    /// ```
    #[must_use]
    pub fn new(build_id: impl Into<String>) -> Self {
        Self {
            version: MANIFEST_VERSION,
            generated_at: build_id.into(),
            default_cache: CachePolicy::default(),
            entries: BTreeMap::new(),
        }
    }

    /// Inserts (or replaces) an entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_core::{build_entry, Manifest};
    ///
    /// let mut m = Manifest::new("b");
    /// m.insert("/a.html", build_entry(vec!["a.md".into()], &[b"a"], None));
    /// assert_eq!(m.len(), 1);
    /// ```
    pub fn insert(&mut self, url: impl Into<String>, entry: ManifestEntry) {
        let _ = self.entries.insert(url.into(), entry);
    }

    /// Returns the entry for `url`, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_core::{build_entry, Manifest};
    ///
    /// let mut m = Manifest::new("b");
    /// m.insert("/a.html", build_entry(vec!["a.md".into()], &[b"a"], None));
    /// assert!(m.get("/a.html").is_some());
    /// assert!(m.get("/missing").is_none());
    /// ```
    #[must_use]
    pub fn get(&self, url: &str) -> Option<&ManifestEntry> {
        self.entries.get(url)
    }

    /// Returns the number of entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_core::{build_entry, Manifest};
    ///
    /// let mut m = Manifest::new("b");
    /// assert_eq!(m.len(), 0);
    /// m.insert("/a.html", build_entry(vec!["a.md".into()], &[b"a"], None));
    /// assert_eq!(m.len(), 1);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reports whether the manifest holds no entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_core::{build_entry, Manifest};
    ///
    /// let mut m = Manifest::new("b");
    /// assert!(m.is_empty());
    /// m.insert("/a.html", build_entry(vec!["a.md".into()], &[b"a"], None));
    /// assert!(!m.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serialises to canonical pretty JSON (stable key order, 2-space
    /// indent). Suitable for direct write to `dist/.ssg/manifest.json`.
    ///
    /// # Errors
    /// Returns the underlying `serde_json::Error` if any entry is not
    /// representable (in practice this is unreachable — every field
    /// is a primitive or a `String`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_core::Manifest;
    ///
    /// let m = Manifest::new("build-1");
    /// let json = m.to_pretty_json().unwrap();
    /// assert!(json.contains("\"version\""));
    /// assert!(json.contains("\"generated_at\": \"build-1\""));
    /// ```
    pub fn to_pretty_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Returns every URL that depends on the given source key.
    ///
    /// Used by the invalidation webhook (AC8): given
    /// `content/posts/foo.md`, find every URL whose manifest entry
    /// lists it as a source — typically the post itself plus any
    /// tag/archive index that includes it.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg_core::{build_entry, Manifest};
    ///
    /// let mut m = Manifest::new("b");
    /// m.insert(
    ///     "/a.html",
    ///     build_entry(vec!["c/a.md".into()], &[b"A"], None),
    /// );
    /// m.insert(
    ///     "/b.html",
    ///     build_entry(vec!["c/b.md".into()], &[b"B"], None),
    /// );
    /// let deps = m.urls_for_source("c/a.md");
    /// assert_eq!(deps, vec!["/a.html"]);
    /// ```
    #[must_use]
    pub fn urls_for_source(&self, source_key: &str) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|(url, entry)| {
                if entry.sources.iter().any(|s| s == source_key) {
                    Some(url.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Computes the canonical SHA-256 hex digest for a set of source bytes.
///
/// The digest covers every source's *full bytes* in the order they
/// appear in `sources`, with a `0x00` separator between sources so
/// `["ab", "c"]` and `["a", "bc"]` produce distinct hashes. Sources
/// are NOT sorted — callers must hand them in deterministic order.
///
/// # Examples
///
/// ```
/// use ssg_core::hash_sources;
///
/// let a = hash_sources(&[b"hello", b"world"]);
/// let b = hash_sources(&[b"world", b"hello"]);
/// assert_eq!(a.len(), 64);
/// assert_ne!(a, b, "hash is order-sensitive");
/// ```
#[must_use]
pub fn hash_sources(sources: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for (i, src) in sources.iter().enumerate() {
        if i > 0 {
            hasher.update([0u8]);
        }
        hasher.update(src);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ =
            std::fmt::Write::write_fmt(&mut out, format_args!("{byte:02x}"));
    }
    out
}

/// Builds a [`ManifestEntry`] from sources + their bytes.
///
/// `sources` and `bytes` MUST be the same length and in matching
/// order. The resulting entry is hashed via [`hash_sources`].
///
/// # Panics
/// Panics in debug builds if the slice lengths disagree.
///
/// # Examples
///
/// ```
/// use ssg_core::build_entry;
///
/// let entry = build_entry(
///     vec!["a".into(), "b".into()],
///     &[b"alpha", b"beta"],
///     None,
/// );
/// assert_eq!(entry.sources, vec!["a", "b"]);
/// assert_eq!(entry.hash.len(), 64);
/// ```
#[must_use]
pub fn build_entry(
    sources: Vec<String>,
    bytes: &[&[u8]],
    cache: Option<CachePolicy>,
) -> ManifestEntry {
    debug_assert_eq!(
        sources.len(),
        bytes.len(),
        "sources and bytes must align"
    );
    let hash = hash_sources(bytes);
    ManifestEntry {
        sources,
        hash,
        cache,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip_json() {
        let mut m = Manifest::new("build-42");
        m.insert(
            "/index.html",
            ManifestEntry {
                sources: vec![
                    "content/index.md".into(),
                    "templates/base.html".into(),
                ],
                hash: hash_sources(&[b"# Home", b"<html></html>"]),
                cache: None,
            },
        );
        m.insert(
            "/posts/foo/index.html",
            ManifestEntry {
                sources: vec![
                    "content/posts/foo.md".into(),
                    "templates/post.html".into(),
                ],
                hash: hash_sources(&[b"# Foo", b"<html><body/></html>"]),
                cache: Some(CachePolicy {
                    s_maxage: 600,
                    swr: 3600,
                }),
            },
        );

        let json = m.to_pretty_json().unwrap();
        // Stable order — /index.html before /posts/foo/index.html.
        let i_idx = json.find("/index.html").unwrap();
        let p_idx = json.find("/posts/foo/index.html").unwrap();
        assert!(i_idx < p_idx, "URLs must be lexicographically ordered");

        let parsed: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn cache_policy_to_cache_control() {
        let p = CachePolicy {
            s_maxage: 120,
            swr: 600,
        };
        assert_eq!(
            p.to_cache_control(),
            "s-maxage=120, stale-while-revalidate=600"
        );
    }

    #[test]
    fn cache_policy_default_matches_constants() {
        let d = CachePolicy::default();
        assert_eq!(d.s_maxage, DEFAULT_S_MAXAGE);
        assert_eq!(d.swr, DEFAULT_SWR);
    }

    #[test]
    fn hash_sources_is_order_sensitive() {
        let a = hash_sources(&[b"hello", b"world"]);
        let b = hash_sources(&[b"world", b"hello"]);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_sources_avoids_concat_collision() {
        // ["ab", "c"] vs ["a", "bc"] must hash differently.
        let a = hash_sources(&[b"ab", b"c"]);
        let b = hash_sources(&[b"a", b"bc"]);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_sources_stable_for_same_input() {
        let a = hash_sources(&[b"foo", b"bar"]);
        let b = hash_sources(&[b"foo", b"bar"]);
        assert_eq!(a, b);
        // SHA-256 hex is 64 chars.
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn build_entry_populates_hash() {
        let entry = build_entry(
            vec!["a".into(), "b".into()],
            &[b"alpha", b"beta"],
            None,
        );
        assert_eq!(entry.sources, vec!["a", "b"]);
        assert!(entry.cache.is_none());
        assert_eq!(entry.hash.len(), 64);
    }

    #[test]
    fn manifest_get_returns_entry() {
        let mut m = Manifest::default();
        let entry = build_entry(vec!["x".into()], &[b"xxx"], None);
        m.insert("/x.html", entry.clone());
        assert_eq!(m.get("/x.html"), Some(&entry));
        assert!(m.get("/missing").is_none());
    }

    #[test]
    fn manifest_len_and_is_empty() {
        let mut m = Manifest::new("b");
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        m.insert("/a.html", build_entry(vec!["a".into()], &[b"a"], None));
        assert!(!m.is_empty());
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn urls_for_source_finds_dependents() {
        let mut m = Manifest::default();
        m.insert(
            "/a.html",
            build_entry(
                vec!["c/a.md".into(), "t/base.html".into()],
                &[b"A", b"T"],
                None,
            ),
        );
        m.insert(
            "/b.html",
            build_entry(
                vec!["c/b.md".into(), "t/base.html".into()],
                &[b"B", b"T"],
                None,
            ),
        );
        m.insert(
            "/tags/foo.html",
            build_entry(
                vec!["c/a.md".into(), "c/b.md".into(), "t/tag.html".into()],
                &[b"A", b"B", b"TAG"],
                None,
            ),
        );

        let mut deps = m.urls_for_source("c/a.md");
        deps.sort();
        assert_eq!(deps, vec!["/a.html", "/tags/foo.html"]);

        let mut tdeps = m.urls_for_source("t/base.html");
        tdeps.sort();
        assert_eq!(tdeps, vec!["/a.html", "/b.html"]);

        let none = m.urls_for_source("unknown.md");
        assert_eq!(none, Vec::<String>::new());
    }

    #[test]
    fn entry_cache_skipped_when_none() {
        let mut m = Manifest::default();
        m.insert("/a.html", build_entry(vec!["a.md".into()], &[b"a"], None));
        let json = m.to_pretty_json().unwrap();
        // entry has no "cache" key when None
        assert!(!json.contains("\"cache\""));
    }

    #[test]
    fn entry_cache_emitted_when_some() {
        let mut m = Manifest::default();
        m.insert(
            "/a.html",
            build_entry(
                vec!["a.md".into()],
                &[b"a"],
                Some(CachePolicy {
                    s_maxage: 10,
                    swr: 20,
                }),
            ),
        );
        let json = m.to_pretty_json().unwrap();
        assert!(json.contains("\"cache\""));
        assert!(json.contains("\"s_maxage\": 10"));
        assert!(json.contains("\"swr\": 20"));
    }

    #[test]
    fn manifest_version_is_one() {
        let m = Manifest::new("x");
        assert_eq!(m.version, 1);
    }
}
