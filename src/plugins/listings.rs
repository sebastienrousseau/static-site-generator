// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Named, filtered, paginated listings (#587).
//!
//! [`crate::pagination`] already paginates: every dated page, newest
//! first, at `/page/N/`. One sequence, no name, no way to ask for a
//! subset. A site with a decade of posts, three languages and a dozen
//! tags cannot browse any of that.
//!
//! A listing is a named subset with its own URL space:
//!
//! ```toml
//! [[listings]]
//! name     = "archive"        # /archive/ and /archive/page/N/
//! title    = "Archive"
//! per_page = 20
//!
//! [[listings]]
//! name     = "rust"
//! title    = "Writing about Rust"
//! tag      = "rust"
//! after    = "2026-01-01"
//! by_year  = true             # also /rust/2026/
//! ```
//!
//! Every filter is optional and they combine with AND — a listing with
//! no filters is every dated page, which is what `/page/N/` already
//! gives you under a name of your choosing.
//!
//! What this deliberately does not have is the issue's "custom
//! predicate". A predicate is code, and a config file that grows an
//! expression language has usually taken a wrong turn; a site needing
//! one can write a plugin, which is the supported way to run code in a
//! build.

use serde::{Deserialize, Serialize};

/// One named listing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListingConfig {
    /// URL segment and directory name: `archive` → `/archive/`.
    pub name: String,
    /// Heading for the listing. Defaults to `name` when absent.
    #[serde(default)]
    pub title: Option<String>,
    /// Items per page. Zero or absent means the plugin default.
    #[serde(default)]
    pub per_page: Option<usize>,
    /// Only pages carrying this tag.
    #[serde(default)]
    pub tag: Option<String>,
    /// Only pages carrying this category.
    #[serde(default)]
    pub category: Option<String>,
    /// Only pages carrying this topic.
    #[serde(default)]
    pub topic: Option<String>,
    /// Only pages in this language.
    #[serde(default)]
    pub language: Option<String>,
    /// Only pages dated on or after this (`YYYY-MM-DD`).
    #[serde(default)]
    pub after: Option<String>,
    /// Only pages dated on or before this (`YYYY-MM-DD`).
    #[serde(default)]
    pub before: Option<String>,
    /// Also emit `/{name}/{year}/` for each year present.
    #[serde(default)]
    pub by_year: bool,
}

impl ListingConfig {
    /// The heading to render, falling back to the name.
    #[must_use]
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    /// Whether this listing filters at all.
    ///
    /// A listing with no filters is every dated page. That is legitimate
    /// — it is how you give `/page/N/` a name — but it is worth being
    /// able to say so.
    #[must_use]
    pub const fn is_unfiltered(&self) -> bool {
        self.tag.is_none()
            && self.category.is_none()
            && self.topic.is_none()
            && self.language.is_none()
            && self.after.is_none()
            && self.before.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_falls_back_to_the_name() {
        let l = ListingConfig {
            name: "archive".to_string(),
            ..ListingConfig::default()
        };
        assert_eq!(l.display_title(), "archive");
    }

    #[test]
    fn a_listing_with_no_filters_says_so() {
        let mut l = ListingConfig {
            name: "all".to_string(),
            ..ListingConfig::default()
        };
        assert!(l.is_unfiltered());
        l.tag = Some("rust".to_string());
        assert!(!l.is_unfiltered());
    }

    /// A typo in a listing section should be reported, not ignored: a
    /// silently dropped filter produces a listing that looks right and
    /// lists the wrong pages.
    #[test]
    fn an_unknown_field_is_rejected() {
        let err = toml::from_str::<ListingConfig>(
            "name = \"archive\"\ntagg = \"rust\"\n",
        );
        assert!(err.is_err(), "unknown field must not be silently dropped");
    }
}
