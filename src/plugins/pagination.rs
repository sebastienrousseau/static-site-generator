// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Pagination plugin.
//!
//! Generates paginated index pages (`/page/2/`, `/page/3/`, etc.)
//! from frontmatter sidecars when `paginate` is specified.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::plugins_group::listings::ListingConfig;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

/// Default number of items per page.
const DEFAULT_PER_PAGE: usize = 10;

/// Page metadata for pagination.
///
/// The terms and language are here so a named listing can filter on them
/// (#587). They are read in the same pass as the title and date: a
/// listing that re-read every sidecar per page would do it once per page
/// per listing, which on a ten-thousand-page corpus is the difference
/// between a build and a coffee break.
#[derive(Debug, Clone)]
struct PageEntry {
    title: String,
    url: String,
    date: String,
    tags: Vec<String>,
    categories: Vec<String>,
    topics: Vec<String>,
    language: Option<String>,
}

/// Plugin that generates paginated listing pages.
///
/// Runs in `after_compile`. Reads `.meta.json` sidecars, collects
/// pages with dates, sorts by date descending, and generates
/// `/page/N/index.html` files.
#[derive(Debug, Clone, Copy)]
pub struct PaginationPlugin {
    per_page: usize,
}

impl Default for PaginationPlugin {
    fn default() -> Self {
        Self {
            per_page: DEFAULT_PER_PAGE,
        }
    }
}

impl PaginationPlugin {
    /// Creates a pagination plugin with a custom page size.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::pagination::PaginationPlugin;
    /// use ssg::plugin::Plugin;
    ///
    /// let p = PaginationPlugin::with_per_page(25);
    /// assert_eq!(p.name(), "pagination");
    /// ```
    #[must_use]
    pub fn with_per_page(per_page: usize) -> Self {
        Self {
            per_page: per_page.max(1),
        }
    }
}

impl Plugin for PaginationPlugin {
    fn name(&self) -> &'static str {
        "pagination"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        let sidecar_dir = ctx.build_dir.join(".meta");
        if !sidecar_dir.exists() {
            return Ok(());
        }

        let mut entries = collect_page_entries(&sidecar_dir)?;
        if entries.is_empty() {
            return Ok(());
        }

        // Date desc, then URL asc. The URL tiebreak makes this a total
        // order: dates here are date-only strings, so a site publishing
        // several pages on one day left them in whatever order the
        // sidecar walk produced, and the paginated listings then differed
        // between filesystems. URLs are unique per page.
        entries.sort_by(|a, b| {
            b.date.cmp(&a.date).then_with(|| a.url.cmp(&b.url))
        });

        // Named listings (#587). Each is a filtered view of the same
        // entries, which were read once — a listing that re-read the
        // sidecars would do so once per listing, per page.
        let listings = ctx
            .config
            .as_ref()
            .map_or::<&[ListingConfig], _>(&[], |c| c.listings.as_slice());
        for listing in listings {
            if listing.name.trim().is_empty() {
                log::warn!("[listings] a listing with no name was skipped");
                continue;
            }
            let selected: Vec<PageEntry> = entries
                .iter()
                .filter(|e| entry_matches(e, listing))
                .cloned()
                .collect();
            if selected.is_empty() {
                // A listing that matches nothing is usually a filter
                // typo, and an empty directory is a worse way to find
                // out than a line on the console.
                log::warn!(
                    "[listings] '{}' matched no pages; nothing written",
                    listing.name
                );
                continue;
            }
            let pages = write_listing(
                &ctx.site_dir,
                listing,
                &selected,
                self.per_page,
            )?;
            let years = if listing.by_year {
                write_year_archives(&ctx.site_dir, listing, &selected)?
            } else {
                0
            };
            log::info!(
                "[listings] '{}': {} page(s), {} year archive(s), {} entries",
                listing.name,
                pages,
                years,
                selected.len()
            );
        }

        let total_pages = entries.len().div_ceil(self.per_page);
        if total_pages <= 1 {
            return Ok(());
        }

        let page_dir = ctx.site_dir.join("page");
        for page_num in 2..=total_pages {
            let start = (page_num - 1) * self.per_page;
            let end = (start + self.per_page).min(entries.len());
            let page_entries = &entries[start..end];

            write_pagination_page(
                &page_dir,
                page_num,
                total_pages,
                page_entries,
            )?;
        }

        log::info!(
            "[pagination] Generated {} page(s) ({} entries, {} per page)",
            total_pages - 1,
            entries.len(),
            self.per_page
        );
        Ok(())
    }
}

/// Whether `entry` belongs in `listing`.
///
/// Filters combine with AND, and each is skipped when absent, so a
/// listing with none of them matches every dated page. Term matching is
/// case-insensitive: `tag = "Rust"` and `tags: "rust"` are the same tag
/// to a reader, and a listing that silently missed half its pages over
/// capitalisation would be a poor way to find that out.
fn entry_matches(entry: &PageEntry, listing: &ListingConfig) -> bool {
    let has = |terms: &[String], want: &Option<String>| -> bool {
        want.as_ref()
            .is_none_or(|w| terms.iter().any(|t| t.eq_ignore_ascii_case(w)))
    };

    has(&entry.tags, &listing.tag)
        && has(&entry.categories, &listing.category)
        && has(&entry.topics, &listing.topic)
        && listing.language.as_ref().is_none_or(|want| {
            entry
                .language
                .as_ref()
                .is_some_and(|l| l.eq_ignore_ascii_case(want))
        })
        // Dates are `YYYY-MM-DD`, which compares correctly as a string.
        // A page whose date is malformed sorts where its text puts it
        // rather than being dropped, which is the same latitude the rest
        // of the pipeline gives it.
        && listing.after.as_ref().is_none_or(|a| entry.date >= *a)
        && listing.before.as_ref().is_none_or(|b| entry.date <= *b)
}

/// Writes one listing: page 1 at `/{name}/`, the rest at `/{name}/page/N/`.
///
/// Unlike the site-wide pagination, page 1 is written here. There is no
/// pre-existing index at `/{name}/` to defer to — the listing is the only
/// thing that knows the directory exists.
fn write_listing(
    site_dir: &Path,
    listing: &ListingConfig,
    entries: &[PageEntry],
    default_per_page: usize,
) -> Result<usize, SsgError> {
    if entries.is_empty() {
        return Ok(0);
    }
    let per_page = listing
        .per_page
        .filter(|n| *n > 0)
        .unwrap_or(default_per_page);
    let total_pages = entries.len().div_ceil(per_page);
    let dir = site_dir.join(&listing.name);

    for page_num in 1..=total_pages {
        let start = (page_num - 1) * per_page;
        let end = (start + per_page).min(entries.len());
        let target = if page_num == 1 {
            dir.clone()
        } else {
            dir.join("page").join(page_num.to_string())
        };
        write_listing_page(
            &target,
            listing,
            page_num,
            total_pages,
            &entries[start..end],
        )?;
    }
    Ok(total_pages)
}

/// Groups entries by the year in their date and writes `/{name}/{year}/`.
///
/// The year is the first four characters of the date, which is what
/// `YYYY-MM-DD` guarantees; anything shorter is skipped rather than
/// producing a `/archive//` directory.
fn write_year_archives(
    site_dir: &Path,
    listing: &ListingConfig,
    entries: &[PageEntry],
) -> Result<usize, SsgError> {
    let mut by_year: BTreeMap<&str, Vec<PageEntry>> = BTreeMap::new();
    for entry in entries {
        if entry.date.len() >= 4 {
            by_year
                .entry(&entry.date[..4])
                .or_default()
                .push(entry.clone());
        }
    }
    let count = by_year.len();
    for (year, group) in by_year {
        let target = site_dir.join(&listing.name).join(year);
        write_listing_page(&target, listing, 1, 1, &group)?;
    }
    Ok(count)
}

/// Collects page entries with dates from sidecar JSON files.
fn collect_page_entries(
    sidecar_dir: &Path,
) -> Result<Vec<PageEntry>, SsgError> {
    let sidecars = collect_json_files(sidecar_dir)?;
    let mut entries = Vec::new();

    for sidecar_path in &sidecars {
        if let Some(entry) = parse_page_entry(sidecar_path, sidecar_dir) {
            entries.push(entry);
        }
    }

    Ok(entries)
}

/// Parses a single sidecar JSON file into a `PageEntry`, if it has a date.
fn parse_page_entry(
    sidecar_path: &Path,
    sidecar_dir: &Path,
) -> Option<PageEntry> {
    let content = fs::read_to_string(sidecar_path).ok()?;
    let meta: HashMap<String, serde_json::Value> =
        serde_json::from_str(&content).ok()?;

    let title = meta
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled")
        .to_string();
    let date = meta
        .get("date")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if date.is_empty() {
        return None;
    }

    let rel = sidecar_path
        .strip_prefix(sidecar_dir)
        .unwrap_or(sidecar_path)
        .with_extension("")
        .with_extension("html");
    let url = format!("/{}", rel.to_string_lossy().replace('\\', "/"));

    // Both front-matter shapes, as everywhere else: `tags: [a, b]` and
    // `tags: "a, b"`.
    let terms = |key: &str| -> Vec<String> {
        meta.get(key).map_or_else(Vec::new, |v| {
            v.as_array().map_or_else(
                || {
                    v.as_str()
                        .map_or_else(Vec::new, |s| ssg_core::split_terms(s))
                },
                |arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .flat_map(ssg_core::split_terms)
                        .collect()
                },
            )
        })
    };

    Some(PageEntry {
        title,
        url,
        date,
        tags: terms("tags"),
        categories: terms("categories"),
        topics: terms("topic_clusters"),
        language: meta
            .get("language")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
    })
}

/// Writes a single pagination page to disk.
fn write_pagination_page(
    page_dir: &Path,
    page_num: usize,
    total_pages: usize,
    page_entries: &[PageEntry],
) -> Result<(), SsgError> {
    let dir = page_dir.join(page_num.to_string());
    fs::create_dir_all(&dir).with_path(&dir)?;

    let prev_url = if page_num == 2 {
        "/".to_string()
    } else {
        format!("/page/{}/", page_num - 1)
    };
    let next_url = if page_num < total_pages {
        Some(format!("/page/{}/", page_num + 1))
    } else {
        None
    };

    let mut html = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\
         <meta charset=\"utf-8\">\
         <title>Page {page_num} of {total_pages}</title></head>\n\
         <body>\n<main>\n\
         <h1>Page {page_num} of {total_pages}</h1>\n<ul>\n",
    );

    for entry in page_entries {
        html.push_str(&format!(
            "<li><a href=\"{}\">{}</a> <time>{}</time></li>\n",
            entry.url, entry.title, entry.date
        ));
    }

    html.push_str("</ul>\n<nav aria-label=\"Pagination\">\n");
    html.push_str(&format!(
        "<a href=\"{prev_url}\" rel=\"prev\">&larr; Previous</a>\n"
    ));
    if let Some(next) = &next_url {
        html.push_str(&format!(
            "<a href=\"{next}\" rel=\"next\">Next &rarr;</a>\n"
        ));
    }
    html.push_str("</nav>\n</main>\n</body>\n</html>\n");

    let out_file = dir.join("index.html");
    fs::write(&out_file, html).with_path(&out_file)?;
    Ok(())
}

/// Writes one page of a named listing to `dir/index.html`.
///
/// Titles and dates are escaped: they come from front matter, and a
/// title containing `<` would otherwise close the anchor and swallow the
/// rest of the list.
fn write_listing_page(
    dir: &Path,
    listing: &ListingConfig,
    page_num: usize,
    total_pages: usize,
    entries: &[PageEntry],
) -> Result<(), SsgError> {
    fs::create_dir_all(dir).with_path(dir)?;

    let base = format!("/{}", listing.name);
    let page_url = |n: usize| -> String {
        if n == 1 {
            format!("{base}/")
        } else {
            format!("{base}/page/{n}/")
        }
    };

    let title = escape_html(listing.display_title());
    let heading = if total_pages > 1 {
        format!("{title} — page {page_num} of {total_pages}")
    } else {
        title.clone()
    };

    let mut html = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\
         <meta charset=\"utf-8\">\
         <title>{heading}</title></head>\n\
         <body>\n<main>\n\
         <h1>{heading}</h1>\n<ul>\n",
    );
    for entry in entries {
        html.push_str(&format!(
            "<li><a href=\"{}\">{}</a> <time datetime=\"{}\">{}</time></li>\n",
            escape_html(&entry.url),
            escape_html(&entry.title),
            escape_html(&entry.date),
            escape_html(&entry.date),
        ));
    }
    html.push_str("</ul>\n");

    if total_pages > 1 {
        html.push_str("<nav aria-label=\"Pagination\">\n");
        if page_num > 1 {
            html.push_str(&format!(
                "<a href=\"{}\" rel=\"prev\">&larr; Previous</a>\n",
                page_url(page_num - 1)
            ));
        }
        if page_num < total_pages {
            html.push_str(&format!(
                "<a href=\"{}\" rel=\"next\">Next &rarr;</a>\n",
                page_url(page_num + 1)
            ));
        }
        html.push_str("</nav>\n");
    }
    html.push_str("</main>\n</body>\n</html>\n");

    let out_file = dir.join("index.html");
    fs::write(&out_file, html).with_path(&out_file)?;
    Ok(())
}

/// Minimal HTML text/attribute escaping for generated listings.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn collect_json_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files(dir, "json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::init_logger;
    use std::path::PathBuf;
    use tempfile::{tempdir, TempDir};

    // -------------------------------------------------------------------
    // Test fixtures
    // -------------------------------------------------------------------

    /// Builds a fresh temp dir layout: `<root>/site`, `<root>/build/.meta`,
    /// and a `PluginContext` pointing at it. Returns the temp dir guard
    /// (must outlive the test), the site path, the meta sidecar path,
    /// and the context.
    fn make_layout() -> (TempDir, PathBuf, PathBuf, PluginContext) {
        init_logger();
        let dir = tempdir().expect("create tempdir");
        let site = dir.path().join("site");
        let build = dir.path().join("build");
        let meta = build.join(".meta");
        fs::create_dir_all(&site).expect("mkdir site");
        fs::create_dir_all(&meta).expect("mkdir meta");
        let ctx = PluginContext::new(dir.path(), &build, &site, dir.path());
        (dir, site, meta, ctx)
    }

    /// A layout whose config carries the given listings (#587).
    fn layout_with_listings(
        listings: Vec<ListingConfig>,
    ) -> (TempDir, PathBuf, PathBuf, PluginContext) {
        init_logger();
        let dir = tempdir().expect("create tempdir");
        let site = dir.path().join("site");
        let build = dir.path().join("build");
        let meta = build.join(".meta");
        fs::create_dir_all(&site).expect("mkdir site");
        fs::create_dir_all(&meta).expect("mkdir meta");
        let mut cfg = crate::cmd::default_config().as_ref().clone();
        cfg.listings = listings;
        let ctx = PluginContext::with_config(
            dir.path(),
            &build,
            &site,
            dir.path(),
            cfg,
        );
        (dir, site, meta, ctx)
    }

    /// A sidecar with terms and a language, for listing filters.
    fn write_rich_sidecar(
        meta: &Path,
        name: &str,
        title: &str,
        date: &str,
        extra: &str,
    ) {
        let json = format!(
            r#"{{"title": "{title}", "date": "{date}"{}{extra}}}"#,
            if extra.is_empty() { "" } else { ", " }
        );
        fs::write(meta.join(format!("{name}.meta.json")), json)
            .expect("write sidecar");
    }

    #[test]
    fn a_named_listing_writes_page_one_at_its_own_path() {
        let (_d, site, meta, ctx) = layout_with_listings(vec![ListingConfig {
            name: "archive".to_string(),
            title: Some("Archive".to_string()),
            ..ListingConfig::default()
        }]);
        write_sidecar(&meta, "a", "Alpha", "2026-01-01");

        PaginationPlugin::default().after_compile(&ctx).unwrap();

        let page = fs::read_to_string(site.join("archive/index.html"))
            .expect("page 1 is written at the listing root");
        assert!(page.contains("Archive"), "{page}");
        assert!(page.contains("Alpha"), "{page}");
    }

    #[test]
    fn a_listing_paginates_at_its_own_per_page() {
        let (_d, site, meta, ctx) = layout_with_listings(vec![ListingConfig {
            name: "archive".to_string(),
            per_page: Some(2),
            ..ListingConfig::default()
        }]);
        for i in 1..=5 {
            write_sidecar(
                &meta,
                &format!("p{i}"),
                &format!("P{i}"),
                &format!("2026-01-0{i}"),
            );
        }

        PaginationPlugin::default().after_compile(&ctx).unwrap();

        assert!(site.join("archive/index.html").exists());
        assert!(site.join("archive/page/2/index.html").exists());
        assert!(site.join("archive/page/3/index.html").exists());
        assert!(
            !site.join("archive/page/4/index.html").exists(),
            "5 entries at 2 per page is 3 pages"
        );
        let p2 =
            fs::read_to_string(site.join("archive/page/2/index.html")).unwrap();
        assert!(
            p2.contains(r#"href="/archive/""#),
            "prev goes to page 1: {p2}"
        );
        assert!(p2.contains(r#"href="/archive/page/3/""#), "next: {p2}");
    }

    #[test]
    fn filters_select_the_pages_and_combine_with_and() {
        let (_d, site, meta, ctx) = layout_with_listings(vec![ListingConfig {
            name: "rust-2026".to_string(),
            tag: Some("rust".to_string()),
            after: Some("2026-01-01".to_string()),
            ..ListingConfig::default()
        }]);
        write_rich_sidecar(
            &meta,
            "a",
            "Match",
            "2026-06-01",
            r#""tags": "rust""#,
        );
        write_rich_sidecar(
            &meta,
            "b",
            "WrongTag",
            "2026-06-01",
            r#""tags": "go""#,
        );
        write_rich_sidecar(
            &meta,
            "c",
            "TooOld",
            "2025-06-01",
            r#""tags": "rust""#,
        );

        PaginationPlugin::default().after_compile(&ctx).unwrap();

        let page =
            fs::read_to_string(site.join("rust-2026/index.html")).unwrap();
        assert!(page.contains("Match"), "{page}");
        assert!(!page.contains("WrongTag"), "tag filter: {page}");
        assert!(!page.contains("TooOld"), "date filter: {page}");
    }

    /// `tag = "Rust"` and `tags: "rust"` are the same tag to a reader.
    #[test]
    fn term_filters_ignore_case() {
        let (_d, site, meta, ctx) = layout_with_listings(vec![ListingConfig {
            name: "r".to_string(),
            tag: Some("Rust".to_string()),
            ..ListingConfig::default()
        }]);
        write_rich_sidecar(
            &meta,
            "a",
            "Alpha",
            "2026-01-01",
            r#""tags": "rust""#,
        );

        PaginationPlugin::default().after_compile(&ctx).unwrap();
        assert!(fs::read_to_string(site.join("r/index.html"))
            .unwrap()
            .contains("Alpha"));
    }

    #[test]
    fn by_year_writes_one_archive_per_year() {
        let (_d, site, meta, ctx) = layout_with_listings(vec![ListingConfig {
            name: "archive".to_string(),
            by_year: true,
            ..ListingConfig::default()
        }]);
        write_sidecar(&meta, "a", "Old", "2025-03-01");
        write_sidecar(&meta, "b", "New", "2026-03-01");

        PaginationPlugin::default().after_compile(&ctx).unwrap();

        let y2025 =
            fs::read_to_string(site.join("archive/2025/index.html")).unwrap();
        assert!(y2025.contains("Old") && !y2025.contains("New"), "{y2025}");
        let y2026 =
            fs::read_to_string(site.join("archive/2026/index.html")).unwrap();
        assert!(y2026.contains("New") && !y2026.contains("Old"), "{y2026}");
    }

    /// A filter that matches nothing writes nothing — and says so.
    #[test]
    fn a_listing_matching_nothing_writes_no_directory() {
        let (_d, site, meta, ctx) = layout_with_listings(vec![ListingConfig {
            name: "ghost".to_string(),
            tag: Some("nonexistent".to_string()),
            ..ListingConfig::default()
        }]);
        write_sidecar(&meta, "a", "Alpha", "2026-01-01");

        PaginationPlugin::default().after_compile(&ctx).unwrap();
        assert!(!site.join("ghost").exists(), "no empty listing directory");
    }

    /// Front-matter text reaches the page as text, not as markup.
    #[test]
    fn titles_are_escaped_in_generated_listings() {
        let (_d, site, meta, ctx) = layout_with_listings(vec![ListingConfig {
            name: "archive".to_string(),
            ..ListingConfig::default()
        }]);
        write_sidecar(&meta, "a", "A <b>bold</b> title", "2026-01-01");

        PaginationPlugin::default().after_compile(&ctx).unwrap();
        let page = fs::read_to_string(site.join("archive/index.html")).unwrap();
        assert!(page.contains("&lt;b&gt;bold&lt;/b&gt;"), "{page}");
        assert!(!page.contains("<b>bold</b>"), "{page}");
    }

    /// Sites with no listings configured behave exactly as before.
    #[test]
    fn no_listings_configured_writes_no_listing_directories() {
        let (_d, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 25);

        PaginationPlugin::default().after_compile(&ctx).unwrap();

        assert!(
            site.join("page/2/index.html").exists(),
            "site-wide unchanged"
        );
        let dirs: Vec<_> = fs::read_dir(&site)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(dirs, vec!["page".to_string()], "only /page/: {dirs:?}");
    }

    /// Writes a sidecar JSON file shaped `{"title": ..., "date": ...}`.
    fn write_sidecar(meta: &Path, name: &str, title: &str, date: &str) {
        let json = if date.is_empty() {
            format!(r#"{{"title": "{title}"}}"#)
        } else {
            format!(r#"{{"title": "{title}", "date": "{date}"}}"#)
        };
        fs::write(meta.join(format!("{name}.meta.json")), json)
            .expect("write sidecar");
    }

    /// Writes `n` dated posts numbered 1..=n with monotonically
    /// increasing dates so sort order is well-defined.
    fn write_n_dated_posts(meta: &Path, n: usize) {
        for i in 1..=n {
            write_sidecar(
                meta,
                &format!("post{i:03}"),
                &format!("Post {i}"),
                &format!("2026-01-{i:02}"),
            );
        }
    }

    // -------------------------------------------------------------------
    // Constructor + derive surface
    // -------------------------------------------------------------------

    #[test]
    fn default_uses_default_per_page_constant() {
        // The Default impl is the public ergonomic — assert it matches
        // the documented constant rather than a magic number, so the
        // test stays correct if DEFAULT_PER_PAGE is ever retuned.
        let plugin = PaginationPlugin::default();
        assert_eq!(plugin.per_page, DEFAULT_PER_PAGE);
    }

    #[test]
    fn with_per_page_stores_supplied_value() {
        let plugin = PaginationPlugin::with_per_page(7);
        assert_eq!(plugin.per_page, 7);
    }

    #[test]
    fn with_per_page_zero_clamps_to_one() {
        // Zero would cause a divide-by-zero in `div_ceil`. The
        // constructor must clamp it.
        let plugin = PaginationPlugin::with_per_page(0);
        assert_eq!(plugin.per_page, 1);
    }

    #[test]
    fn with_per_page_one_is_valid_lower_bound() {
        let plugin = PaginationPlugin::with_per_page(1);
        assert_eq!(plugin.per_page, 1);
    }

    #[test]
    fn with_per_page_table_driven_values() {
        // Table-driven sanity check across a spread of valid sizes.
        let cases: &[(usize, usize)] = &[
            (1, 1),
            (5, 5),
            (10, 10),
            (100, 100),
            (usize::MAX, usize::MAX),
        ];
        for &(input, expected) in cases {
            let plugin = PaginationPlugin::with_per_page(input);
            assert_eq!(
                plugin.per_page, expected,
                "with_per_page({input}) should store {expected}"
            );
        }
    }

    #[test]
    fn pagination_plugin_is_copy_after_move() {
        // Guards the `Copy` derive added in v0.0.34.
        let plugin = PaginationPlugin::with_per_page(3);
        let _copy = plugin;
        assert_eq!(plugin.per_page, 3);
    }

    #[test]
    fn name_returns_static_pagination_identifier() {
        let plugin = PaginationPlugin::default();
        assert_eq!(plugin.name(), "pagination");
    }

    // -------------------------------------------------------------------
    // after_compile — early-return paths
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_missing_meta_dir_returns_ok_without_writing() {
        // No `.meta` directory under build/ — must short-circuit, not
        // error, and must not create the page/ directory.
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let build = dir.path().join("build");
        fs::create_dir_all(&site).expect("mkdir site");
        fs::create_dir_all(&build).expect("mkdir build");
        let ctx = PluginContext::new(dir.path(), &build, &site, dir.path());

        PaginationPlugin::default()
            .after_compile(&ctx)
            .expect("missing meta dir is not an error");

        assert!(!site.join("page").exists());
    }

    #[test]
    fn after_compile_empty_meta_dir_returns_ok_without_writing() {
        let (_tmp, site, _meta, ctx) = make_layout();
        PaginationPlugin::default()
            .after_compile(&ctx)
            .expect("empty meta is fine");
        assert!(!site.join("page").exists());
    }

    #[test]
    fn after_compile_only_undated_pages_returns_ok_without_writing() {
        // Pages without `date` are skipped — see line 91. Only undated
        // entries means `entries.is_empty()` short-circuit at line 105.
        let (_tmp, site, meta, ctx) = make_layout();
        write_sidecar(&meta, "about", "About", "");
        write_sidecar(&meta, "contact", "Contact", "");

        PaginationPlugin::default().after_compile(&ctx).unwrap();
        assert!(!site.join("page").exists());
    }

    #[test]
    fn after_compile_single_page_skips_pagination() {
        // 5 dated posts at default per_page=10 → 1 page total → no
        // /page/N/ directories produced (line 114 `total_pages <= 1`).
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 5);

        PaginationPlugin::default().after_compile(&ctx).unwrap();
        assert!(!site.join("page").exists());
    }

    // -------------------------------------------------------------------
    // after_compile — sidecar parsing fallbacks
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_skips_invalid_json_sidecars() {
        // The JSON parser error branch at line 76 must not propagate —
        // bad sidecars are silently skipped so a single corrupt file
        // can't poison the whole build.
        let (_tmp, site, meta, ctx) = make_layout();
        fs::write(meta.join("broken.meta.json"), "{not valid json").unwrap();
        // Add 11 valid posts so we still cross the pagination threshold
        // (default per_page=10 → 2 pages).
        write_n_dated_posts(&meta, 11);

        PaginationPlugin::default()
            .after_compile(&ctx)
            .expect("broken sidecar must not error");
        assert!(site.join("page/2/index.html").exists());
    }

    #[test]
    fn after_compile_missing_title_defaults_to_untitled() {
        // Pages without a `title` field but with a `date` are still
        // paginated; the title falls back to "Untitled" (line 82).
        let (_tmp, site, meta, ctx) = make_layout();
        // 11 entries with NO title field → "Untitled" fallback used.
        for i in 1..=11 {
            fs::write(
                meta.join(format!("post{i}.meta.json")),
                format!(r#"{{"date": "2026-01-{i:02}"}}"#),
            )
            .unwrap();
        }

        PaginationPlugin::default().after_compile(&ctx).unwrap();
        let page2 = fs::read_to_string(site.join("page/2/index.html")).unwrap();
        assert!(
            page2.contains("Untitled"),
            "missing title must fall back to \"Untitled\":\n{page2}"
        );
    }

    #[test]
    fn after_compile_skips_pages_with_empty_date_string() {
        // A `date` field present but empty must be treated the same
        // as a missing date (line 91-93).
        let (_tmp, site, meta, ctx) = make_layout();
        write_sidecar(&meta, "draft", "Draft", ""); // empty date branch
        write_n_dated_posts(&meta, 11);

        PaginationPlugin::default().after_compile(&ctx).unwrap();
        // Only the 11 dated posts paginate; the empty-date entry is
        // ignored, so we get exactly one /page/2/ (11 → 2 pages).
        assert!(site.join("page/2/index.html").exists());
        assert!(!site.join("page/3/index.html").exists());
    }

    // -------------------------------------------------------------------
    // after_compile — page slicing arithmetic
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_exact_multiple_yields_full_pages() {
        // 10 posts at per_page=5 → exactly 2 full pages, no remainder.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 10);

        PaginationPlugin::with_per_page(5)
            .after_compile(&ctx)
            .unwrap();

        let page2 = fs::read_to_string(site.join("page/2/index.html")).unwrap();
        // Page 2 of 2: should contain exactly 5 list items.
        let li_count = page2.matches("<li>").count();
        assert_eq!(li_count, 5, "page 2 should have 5 entries:\n{page2}");
        assert!(!site.join("page/3/index.html").exists());
    }

    #[test]
    fn after_compile_non_multiple_yields_partial_last_page() {
        // 11 posts at per_page=5 → 3 pages: 5 + 5 + 1.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);

        PaginationPlugin::with_per_page(5)
            .after_compile(&ctx)
            .unwrap();

        assert!(site.join("page/2/index.html").exists());
        assert!(site.join("page/3/index.html").exists());
        assert!(!site.join("page/4/index.html").exists());

        let page3 = fs::read_to_string(site.join("page/3/index.html")).unwrap();
        let li_count = page3.matches("<li>").count();
        assert_eq!(li_count, 1, "last page should have 1 entry:\n{page3}");
    }

    #[test]
    fn after_compile_per_page_one_yields_one_page_per_post() {
        // per_page=1 boundary: 5 posts → 5 pages → /page/2 .. /page/5.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 5);

        PaginationPlugin::with_per_page(1)
            .after_compile(&ctx)
            .unwrap();

        for n in 2..=5 {
            assert!(
                site.join(format!("page/{n}/index.html")).exists(),
                "page/{n}/ should exist"
            );
        }
        assert!(!site.join("page/6/index.html").exists());
    }

    // -------------------------------------------------------------------
    // after_compile — sort order
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_sorts_entries_by_date_descending() {
        // Posts written out of order — newest must appear first on
        // page 1 (which is unwritten by this plugin), so the remainder
        // on page 2 must be the *oldest* entries.
        let (_tmp, site, meta, ctx) = make_layout();
        // Write posts with dates that are NOT in filename order:
        // file `a` → 2026-01-01 (oldest)
        // file `z` → 2026-01-11 (newest)
        let dates = [
            ("a", "2026-01-01"),
            ("m", "2026-01-05"),
            ("z", "2026-01-11"),
            ("b", "2026-01-02"),
            ("y", "2026-01-10"),
            ("c", "2026-01-03"),
            ("x", "2026-01-09"),
            ("d", "2026-01-04"),
            ("w", "2026-01-08"),
            ("e", "2026-01-06"),
            ("f", "2026-01-07"),
        ];
        for (name, date) in dates {
            write_sidecar(&meta, name, &format!("Post {name}"), date);
        }

        PaginationPlugin::with_per_page(10)
            .after_compile(&ctx)
            .unwrap();

        // 11 entries / 10 per page → page 2 has the single OLDEST entry.
        let page2 = fs::read_to_string(site.join("page/2/index.html")).unwrap();
        assert!(
            page2.contains("2026-01-01"),
            "page 2 should contain the oldest entry:\n{page2}"
        );
        assert!(
            !page2.contains("2026-01-11"),
            "page 2 should NOT contain the newest entry:\n{page2}"
        );
    }

    // -------------------------------------------------------------------
    // after_compile — HTML structure & navigation
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_emits_doctype_lang_and_charset() {
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        PaginationPlugin::default().after_compile(&ctx).unwrap();

        let html = fs::read_to_string(site.join("page/2/index.html")).unwrap();
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("<meta charset=\"utf-8\">"));
    }

    #[test]
    fn after_compile_emits_pagination_nav_landmark() {
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        PaginationPlugin::default().after_compile(&ctx).unwrap();

        let html = fs::read_to_string(site.join("page/2/index.html")).unwrap();
        assert!(html.contains("<nav aria-label=\"Pagination\">"));
    }

    #[test]
    fn after_compile_page_two_prev_link_points_at_root() {
        // The "previous" link from page 2 must point to "/" — page 1
        // is the home/root, not /page/1/. Guards line 129-130.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        PaginationPlugin::default().after_compile(&ctx).unwrap();

        let html = fs::read_to_string(site.join("page/2/index.html")).unwrap();
        assert!(
            html.contains(r#"<a href="/" rel="prev">"#),
            "page 2's prev should point to root:\n{html}"
        );
    }

    #[test]
    fn after_compile_page_three_prev_link_points_at_page_two() {
        // Beyond page 2 the prev link uses /page/N-1/ form (line 132).
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        PaginationPlugin::with_per_page(5)
            .after_compile(&ctx)
            .unwrap();

        let html = fs::read_to_string(site.join("page/3/index.html")).unwrap();
        assert!(
            html.contains(r#"<a href="/page/2/" rel="prev">"#),
            "page 3's prev should point to /page/2/:\n{html}"
        );
    }

    #[test]
    fn after_compile_last_page_has_no_next_link() {
        // The Next link is omitted on the final page — guards
        // the `if let Some(next)` branch at line 159.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        PaginationPlugin::with_per_page(5)
            .after_compile(&ctx)
            .unwrap();

        let last = fs::read_to_string(site.join("page/3/index.html")).unwrap();
        assert!(
            !last.contains(r#"rel="next""#),
            "last page must not emit a Next link:\n{last}"
        );
    }

    #[test]
    fn after_compile_middle_page_has_both_prev_and_next() {
        // 16 posts / per_page=5 → 4 pages. Page 2 and page 3 are both
        // "middle" — assert they have BOTH prev and next.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 16);
        PaginationPlugin::with_per_page(5)
            .after_compile(&ctx)
            .unwrap();

        let page3 = fs::read_to_string(site.join("page/3/index.html")).unwrap();
        assert!(page3.contains(r#"rel="prev""#));
        assert!(page3.contains(r#"rel="next""#));
    }

    #[test]
    fn after_compile_renders_time_element_per_entry() {
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        PaginationPlugin::default().after_compile(&ctx).unwrap();

        let html = fs::read_to_string(site.join("page/2/index.html")).unwrap();
        assert!(
            html.contains("<time>2026-01-01</time>"),
            "page 2 should render a <time> element:\n{html}"
        );
    }

    #[test]
    fn after_compile_idempotent_overwrites_existing_pages() {
        // Re-running must not error and must leave the page directories
        // intact. Guards against any future use of `create_new`.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        let plugin = PaginationPlugin::default();
        plugin.after_compile(&ctx).expect("first run");
        plugin.after_compile(&ctx).expect("second run");
        assert!(site.join("page/2/index.html").exists());
    }

    // -------------------------------------------------------------------
    // collect_json_files — recursion + filtering
    // -------------------------------------------------------------------

    #[test]
    fn collect_json_files_returns_empty_for_missing_directory() {
        // Non-existent path: the inner `is_dir()` check at line 183
        // means we just `continue`, ending with an empty Vec — no Err.
        let dir = tempdir().expect("tempdir");
        let result =
            collect_json_files(&dir.path().join("does-not-exist")).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn collect_json_files_returns_empty_for_empty_directory() {
        let dir = tempdir().expect("tempdir");
        let result = collect_json_files(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn collect_json_files_filters_non_json_extensions() {
        // Only `.json` files are returned. The `is_some_and` filter at
        // line 191 must reject `.txt`, `.md`, extensionless files, etc.
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("a.json"), "{}").unwrap();
        fs::write(dir.path().join("b.txt"), "x").unwrap();
        fs::write(dir.path().join("c.md"), "x").unwrap();
        fs::write(dir.path().join("noext"), "x").unwrap();

        let result = collect_json_files(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].file_name().unwrap(),
            std::ffi::OsStr::new("a.json")
        );
    }

    #[test]
    fn collect_json_files_recurses_into_subdirectories() {
        // Walks nested directories — guards line 189-190 (push subdir
        // onto stack).
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("top.json"), "{}").unwrap();
        fs::write(dir.path().join("a").join("mid.json"), "{}").unwrap();
        fs::write(nested.join("deep.json"), "{}").unwrap();

        let result = collect_json_files(dir.path()).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn collect_json_files_returns_results_sorted() {
        // The `files.sort()` at line 196 must yield deterministic output.
        let dir = tempdir().expect("tempdir");
        for name in ["zebra.json", "apple.json", "mango.json"] {
            fs::write(dir.path().join(name), "{}").unwrap();
        }
        let result = collect_json_files(dir.path()).unwrap();
        let names: Vec<&str> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["apple.json", "mango.json", "zebra.json"]);
    }

    // -------------------------------------------------------------------
    // I/O error propagation
    // -------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_walk_error_from_unreadable_meta_subdir() {
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, _site, meta, ctx) = make_layout();
        let locked = meta.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let result = PaginationPlugin::default().after_compile(&ctx);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(result.is_err(), "unreadable meta subdir must be an Err");
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_skips_unreadable_sidecar_file() {
        use std::os::unix::fs::PermissionsExt;

        // An unreadable sidecar takes the `.ok()?` None path in
        // parse_page_entry and is silently skipped; the remaining
        // dated posts still paginate.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        let locked = meta.join("locked.meta.json");
        fs::write(&locked, r#"{"title": "L", "date": "2026-02-01"}"#).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let result = PaginationPlugin::default().after_compile(&ctx);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644))
            .unwrap();
        result.expect("unreadable sidecar must be skipped, not fatal");
        assert!(site.join("page/2/index.html").exists());
        assert!(!site.join("page/3/index.html").exists());
    }

    #[test]
    fn after_compile_create_dir_failure_when_page_is_a_file() {
        // A regular file occupying site/page makes create_dir_all fail,
        // which must propagate through write_pagination_page's `?`.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        fs::write(site.join("page"), "not a directory").unwrap();

        let err = PaginationPlugin::default()
            .after_compile(&ctx)
            .expect_err("create_dir_all over a file must fail");
        let msg = format!("{err:?}");
        assert!(msg.contains("Io"), "expected Io error, got: {msg}");
    }

    #[test]
    fn after_compile_write_failure_when_index_html_is_a_directory() {
        // page/2/index.html pre-created as a directory: create_dir_all
        // succeeds (page/2 exists) but the fs::write must fail.
        let (_tmp, site, meta, ctx) = make_layout();
        write_n_dated_posts(&meta, 11);
        fs::create_dir_all(site.join("page/2/index.html")).unwrap();

        let err = PaginationPlugin::default()
            .after_compile(&ctx)
            .expect_err("write over a directory must fail");
        let msg = format!("{err:?}");
        assert!(msg.contains("index.html"), "path context expected: {msg}");
    }
}
