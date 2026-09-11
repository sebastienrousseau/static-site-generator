// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Curated metadata for the `topics` taxonomy (#587).
//!
//! ssg already derives `/topics/{slug}/` from each page's `topics:`
//! front matter. What it cannot derive is editorial judgement: which
//! topic deserves a title that is not just its slug title-cased, what
//! the topic is *about*, and which of its pages should lead.
//!
//! A pillar page is that judgement written down. `_data/topics.toml`
//! supplies it, and every field is optional:
//!
//! ```toml
//! [post-quantum-cryptography]
//! title = "Post-Quantum Cryptography"
//! lede  = "Lattice-based cryptography, NIST PQC standards, and the \
//!          harvest-now-decrypt-later threat."
//! banner = "/images/pqc.webp"
//! order = [
//!   "quantum-safe-banking-index",
//!   "securing-the-ledger",
//! ]
//! ```
//!
//! A file that is absent, empty or unreadable leaves the taxonomy
//! exactly as it is today — this can only add to a build, never change
//! one that does not opt in.
//!
//! `order` names pages that should lead; anything not named keeps the
//! order the taxonomy already produced, appended after them. Naming a
//! page that is not in the topic is not an error: curation and content
//! drift apart, and failing a build over a stale slug in a data file
//! helps nobody. The same goes for a `[section]` naming a topic that no
//! page carries — it is reported once, on stderr, and skipped.

use std::collections::HashMap;
use std::path::Path;

/// Editorial metadata for one topic.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicCluster {
    /// Display title, in place of the slug title-cased.
    #[serde(default)]
    pub title: Option<String>,
    /// One-paragraph introduction shown above the page list.
    #[serde(default)]
    pub lede: Option<String>,
    /// Banner image URL for the pillar page.
    #[serde(default)]
    pub banner: Option<String>,
    /// Page slugs that should lead, in this order.
    #[serde(default)]
    pub order: Vec<String>,
}

/// Topic slug → curated metadata.
pub type TopicClusters = HashMap<String, TopicCluster>;

/// Relative location of the curated data file.
const DATA_PATH: [&str; 2] = ["_data", "topics.toml"];

/// Loads `_data/topics.toml`, or an empty map.
///
/// Looked for beside `content/` first — `_data/` is a sibling of the
/// content tree, not part of it, so it is not mistaken for a page — and
/// then inside it, which is where a project that keeps everything under
/// one directory will have put it.
///
/// Absence is the normal case and is silent. A file that exists but does
/// not parse is reported on stderr and then ignored: a typo in curation
/// data should not take a site build down with it.
#[must_use]
pub fn load(content_dir: &Path) -> TopicClusters {
    let mut candidates = Vec::new();
    if let Some(parent) = content_dir.parent() {
        candidates.push(parent.join(DATA_PATH[0]).join(DATA_PATH[1]));
    }
    candidates.push(content_dir.join(DATA_PATH[0]).join(DATA_PATH[1]));

    let Some((path, text)) = candidates
        .into_iter()
        .find_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
    else {
        return TopicClusters::new();
    };
    match toml::from_str::<TopicClusters>(&text) {
        Ok(clusters) => clusters,
        Err(e) => {
            eprintln!(
                "[topics] {} could not be parsed, ignoring it: {e}",
                path.display()
            );
            TopicClusters::new()
        }
    }
}

/// Reports `[section]`s naming a topic no page carries.
///
/// Curation drifts: a topic is renamed, its last page is unpublished, a
/// slug is mistyped. None of that should fail a build, but a silent
/// no-op is how a pillar page goes missing without anyone noticing.
pub fn warn_unknown(clusters: &TopicClusters, known: &[String]) {
    let mut unknown: Vec<&str> = clusters
        .keys()
        .filter(|k| !known.iter().any(|t| t == *k))
        .map(String::as_str)
        .collect();
    unknown.sort_unstable();
    for key in unknown {
        eprintln!(
            "[topics] _data/topics.toml describes '{key}', which no page \
             lists under `topics:`; skipping it."
        );
    }
}

/// Moves the pages named in `order` to the front, in that order.
///
/// Everything else keeps the order it already had. A named slug that is
/// not present is skipped rather than inserted, so a stale entry costs
/// nothing.
pub fn apply_order(order: &[String], pages: &mut Vec<(String, String)>) {
    if order.is_empty() {
        return;
    }
    let mut leading: Vec<(String, String)> = Vec::new();
    for want in order {
        if let Some(pos) = pages
            .iter()
            .position(|(_, url)| url_slug(url) == want.as_str())
        {
            leading.push(pages.remove(pos));
        }
    }
    if leading.is_empty() {
        return;
    }
    leading.append(pages);
    *pages = leading;
}

/// The slug a page URL ends in, in any of the shapes ssg emits.
///
/// `/posts/c/`, `/posts/c/index.html` and `/c.html` all name the page `c`,
/// which is what an author writes in `order`. Matching the raw final
/// segment would make the data file depend on which permalink style the
/// site happens to use.
fn url_slug(url: &str) -> &str {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if last.eq_ignore_ascii_case("index.html") {
        // `/posts/c/index.html` — the slug is the directory above.
        return trimmed
            .rsplit('/')
            .nth(1)
            .unwrap_or(last)
            .trim_end_matches(".html");
    }
    last.strip_suffix(".html").unwrap_or(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `content/` is passed in; the data file sits beside it.
    fn content_dir_with(toml: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("content")).expect("mkdir");
        let data = dir.path().join("_data");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("topics.toml"), toml).expect("write");
        dir
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load(&dir.path().join("content")).is_empty());
    }

    #[test]
    fn found_beside_the_content_directory() {
        let dir = content_dir_with("[payments]\ntitle = \"Payments\"\n");
        let clusters = load(&dir.path().join("content"));
        assert_eq!(
            clusters.get("payments").and_then(|c| c.title.as_deref()),
            Some("Payments")
        );
    }

    #[test]
    fn unparseable_file_is_ignored_rather_than_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("_data");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("topics.toml"), "this is not toml [[[")
            .expect("write");
        assert!(load(&dir.path().join("content")).is_empty());
    }

    #[test]
    fn every_field_is_optional() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("_data");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("topics.toml"), "[payments]\n")
            .expect("write");
        let clusters = load(&dir.path().join("content"));
        let payments = clusters.get("payments").expect("section loaded");
        assert!(payments.title.is_none());
        assert!(payments.lede.is_none());
        assert!(payments.order.is_empty());
    }

    #[test]
    fn order_moves_named_pages_to_the_front() {
        let mut pages = vec![
            ("A".to_string(), "/posts/a/".to_string()),
            ("B".to_string(), "/posts/b/".to_string()),
            ("C".to_string(), "/posts/c/".to_string()),
        ];
        apply_order(&["c".to_string(), "a".to_string()], &mut pages);
        let order: Vec<&str> = pages.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(order, ["C", "A", "B"]);
    }

    /// Curation and content drift apart; a stale slug must not reorder
    /// anything or panic.
    #[test]
    fn order_ignores_a_slug_that_is_not_in_the_topic() {
        let mut pages = vec![
            ("A".to_string(), "/posts/a/".to_string()),
            ("B".to_string(), "/posts/b/".to_string()),
        ];
        apply_order(&["gone".to_string(), "b".to_string()], &mut pages);
        let order: Vec<&str> = pages.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(order, ["B", "A"]);
    }

    /// The same page is named `c` whichever permalink style a site uses.
    #[test]
    fn order_matches_every_url_shape_ssg_emits() {
        for url in ["/posts/c/", "/posts/c/index.html", "/c.html", "/c"] {
            let mut pages = vec![
                ("A".to_string(), "/a.html".to_string()),
                ("C".to_string(), url.to_string()),
            ];
            apply_order(&["c".to_string()], &mut pages);
            assert_eq!(
                pages.first().map(|(t, _)| t.as_str()),
                Some("C"),
                "{url} did not match the slug `c`"
            );
        }
    }

    #[test]
    fn empty_order_leaves_the_taxonomy_order_alone() {
        let mut pages = vec![
            ("A".to_string(), "/posts/a/".to_string()),
            ("B".to_string(), "/posts/b/".to_string()),
        ];
        apply_order(&[], &mut pages);
        let order: Vec<&str> = pages.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(order, ["A", "B"]);
    }
}
