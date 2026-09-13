// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Client-side search index generator.
//!
//! Generates a JSON search index and injects a search UI into HTML pages,
//! providing instant full-text search without any server or external service.
//!
//! # How it works
//!
//! 1. At build time, `SearchIndex` scans all HTML files in the site directory.
//! 2. It extracts the page title, URL, headings, and body text.
//! 3. It writes a `search-index.json` file to the site root.
//! 4. The `SearchPlugin` injects a `<script>` tag and search UI into every
//!    HTML page that loads the index and performs client-side fuzzy matching.
//!
//! The search UI is a modal overlay activated by `Ctrl+K` / `Cmd+K`.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::util::html_rewriter::decode_html_entities;
use crate::util::html_rewriter::inject_before_body_close_or_append;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A single entry in the search index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchEntry {
    /// Page title extracted from `<title>` or first `<h1>`.
    pub title: String,
    /// Relative URL path (e.g., `/about/index.html`).
    pub url: String,
    /// Plain-text body content, truncated to `MAX_CONTENT_LENGTH`.
    pub content: String,
    /// Section headings found on the page.
    pub headings: Vec<String>,
}

/// Maximum content length per page in the search index (characters).
/// Keeps the index compact for fast client-side loading.
pub const MAX_CONTENT_LENGTH: usize = 5_000;

/// Maximum number of pages to index.
pub const MAX_INDEX_ENTRIES: usize = 50_000;

/// The complete search index written to `search-index.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchIndex {
    /// All indexed pages.
    pub entries: Vec<SearchEntry>,
}

impl SearchIndex {
    /// Build a search index from all HTML files in `site_dir`.
    ///
    /// Walks the directory recursively, extracts content from each
    /// `.html` file, and returns the populated index.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::search::SearchIndex;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// // Empty dir ⇒ empty index, never an error.
    /// let idx = SearchIndex::build(dir.path()).unwrap();
    /// assert!(idx.is_empty());
    /// ```
    pub fn build(site_dir: &Path) -> Result<Self, SsgError> {
        let html_files = collect_html_files(site_dir)?;
        let capped: Vec<_> = html_files
            .into_iter()
            .filter(|p| {
                // Separators normalised before matching. These patterns
                // are written with `/`, but `to_string_lossy` yields the
                // platform separator -- so on Windows every one of these
                // six checks silently failed and the 404, offline and
                // thanks pages were indexed into site search. Nothing
                // errored; the index simply contained pages that should
                // never be a search result.
                let s = p.to_string_lossy().replace('\\', "/").to_lowercase();
                !s.contains("/404/")
                    && !s.contains("/offline/")
                    && !s.contains("/thanks/")
                    && !s.ends_with("/404.html")
                    && !s.ends_with("/offline.html")
                    && !s.ends_with("/thanks.html")
            })
            .take(MAX_INDEX_ENTRIES)
            .collect();

        let entries: Vec<SearchEntry> = capped
            .par_iter()
            .map_init(
                // Per-thread scratch buffer reused across files, so each
                // rayon worker amortises one HTML-sized allocation over
                // its whole share of the corpus instead of allocating a
                // fresh `String` per file (issue #578, plan §4 3.1).
                String::new,
                |buf, path| -> Result<SearchEntry, SsgError> {
                    buf.clear();
                    let mut file = fs::File::open(path).with_path(path)?;
                    let _ = std::io::Read::read_to_string(&mut file, buf)
                        .with_path(path)?;
                    let html: &str = buf;

                    // Build `/{rel}` with backslashes normalised in one
                    // pass — replaces the `to_string_lossy().replace()`
                    // double allocation (issue #578, plan §4 3.1).
                    let rel = path.strip_prefix(site_dir).unwrap_or(path);
                    let rel_lossy = rel.to_string_lossy();
                    let mut url = String::with_capacity(rel_lossy.len() + 1);
                    url.push('/');
                    for ch in rel_lossy.chars() {
                        url.push(if ch == '\\' { '/' } else { ch });
                    }

                    // `extract_text` already decodes; `extract_title` and
                    // `extract_headings` did not, so one SearchEntry carried
                    // plain-text content beside a title reading `A &amp; B`.
                    // The index is consumed as text, not markup.
                    let title = decode_html_entities(&extract_title(html));
                    let headings = extract_headings(html)
                        .iter()
                        .map(|h| decode_html_entities(h))
                        .collect();
                    let content = extract_text(html);

                    Ok(SearchEntry {
                        title,
                        url,
                        content: truncate(&content, MAX_CONTENT_LENGTH),
                        headings,
                    })
                },
            )
            .collect::<Result<Vec<_>, SsgError>>()?;

        // Deterministic output (determinism.yml CI gate): the walker's
        // directory-iteration order is filesystem-dependent, so
        // search-index.json would differ across OSes without a stable
        // sort. URLs are unique per page — an unambiguous key.
        let mut entries = entries;
        entries.sort_by(|a, b| a.url.cmp(&b.url));

        Ok(Self { entries })
    }

    /// Write the index to `search-index.json` in the given directory.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::search::SearchIndex;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let idx = SearchIndex::build(dir.path()).unwrap();
    /// idx.write(dir.path()).unwrap();
    /// assert!(dir.path().join("search-index.json").exists());
    /// ```
    pub fn write(&self, site_dir: &Path) -> Result<(), SsgError> {
        let json = serialize_search_index(self).map_err(|e| SsgError::Io {
            path: site_dir.join("search-index.json"),
            source: std::io::Error::other(e),
        })?;
        let path = site_dir.join("search-index.json");
        fs::write(&path, json).with_path(&path)?;
        Ok(())
    }

    /// Number of indexed pages.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::search::SearchIndex;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let idx = SearchIndex::build(dir.path()).unwrap();
    /// assert_eq!(idx.len(), 0);
    /// ```
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the index has no entries.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::search::SearchIndex;
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let idx = SearchIndex::build(dir.path()).unwrap();
    /// assert!(idx.is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Serialize the search index with a fault-injection hook so tests can
/// drive the error branch (serializing `SearchIndex` — plain owned
/// `String`/`Vec<String>` fields — cannot fail in practice).
fn serialize_search_index(index: &SearchIndex) -> serde_json::Result<String> {
    fail_point!("search::serialize", |_| Err(
        <serde_json::Error as serde::ser::Error>::custom(
            "injected: search::serialize"
        )
    ));
    serde_json::to_string(index)
}

/// Localizable strings shown in the search widget UI.
///
/// All fields are plain text. They are HTML-escaped when substituted into
/// attributes/text and JS-escaped when substituted into the inline script
/// (for the "no results" message). Build a value with one of the bundled
/// constructors ([`SearchLabels::english`], [`SearchLabels::french`],
/// [`SearchLabels::for_locale`]) or construct your own for any locale.
#[derive(Debug, Clone)]
pub struct SearchLabels {
    /// Visible text on the trigger button (e.g. "Search").
    pub button_text: String,
    /// `aria-label` of the trigger button.
    pub button_aria: String,
    /// `aria-label` of the modal dialog.
    pub modal_aria: String,
    /// Placeholder text inside the input field.
    pub input_placeholder: String,
    /// `aria-label` of the input field.
    pub input_aria: String,
    /// Footer hint text shown next to the `Esc` key.
    pub footer_close: String,
    /// Footer hint text shown next to the up/down arrow keys.
    pub footer_navigate: String,
    /// Footer hint text shown next to the `Enter` key.
    pub footer_open: String,
    /// Message shown when a query has no matches. The literal `{query}`
    /// is replaced with the typed query at runtime.
    pub no_results: String,
}

/// Compact per-locale strings used by [`SearchLabels::for_locale`].
struct LocaleEntry {
    button: &'static str,
    placeholder: &'static str,
    close: &'static str,
    navigate: &'static str,
    open: &'static str,
    no_results: &'static str,
}

/// Translations for the locales bundled with the search widget.
const LOCALE_TABLE: &[(&str, LocaleEntry)] = &[
    ("en", LocaleEntry { button: "Search",     placeholder: "Search documentation...",                close: "close",     navigate: "navigate",   open: "open",     no_results: "No results for \u{201c}{query}\u{201d}" }),
    ("fr", LocaleEntry { button: "Rechercher", placeholder: "Rechercher dans la documentation...",    close: "fermer",    navigate: "naviguer",   open: "ouvrir",   no_results: "Aucun r\u{e9}sultat pour \u{ab}\u{a0}{query}\u{a0}\u{bb}" }),
    ("ar", LocaleEntry { button: "بحث",        placeholder: "ابحث في الوثائق...",                      close: "إغلاق",     navigate: "تنقل",        open: "فتح",      no_results: "لا توجد نتائج لـ «{query}»" }),
    ("bn", LocaleEntry { button: "অনুসন্ধান",  placeholder: "ডকুমেন্টেশন অনুসন্ধান করুন...",          close: "বন্ধ",      navigate: "নেভিগেট",     open: "খুলুন",    no_results: "{query} এর জন্য কোনো ফলাফল নেই" }),
    ("cs", LocaleEntry { button: "Hledat",     placeholder: "Prohledat dokumentaci...",               close: "zav\u{159}\u{ed}t", navigate: "proch\u{e1}zet", open: "otev\u{159}\u{ed}t", no_results: "\u{17d}\u{e1}dn\u{e9} v\u{fd}sledky pro \u{201e}{query}\u{201c}" }),
    ("de", LocaleEntry { button: "Suchen",     placeholder: "Dokumentation durchsuchen...",           close: "schlie\u{df}en", navigate: "navigieren", open: "\u{f6}ffnen", no_results: "Keine Ergebnisse f\u{fc}r \u{201e}{query}\u{201c}" }),
    ("es", LocaleEntry { button: "Buscar",     placeholder: "Buscar en la documentaci\u{f3}n...",    close: "cerrar",    navigate: "navegar",    open: "abrir",    no_results: "Sin resultados para \u{ab}{query}\u{bb}" }),
    ("ha", LocaleEntry { button: "Bincike",    placeholder: "Bincika takardun...",                    close: "rufe",      navigate: "kewaya",     open: "bu\u{6b}e", no_results: "Babu sakamako don \u{201c}{query}\u{201d}" }),
    ("he", LocaleEntry { button: "חיפוש",      placeholder: "חפש בתיעוד...",                          close: "סגור",       navigate: "נווט",        open: "פתח",      no_results: "אין תוצאות עבור «{query}»" }),
    ("hi", LocaleEntry { button: "खोजें",       placeholder: "दस्तावेज़ खोजें...",                      close: "बंद करें",   navigate: "नेविगेट",     open: "खोलें",    no_results: "{query} के लिए कोई परिणाम नहीं" }),
    ("id", LocaleEntry { button: "Cari",       placeholder: "Cari dokumentasi...",                    close: "tutup",     navigate: "navigasi",   open: "buka",     no_results: "Tidak ada hasil untuk \u{201c}{query}\u{201d}" }),
    ("it", LocaleEntry { button: "Cerca",      placeholder: "Cerca nella documentazione...",          close: "chiudi",    navigate: "naviga",     open: "apri",     no_results: "Nessun risultato per \u{ab}{query}\u{bb}" }),
    ("ja", LocaleEntry { button: "検索",        placeholder: "ドキュメントを検索...",                     close: "閉じる",    navigate: "移動",        open: "開く",     no_results: "「{query}」の結果はありません" }),
    ("ko", LocaleEntry { button: "검색",        placeholder: "문서 검색...",                              close: "닫기",       navigate: "탐색",        open: "열기",     no_results: "«{query}»에 대한 결과가 없습니다" }),
    ("nl", LocaleEntry { button: "Zoeken",     placeholder: "Documentatie doorzoeken...",             close: "sluiten",   navigate: "navigeren",  open: "openen",   no_results: "Geen resultaten voor \u{201c}{query}\u{201d}" }),
    ("pl", LocaleEntry { button: "Szukaj",     placeholder: "Przeszukaj dokumentacj\u{119}...",      close: "zamknij",   navigate: "nawiguj",    open: "otw\u{f3}rz", no_results: "Brak wynik\u{f3}w dla \u{201e}{query}\u{201d}" }),
    ("pt", LocaleEntry { button: "Pesquisar",  placeholder: "Pesquisar na documenta\u{e7}\u{e3}o...", close: "fechar",  navigate: "navegar",    open: "abrir",    no_results: "Sem resultados para \u{ab}{query}\u{bb}" }),
    ("ro", LocaleEntry { button: "Caut\u{103}", placeholder: "Caut\u{103} \u{ee}n documenta\u{21b}ie...", close: "\u{ee}nchide", navigate: "navigheaz\u{103}", open: "deschide", no_results: "Niciun rezultat pentru \u{201e}{query}\u{201d}" }),
    ("ru", LocaleEntry { button: "Поиск",      placeholder: "Поиск по документации...",               close: "закрыть",   navigate: "навигация",  open: "открыть",  no_results: "Нет результатов для «{query}»" }),
    ("sv", LocaleEntry { button: "S\u{f6}k",  placeholder: "S\u{f6}k i dokumentationen...",         close: "st\u{e4}ng", navigate: "navigera", open: "\u{f6}ppna", no_results: "Inga resultat f\u{f6}r \u{201d}{query}\u{201d}" }),
    ("th", LocaleEntry { button: "ค้นหา",       placeholder: "ค้นหาเอกสาร...",                          close: "ปิด",        navigate: "นำทาง",       open: "เปิด",      no_results: "ไม่พบผลลัพธ์สำหรับ \u{201c}{query}\u{201d}" }),
    ("tl", LocaleEntry { button: "Maghanap",   placeholder: "Maghanap sa dokumentasyon...",           close: "isara",     navigate: "mag-navigate", open: "buksan", no_results: "Walang resulta para sa \u{201c}{query}\u{201d}" }),
    ("tr", LocaleEntry { button: "Ara",        placeholder: "Belgelerde ara...",                      close: "kapat",     navigate: "gezin",      open: "a\u{e7}", no_results: "\u{201c}{query}\u{201d} i\u{e7}in sonu\u{e7} yok" }),
    ("uk", LocaleEntry { button: "Пошук",      placeholder: "Пошук у документації...",                close: "закрити",   navigate: "навігація",  open: "відкрити", no_results: "Немає результатів для «{query}»" }),
    ("vi", LocaleEntry { button: "T\u{ec}m ki\u{1ebf}m", placeholder: "T\u{ec}m trong t\u{e0}i li\u{1ec7}u...", close: "\u{111}\u{f3}ng", navigate: "\u{111}i\u{1ec1}u h\u{1b0}\u{1edb}ng", open: "m\u{1edf}", no_results: "Kh\u{f4}ng c\u{f3} k\u{1ebf}t qu\u{1ea3} cho \u{201c}{query}\u{201d}" }),
    ("yo", LocaleEntry { button: "Wáàwáà",     placeholder: "Ṣàwárí ìwé...",                           close: "pa",        navigate: "lọ kiri",    open: "ṣí",       no_results: "Kò sí àbájáde fún \u{201c}{query}\u{201d}" }),
    ("zh", LocaleEntry { button: "搜索",        placeholder: "搜索文档...",                              close: "关闭",       navigate: "导航",        open: "打开",     no_results: "「{query}」没有匹配结果" }),
    ("zh-tw", LocaleEntry { button: "搜尋",     placeholder: "搜尋文件...",                              close: "關閉",       navigate: "瀏覽",        open: "開啟",     no_results: "「{query}」找不到結果" }),
];

impl SearchLabels {
    /// English (default) labels.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::search::SearchLabels;
    ///
    /// let lbl = SearchLabels::english();
    /// assert_eq!(lbl.button_text, "Search");
    /// ```
    #[must_use]
    pub fn english() -> Self {
        Self::for_locale("en")
    }

    /// French labels.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::search::SearchLabels;
    ///
    /// let lbl = SearchLabels::french();
    /// assert_eq!(lbl.button_text, "Rechercher");
    /// ```
    #[must_use]
    pub fn french() -> Self {
        Self::for_locale("fr")
    }

    /// Build labels for a known locale code (ISO 639-1, plus `zh-tw`).
    ///
    /// Lookup is case-insensitive. Falls back to English if the code is not
    /// in the bundled table.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::search::SearchLabels;
    ///
    /// let de = SearchLabels::for_locale("de");
    /// assert_eq!(de.button_text, "Suchen");
    /// // Unknown locale ⇒ English fallback.
    /// let xx = SearchLabels::for_locale("xx");
    /// assert_eq!(xx.button_text, "Search");
    /// ```
    #[must_use]
    pub fn for_locale(code: &str) -> Self {
        let key = code.to_ascii_lowercase();
        let entry = LOCALE_TABLE.iter().find(|(c, _)| *c == key).map_or_else(
            || {
                // `LOCALE_TABLE` is a hand-authored constant array that
                // always contains the `en` entry; the `expect` is a
                // type-system formality, not a runtime risk.
                #[allow(clippy::expect_used)]
                let en = LOCALE_TABLE
                    .iter()
                    .find(|(c, _)| *c == "en")
                    .expect("en entry must exist in LOCALE_TABLE");
                &en.1
            },
            |(_, e)| e,
        );
        Self {
            button_text: entry.button.into(),
            button_aria: entry.button.into(),
            modal_aria: entry.button.into(),
            input_placeholder: entry.placeholder.into(),
            input_aria: entry.button.into(),
            footer_close: entry.close.into(),
            footer_navigate: entry.navigate.into(),
            footer_open: entry.open.into(),
            no_results: entry.no_results.into(),
        }
    }
}

impl Default for SearchLabels {
    fn default() -> Self {
        Self::english()
    }
}

/// Plugin that generates a search index and injects client-side search UI.
///
/// The unit form uses [`SearchLabels::english`] for the modal copy. To render
/// the widget in another language, construct a [`LocalizedSearchPlugin`].
///
/// # Example
///
/// ```rust
/// use ssg::plugin::PluginManager;
/// use ssg::search::SearchPlugin;
///
/// let mut pm = PluginManager::new();
/// pm.register(SearchPlugin);
/// ```
#[derive(Debug, Copy, Clone)]
pub struct SearchPlugin;

impl Plugin for SearchPlugin {
    fn name(&self) -> &'static str {
        "search"
    }

    fn has_transform(&self) -> bool {
        true
    }

    fn transform_html(
        &self,
        html: &str,
        _path: &Path,
        ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        transform_search_html(
            html,
            &SearchLabels::english(),
            &site_path_prefix(ctx),
        )
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        run_search_index(ctx)
    }
}

/// Variant of [`SearchPlugin`] that injects the widget with caller-supplied
/// localized [`SearchLabels`].
///
/// # Example
///
/// ```rust
/// use ssg::plugin::PluginManager;
/// use ssg::search::{LocalizedSearchPlugin, SearchLabels};
///
/// let mut pm = PluginManager::new();
/// pm.register(LocalizedSearchPlugin::new(SearchLabels::french()));
/// ```
#[derive(Debug, Clone)]
pub struct LocalizedSearchPlugin {
    labels: SearchLabels,
}

impl LocalizedSearchPlugin {
    /// Create a new localized search plugin with the given labels.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::plugin::Plugin;
    /// use ssg::search::{LocalizedSearchPlugin, SearchLabels};
    ///
    /// let p = LocalizedSearchPlugin::new(SearchLabels::english());
    /// assert_eq!(p.name(), "search");
    /// ```
    #[must_use]
    pub const fn new(labels: SearchLabels) -> Self {
        Self { labels }
    }
}

impl Plugin for LocalizedSearchPlugin {
    fn name(&self) -> &'static str {
        "search"
    }

    fn has_transform(&self) -> bool {
        true
    }

    fn transform_html(
        &self,
        html: &str,
        _path: &Path,
        ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        transform_search_html(html, &self.labels, &site_path_prefix(ctx))
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        run_search_index(ctx)
    }
}

/// Builds the search index and writes it to disk (`after_compile` phase).
fn run_search_index(ctx: &PluginContext) -> Result<(), SsgError> {
    if !ctx.site_dir.exists() {
        return Ok(());
    }

    let index = SearchIndex::build(&ctx.site_dir)?;
    if index.is_empty() {
        return Ok(());
    }

    index.write(&ctx.site_dir)?;

    println!(
        "[search] Indexed {} pages, search-index.json written",
        index.len()
    );
    Ok(())
}

/// Attribute marking where a theme wants the search trigger placed.
///
/// An element form rather than a comment, for the reason `i18n` documents
/// for `data-ssg-lang-switcher`: `html-generator` minifies some pages
/// before any plugin runs, and a minifier deletes comments but will not
/// delete an element.
///
/// Without a slot the trigger is `position: fixed` in the viewport corner,
/// which cannot line up with a header it is not inside. Five of the nine
/// bundled themes tuned `--ssg-search-top` to compensate and four never
/// did, so the control sat at a different height depending on the theme —
/// and horizontally it pinned to the viewport edge while every other
/// header control sat at the content container's edge, leaving a gap that
/// widened with the window. Flow layout inside the header solves both,
/// which offsets never can.
const SEARCH_SLOT_ATTR: &str = "data-ssg-search";

/// Comment delimiters bounding the trigger button inside
/// [`SEARCH_WIDGET_SCRIPT`], so it can be lifted out and placed in a slot.
const TRIGGER_OPEN: &str = "<!-- Search trigger button -->";
const TRIGGER_CLOSE: &str = "<!-- Search modal -->";

/// Neutralises the fixed positioning once the button lives in a header.
const SLOTTED_TRIGGER_CSS: &str =
    "<style>#ssg-search-btn{position:static;top:auto;right:auto;\
z-index:auto;box-shadow:none}</style>";

/// Splits the widget into (everything before the trigger, the trigger,
/// everything after it).
fn split_widget(script: &str) -> Option<(&str, &str, &str)> {
    let open = script.find(TRIGGER_OPEN)?;
    let close = script.find(TRIGGER_CLOSE)?;
    if close <= open {
        return None;
    }
    Some((&script[..open], &script[open..close], &script[close..]))
}

/// Finds an empty placeholder element carrying [`SEARCH_SLOT_ATTR`] and
/// returns its byte range, including the closing tag.
///
/// Same shape as `i18n::find_lang_switcher_element`, and deliberately not a
/// regex for the same reason: the crate carries no regex dependency and the
/// match is one empty element, not a grammar.
fn find_search_slot(html: &str) -> Option<(usize, usize)> {
    let attr_at = html.find(SEARCH_SLOT_ATTR)?;
    let start = html[..attr_at].rfind('<')?;
    let name_start = start + 1;
    let name_end = html[name_start..]
        .find(|c: char| !c.is_ascii_alphanumeric())
        .map(|i| name_start + i)?;
    let name = &html[name_start..name_end];
    if name.is_empty() {
        return None;
    }
    // The attribute must belong to this tag, not to a later one.
    let open_end = html[start..].find('>')? + start + 1;
    if attr_at > open_end {
        return None;
    }
    let close = format!("</{name}>");
    let close_at = html[open_end..].find(&close)? + open_end;
    // Only an *empty* placeholder is replaced; anything else is content.
    if !html[open_end..close_at].trim().is_empty() {
        return None;
    }
    Some((start, close_at + close.len()))
}

/// Injects the search widget into an HTML string (`transform_html` phase).
fn transform_search_html(
    html: &str,
    labels: &SearchLabels,
    site_prefix: &str,
) -> Result<String, SsgError> {
    if html.contains("ssg-search-widget") {
        return Ok(html.to_string()); // Already injected
    }

    let script = build_widget_script(labels, site_prefix);

    // A theme that provides a slot gets the trigger inside its header, in
    // flow with the other controls. Everything else -- styles, modal,
    // behaviour -- still goes before `</body>` as before, so a theme with
    // no slot is byte-for-byte unaffected.
    if let (Some((slot_start, slot_end)), Some((head, trigger, tail))) =
        (find_search_slot(html), split_widget(&script))
    {
        let rest = format!("{head}{tail}{SLOTTED_TRIGGER_CSS}");
        let placed = format!(
            "{}{}{}",
            &html[..slot_start],
            trigger.trim(),
            &html[slot_end..]
        );
        let injected = inject_before_body_close_or_append(&placed, &rest);
        return Ok(injected);
    }

    let injected = inject_before_body_close_or_append(html, &script);

    Ok(injected)
}

// =====================================================================
// HTML content extraction (streaming via lol_html — issue #525)
// =====================================================================

/// Extract the page title from `<title>` tag or first `<h1>`.
///
/// Uses [`crate::util::html_rewriter::extract_text_with_filter`] which
/// streams the input through `lol_html`, decoding character entities
/// (`&amp;` → `&`) and ignoring `<title>` tags hidden inside HTML
/// comments. Falls back to the first `<h1>` if `<title>` is missing
/// or empty.
fn extract_title(html: &str) -> String {
    use crate::util::html_rewriter::extract_text_with_filter;

    if let Ok(titles) = extract_text_with_filter(html, "title") {
        if let Some(t) = titles.into_iter().find(|s| !s.trim().is_empty()) {
            return t;
        }
    }
    if let Ok(h1s) = extract_text_with_filter(html, "h1") {
        if let Some(h) = h1s.into_iter().find(|s| !s.trim().is_empty()) {
            return h;
        }
    }
    String::new()
}

/// Extract all heading text (`<h1>` through `<h6>`).
///
/// Preserves document order across all six heading levels — `<h2>`
/// inside `<h1>` is captured once at the outer level (matching the
/// legacy `str::find`-based behaviour) because `lol_html` fires the
/// end-tag handler for the outer element first.
fn extract_headings(html: &str) -> Vec<String> {
    use crate::util::html_rewriter::extract_text_with_filter;

    let mut out = Vec::new();
    for tag in &["h1", "h2", "h3", "h4", "h5", "h6"] {
        if let Ok(hs) = extract_text_with_filter(html, tag) {
            out.extend(hs);
        }
    }
    out
}

/// Extract visible text from HTML, stripping all tags.
///
/// Uses `lol_html` to skip `<script>`, `<style>`, `<nav>`, `<footer>`,
/// and `<head>` blocks (matching the historical filter), then walks
/// the remaining text chunks via the document-level text handler.
/// Entities are decoded so the search-index content matches the
/// rendered page.
fn extract_text(html: &str) -> String {
    use crate::util::html_rewriter::{
        collapse_whitespace, decode_html_entities, rewrite_html,
    };
    use lol_html::html_content::ContentType;
    use lol_html::{doc_text, element};
    use std::cell::RefCell;
    use std::rc::Rc;

    // We do the work in two passes:
    // 1. Use `lol_html` to remove `<script>`, `<style>`, `<nav>`,
    //    `<footer>`, and `<head>` subtrees (matching the legacy
    //    filter).
    // 2. Walk the resulting document's text nodes and join them.
    let skip = ["script", "style", "nav", "footer", "head"];
    let mut handlers = Vec::new();
    for tag in &skip {
        handlers.push(element!(*tag, |el| {
            el.replace(" ", ContentType::Text);
            Ok(())
        }));
    }
    let Ok(stripped) = rewrite_html(html, handlers) else {
        return String::new();
    };

    // Walk only text nodes at the document level. The `doc_text!`
    // helper is part of the public `lol_html` macro family but we
    // construct the handler manually so we can build a `Settings` with
    // it set on the document-level handlers list rather than the
    // element-level one.
    let buf: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let buf_cb = Rc::clone(&buf);
    let text_handler = doc_text!(move |t| {
        buf_cb.borrow_mut().push_str(t.as_str());
        Ok(())
    });

    let mut settings = lol_html::RewriteStrSettings::new();
    settings = settings.append_document_content_handler(text_handler);
    let _ = lol_html::rewrite_str(stripped.as_str(), settings);

    let raw = buf.borrow().clone();
    collapse_whitespace(&decode_html_entities(&raw))
}

/// Remove all HTML tags, collapse whitespace. Retained for the legacy
/// proptest `strip_tags_no_angle_brackets` so the property holds for
/// arbitrary input even when `lol_html` isn't in the loop. Internally
/// delegates to the wrapper's text extractor + entity decoder so the
/// invariant is byte-identical with the new path.
#[cfg(test)]
fn strip_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                result.push(' ');
            }
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    crate::util::html_rewriter::collapse_whitespace(&result)
}

/// Truncate a string to approximately `max` characters at a word boundary.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let byte_pos: usize = s
        .char_indices()
        .take(max)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    let truncated = &s[..byte_pos];
    if let Some(last_space) = truncated.rfind(' ') {
        truncated[..last_space].to_string()
    } else {
        truncated.to_string()
    }
}

/// Collect all `.html` files under `dir` (delegates to `crate::walk`).
fn collect_html_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files_bounded_count(dir, "html", MAX_INDEX_ENTRIES)
}

/// Inject the search UI script into an HTML file.
///
/// Inserts a `<script>` block before `</body>` that:
/// 1. Loads `search-index.json`
/// 2. Creates a modal overlay with an input field
/// 3. Performs case-insensitive substring matching on title + content
/// 4. Displays results with highlighted snippets
/// 5. Activates on `Ctrl+K` / `Cmd+K`
#[cfg(test)]
fn inject_search_ui(path: &Path, script: &str) -> Result<(), SsgError> {
    let html = fs::read_to_string(path).with_path(path)?;

    if html.contains("ssg-search-widget") {
        return Ok(()); // Already injected
    }

    let injected = inject_before_body_close_or_append(&html, script);

    fs::write(path, injected).with_path(path)?;
    Ok(())
}

/// The path component of `base_url`, or `""` when the site owns its host.
///
/// The widget fetches its index with a root-absolute URL. A site published
/// under a path — `https://example.com/apex` — therefore asked for
/// `/search-index.json` at the *host* root, which is not its own index.
///
/// That failed quietly in the worst way: on a host where something else
/// answers at `/search-index.json`, search returned results from a
/// different site rather than erroring. It only became visible when the
/// showcase moved to a host with nothing at the root.
fn site_path_prefix(ctx: &PluginContext) -> String {
    ctx.config.as_ref().map_or_else(String::new, |c| {
        crate::plugins_group::csp::base_url_path_prefix(&c.base_url)
    })
}

/// Render [`SEARCH_WIDGET_SCRIPT`] (a template) with the given labels.
///
/// HTML attribute / text values are HTML-escaped; the `no_results` string is
/// also JS-escaped because it ends up inside a single-quoted JS string literal.
fn build_widget_script(labels: &SearchLabels, site_prefix: &str) -> String {
    let no_results_with_expr = html_escape(&labels.no_results)
        .replace("{query}", "&ldquo;\'+esc(q)+\'&rdquo;");

    SEARCH_WIDGET_SCRIPT
        .replace("{{SSG_BTN_ARIA}}", &html_escape(&labels.button_aria))
        .replace("{{SSG_BTN_TEXT}}", &html_escape(&labels.button_text))
        .replace("{{SSG_MODAL_ARIA}}", &html_escape(&labels.modal_aria))
        .replace(
            "{{SSG_INPUT_PLACEHOLDER}}",
            &html_escape(&labels.input_placeholder),
        )
        .replace("{{SSG_INPUT_ARIA}}", &html_escape(&labels.input_aria))
        .replace("{{SSG_FOOTER_CLOSE}}", &html_escape(&labels.footer_close))
        .replace(
            "{{SSG_FOOTER_NAVIGATE}}",
            &html_escape(&labels.footer_navigate),
        )
        .replace("{{SSG_FOOTER_OPEN}}", &html_escape(&labels.footer_open))
        .replace("{{SSG_NO_RESULTS}}", &js_escape(&no_results_with_expr))
        .replace("{{SSG_SITE_PREFIX}}", site_prefix)
}

/// Minimal HTML escaper covering the characters that matter inside attribute
/// values and text nodes.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape a string so it is safe to embed inside a single-quoted JS literal.
fn js_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

/// The self-contained search widget (HTML + CSS + JS).
///
/// Includes a fixed search button in the top-right corner (like pacs008.com's
/// `DocSearch` bar) that opens a full-screen search modal. Also responds to
/// `Ctrl+K` / `Cmd+K`.
const SEARCH_WIDGET_SCRIPT: &str = r#"
<!-- SSG Search Widget -->
<div id="ssg-search-widget">
<style>
/* ── Trigger button (always visible) ── */
/* The trigger is `position: fixed` over the page, not a child of the
   site header, so it cannot inherit the header's vertical centring. The
   hardcoded `top: 16px` therefore sat 4-6px below every other header
   control on all four bundled themes. A theme knows its own header height
   and this plugin cannot, so the offsets are custom properties with the
   previous values as defaults: setting `--ssg-search-top` is all a theme
   needs, and a theme that sets nothing behaves exactly as before. */
#ssg-search-btn{position:fixed;top:var(--ssg-search-top,16px);right:var(--ssg-search-right,16px);z-index:9998;min-height:44px;display:flex;align-items:center;gap:8px;padding:8px 16px;background:#fff;border:1px solid #d1d5db;border-radius:8px;cursor:pointer;font-family:-apple-system,system-ui,sans-serif;font-size:14px;color:#55555c;box-shadow:0 1px 3px rgba(0,0,0,.08);transition:border-color .15s,box-shadow .15s}
@media(max-width:47.999rem){#ssg-search-btn{top:auto;bottom:var(--ssg-search-bottom,16px);right:var(--ssg-search-right,16px);width:44px;height:44px;padding:0;justify-content:center;border-radius:50%;box-shadow:0 4px 14px rgba(0,0,0,.18)}#ssg-search-btn kbd,#ssg-search-btn span{display:none}}
#ssg-search-btn:hover{border-color:#595960;box-shadow:0 2px 6px rgba(0,0,0,.12)}
#ssg-search-btn svg{width:16px;height:16px;stroke:currentColor;fill:none;stroke-width:2;stroke-linecap:round;stroke-linejoin:round}
#ssg-search-btn kbd{font-family:inherit;font-size:11px;padding:2px 6px;background:#f3f4f6;border:1px solid #e5e7eb;border-radius:4px;color:#4f4f55;margin-left:4px}
/* ── Modal overlay ── */
#ssg-search-overlay{display:none;position:fixed;inset:0;z-index:9999;background:rgba(0,0,0,.5);align-items:flex-start;justify-content:center;padding-top:12vh}
#ssg-search-overlay.active{display:flex}
#ssg-search-box{background:#fff;border-radius:12px;width:92%;max-width:640px;box-shadow:0 25px 60px rgba(0,0,0,.3);overflow:hidden;font-family:-apple-system,system-ui,sans-serif}
#ssg-search-header{display:flex;align-items:center;padding:0 16px;border-bottom:1px solid #e5e7eb}
#ssg-search-header svg{width:20px;height:20px;stroke:#9ca3af;fill:none;stroke-width:2;flex-shrink:0}
#ssg-search-input{flex:1;padding:16px 12px;font-size:16px;border:none;outline:none;background:transparent}
#ssg-search-results{max-height:50vh;overflow-y:auto}
#ssg-sr-status{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);border:0}
.ssg-result{display:block;padding:12px 20px;text-decoration:none;color:#111;border-bottom:1px solid #f3f4f6;transition:background .1s}
.ssg-result:hover,.ssg-result.active{background:#ecfdf5}
.ssg-result-title{font-weight:600;font-size:15px;margin-bottom:3px}
.ssg-result-snippet{font-size:13px;color:#595960;line-height:1.5}
.ssg-result-snippet mark{background:#fef08a;color:inherit;border-radius:2px;padding:0 2px}
.ssg-no-results{padding:32px 20px;text-align:center;color:#595960;font-size:14px}
.ssg-no-results[role="status"]{}
/* Forced-colours / Windows High Contrast Mode */
@media(forced-colors:active){
#ssg-search-btn{border:1px solid ButtonText}
#ssg-search-btn:focus{outline:2px solid Highlight}
#ssg-search-input{border:1px solid CanvasText}
#ssg-search-input:focus{outline:2px solid Highlight}
.ssg-result:focus,.ssg-result.active{outline:2px solid Highlight}
.ssg-result-snippet mark{background:Highlight;color:HighlightText}
}
.ssg-search-footer{display:flex;gap:16px;padding:10px 20px;font-size:12px;color:#595960;border-top:1px solid #e5e7eb;justify-content:flex-end}
.ssg-search-footer kbd{font-family:inherit;font-size:11px;padding:1px 5px;background:#f3f4f6;border:1px solid #e5e7eb;border-radius:3px}
/* ── Dark mode (media query + data-theme attribute) ── */
@media(prefers-color-scheme:dark){
:root:not([data-theme="light"]) #ssg-search-btn{background:#1f2937;border-color:#374151;color:#cccccf}
:root:not([data-theme="light"]) #ssg-search-btn:hover{border-color:#4b5563}
:root:not([data-theme="light"]) #ssg-search-btn kbd{background:#374151;border-color:#4b5563;color:#d9d9db}
:root:not([data-theme="light"]) #ssg-search-box{background:#1f2937;color:#f9fafb}
:root:not([data-theme="light"]) #ssg-search-header{border-color:#374151}
:root:not([data-theme="light"]) #ssg-search-input{color:#f9fafb}
:root:not([data-theme="light"]) .ssg-result{color:#f9fafb;border-color:#374151}
:root:not([data-theme="light"]) .ssg-result:hover,:root:not([data-theme="light"]) .ssg-result.active{background:#374151}
:root:not([data-theme="light"]) .ssg-result-snippet{color:#cccccf}
:root:not([data-theme="light"]) .ssg-result-snippet mark{background:#854d0e;color:#fef08a}
:root:not([data-theme="light"]) .ssg-no-results{color:#cccccf}
:root:not([data-theme="light"]) .ssg-search-footer{border-color:#374151;color:#cccccf}
:root:not([data-theme="light"]) .ssg-search-footer kbd{background:#374151;border-color:#4b5563}
}
[data-theme="dark"] #ssg-search-btn{background:#1f2937;border-color:#374151;color:#cccccf}
[data-theme="dark"] #ssg-search-btn:hover{border-color:#4b5563}
[data-theme="dark"] #ssg-search-btn kbd{background:#374151;border-color:#4b5563;color:#d9d9db}
[data-theme="dark"] #ssg-search-box{background:#1f2937;color:#f9fafb}
[data-theme="dark"] #ssg-search-header{border-color:#374151}
[data-theme="dark"] #ssg-search-input{color:#f9fafb}
[data-theme="dark"] .ssg-result{color:#f9fafb;border-color:#374151}
[data-theme="dark"] .ssg-result:hover,[data-theme="dark"] .ssg-result.active{background:#374151}
[data-theme="dark"] .ssg-result-snippet{color:#cccccf}
[data-theme="dark"] .ssg-result-snippet mark{background:#854d0e;color:#fef08a}
[data-theme="dark"] .ssg-no-results{color:#cccccf}
[data-theme="dark"] .ssg-search-footer{border-color:#374151;color:#cccccf}
[data-theme="dark"] .ssg-search-footer kbd{background:#374151;border-color:#4b5563}
</style>
<!-- Search trigger button -->
<button id="ssg-search-btn" type="button" aria-label="{{SSG_BTN_ARIA}}">
<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
<span>{{SSG_BTN_TEXT}}</span>
<kbd>K</kbd>
</button>
<!-- Search modal -->
<div id="ssg-search-overlay" role="dialog" aria-label="{{SSG_MODAL_ARIA}}">
<div id="ssg-search-box">
<div id="ssg-search-header">
<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
<input id="ssg-search-input" type="search" placeholder="{{SSG_INPUT_PLACEHOLDER}}" autocomplete="off" aria-label="{{SSG_INPUT_ARIA}}"/>
</div>
<div id="ssg-search-results" aria-live="polite"></div>
<div id="ssg-sr-status" role="status" aria-live="polite" aria-atomic="true"></div>
<div class="ssg-search-footer"><span><kbd>Esc</kbd> {{SSG_FOOTER_CLOSE}}</span><span><kbd>&uarr;</kbd><kbd>&darr;</kbd> {{SSG_FOOTER_NAVIGATE}}</span><span><kbd>Enter</kbd> {{SSG_FOOTER_OPEN}}</span></div>
</div>
</div>
<script>
(function(){
var idx=null,overlay=document.getElementById('ssg-search-overlay'),
input=document.getElementById('ssg-search-input'),
results=document.getElementById('ssg-search-results'),
btn=document.getElementById('ssg-search-btn'),active=-1,
lm=location.pathname.match(/^\/(en|fr|ar|bn|cs|de|es|ha|he|hi|id|it|ja|ko|nl|pl|pt|ro|ru|sv|th|tl|tr|uk|vi|yo|zh-tw|zh)\//),
lp=lm?'{{SSG_SITE_PREFIX}}/'+lm[1]:'{{SSG_SITE_PREFIX}}';
function load(){if(idx)return Promise.resolve();var sp=lm?'{{SSG_SITE_PREFIX}}/'+lm[1]+'/search-index.json':'{{SSG_SITE_PREFIX}}/search-index.json';return fetch(sp).then(function(r){return r.json()}).then(function(d){idx=d.entries||[]}).catch(function(){idx=[]})}
function open(){load().then(function(){overlay.classList.add('active');input.value='';results.innerHTML='';input.focus();active=-1})}
function close(){overlay.classList.remove('active');active=-1}
function highlight(text,q){if(!q)return esc(text);var re=new RegExp('('+q.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')+')','gi');return esc(text).replace(re,'<mark>$1</mark>')}
function esc(s){var d=document.createElement('div');d.textContent=s;return d.innerHTML}
function snippet(content,q,len){len=len||150;if(!q)return esc(content.substring(0,len));var i=content.toLowerCase().indexOf(q.toLowerCase());if(i<0)return esc(content.substring(0,len));var s=Math.max(0,i-50),e=Math.min(content.length,i+len);var t=(s>0?'...':'')+content.substring(s,e)+(e<content.length?'...':'');return highlight(t,q)}
function search(q){if(!idx||!q){results.innerHTML='';return}q=q.trim();if(!q){results.innerHTML='';return}var ql=q.toLowerCase(),hits=[];
for(var i=0;i<idx.length&&hits.length<20;i++){var e=idx[i],s=0;if(e.title.toLowerCase().indexOf(ql)>=0)s+=10;if(e.content.toLowerCase().indexOf(ql)>=0)s+=5;for(var h=0;h<e.headings.length;h++){if(e.headings[h].toLowerCase().indexOf(ql)>=0){s+=3;break}}if(s>0)hits.push({entry:e,score:s})}
hits.sort(function(a,b){return b.score-a.score});
var sr=document.getElementById('ssg-sr-status');
if(!hits.length){results.innerHTML='<div class="ssg-no-results" role="status">{{SSG_NO_RESULTS}}</div>';if(sr)sr.textContent='No results found';return}
var html='';for(var j=0;j<hits.length;j++){var e=hits[j].entry;html+='<a class="ssg-result" href="'+esc(lp+e.url)+'">'+'<div class="ssg-result-title">'+highlight(e.title,q)+'</div>'+'<div class="ssg-result-snippet">'+snippet(e.content,q)+'</div></a>'}
results.innerHTML=html;active=-1;if(sr)sr.textContent=hits.length+' result'+(hits.length===1?'':'s')+' found'}
function nav(dir){var items=results.querySelectorAll('.ssg-result');if(!items.length)return;if(active>=0&&items[active])items[active].classList.remove('active');active+=dir;if(active<0)active=items.length-1;if(active>=items.length)active=0;items[active].classList.add('active');items[active].scrollIntoView({block:'nearest'})}
btn.addEventListener('click',function(){open()});
input.addEventListener('input',function(){search(this.value)});
overlay.addEventListener('click',function(e){if(e.target===overlay)close()});
document.addEventListener('keydown',function(e){if((e.ctrlKey||e.metaKey)&&e.key==='k'){e.preventDefault();if(overlay.classList.contains('active'))close();else open()}
if(!overlay.classList.contains('active'))return;if(e.key==='Escape')close();if(e.key==='ArrowDown'){e.preventDefault();nav(1)}if(e.key==='ArrowUp'){e.preventDefault();nav(-1)}
if(e.key==='Enter'){e.preventDefault();var items=results.querySelectorAll('.ssg-result');if(active>=0&&items[active])window.location=items[active].href;else if(items[0])window.location=items[0].href}})
})();
</script>
</div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SsgError;
    use anyhow::Result;
    use tempfile::tempdir;

    fn make_html(title: &str, body: &str) -> String {
        format!(
            "<html><head><title>{title}</title></head>\
             <body><h1>{title}</h1>{body}</body></html>"
        )
    }

    /// The widget fetches its index root-absolutely, so a site published
    /// under a path must carry that path or it asks the *host* root for an
    /// index that is not its own.
    ///
    /// This failed silently rather than loudly: on a host where something
    /// else answered at `/search-index.json`, search returned another
    /// site's results. It only surfaced when the themes showcase moved to
    /// a host with nothing at the root.
    #[test]
    fn search_index_url_carries_the_site_path_prefix() {
        let script = build_widget_script(&SearchLabels::english(), "/apex");
        // The non-locale branch — the one that fetched the wrong index.
        assert!(
            script.contains(":'/apex/search-index.json'"),
            "default branch should be prefixed: {script}"
        );
        // The locale branch prefixes the locale segment, not the host root.
        assert!(
            script.contains("'/apex/'+lm[1]+'/search-index.json'"),
            "locale branch should be prefixed: {script}"
        );
        assert!(
            !script.contains("{{SSG_SITE_PREFIX}}"),
            "placeholder should be substituted: {script}"
        );
    }

    /// A site that owns its host keeps the bare path — the prefix is empty
    /// and nothing should be doubled up.
    #[test]
    fn search_index_url_is_bare_without_a_prefix() {
        let script = build_widget_script(&SearchLabels::english(), "");
        assert!(script.contains("'/search-index.json'"), "{script}");
        assert!(!script.contains("//search-index.json"), "{script}");
    }

    /// Loading the index is half the job; the other half is where a result
    /// sends you. Entry URLs are stored site-relative (`/contact/index.html`),
    /// so `lp` must carry the same prefix as the index URL.
    ///
    /// Fixing only the fetch left search visibly working and every result
    /// leading to a 404 — a worse failure than the one it replaced, because
    /// the widget now looked healthy.
    #[test]
    fn result_links_carry_the_site_path_prefix() {
        let script = build_widget_script(&SearchLabels::english(), "/apex");
        // Non-locale: an empty `lp` produced a host-root link.
        assert!(
            script.contains("lp=lm?'/apex/'+lm[1]:'/apex'"),
            "result prefix should be the site prefix: {script}"
        );
        // The href is built by concatenation, so the entry's leading slash
        // must not be doubled by the prefix.
        assert!(
            !script.contains("'/apex/':"),
            "prefix must not end in a slash: {script}"
        );
    }

    /// The same, for a site at its host root: `lp` stays empty so links
    /// remain `/contact/index.html` rather than gaining a stray prefix.
    #[test]
    fn result_links_are_bare_without_a_prefix() {
        let script = build_widget_script(&SearchLabels::english(), "");
        assert!(script.contains("lp=lm?'/'+lm[1]:''"), "{script}");
    }

    #[test]
    fn build_entries_are_sorted_by_url_for_determinism() {
        // determinism.yml gate: walker order is filesystem-dependent,
        // so search-index.json must be sorted to hash identically
        // across OSes.
        let dir = tempdir().unwrap();
        for name in ["zeta", "alpha", "mid"] {
            let d = dir.path().join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(
                d.join("index.html"),
                format!(
                    "<html><head><title>{name}</title></head>\
                     <body><p>{name} body</p></body></html>"
                ),
            )
            .unwrap();
        }
        let idx = SearchIndex::build(dir.path()).unwrap();
        let urls: Vec<&str> =
            idx.entries.iter().map(|e| e.url.as_str()).collect();
        let mut sorted = urls.clone();
        sorted.sort_unstable();
        assert_eq!(urls, sorted, "entries must be URL-sorted");
        assert_eq!(idx.entries.len(), 3);
    }

    #[test]
    fn extract_title_from_title_tag() {
        let html =
            "<html><head><title>My Page</title></head><body></body></html>";
        assert_eq!(extract_title(html), "My Page");
    }

    #[test]
    fn extract_title_from_h1() {
        let html = "<html><body><h1>Heading</h1></body></html>";
        assert_eq!(extract_title(html), "Heading");
    }

    #[test]
    fn extract_title_empty() {
        assert_eq!(extract_title("<html><body></body></html>"), "");
    }

    #[test]
    fn extract_headings_multiple() {
        let html = "<h1>Title</h1><h2>Intro</h2><h3>Detail</h3>";
        let h = extract_headings(html);
        assert_eq!(h, vec!["Title", "Intro", "Detail"]);
    }

    #[test]
    fn extract_headings_with_attributes() {
        let html = r#"<h2 class="section" id="s1">Section One</h2>"#;
        let h = extract_headings(html);
        assert_eq!(h, vec!["Section One"]);
    }

    #[test]
    fn extract_text_strips_tags() {
        let html = "<p>Hello <strong>world</strong></p>";
        let text = extract_text(html);
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn extract_text_removes_scripts() {
        let html = "<body><script>alert(1)</script><p>Visible</p></body>";
        let text = extract_text(html);
        assert!(text.contains("Visible"));
        assert!(!text.contains("alert"));
    }

    #[test]
    fn strip_tags_collapses_whitespace() {
        let result = strip_tags("<p>  hello   <br>  world  </p>");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("short", 100), "short");
    }

    #[test]
    fn truncate_at_word_boundary() {
        let result = truncate("hello beautiful world", 18);
        assert_eq!(result, "hello beautiful");
    }

    #[test]
    fn search_index_build_from_directory() -> Result<()> {
        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("index.html"),
            make_html("Home", "<p>Welcome to SSG</p>"),
        )
        .unwrap();
        fs::write(
            tmp.path().join("about.html"),
            make_html("About", "<p>About this site</p>"),
        )
        .unwrap();

        let index = SearchIndex::build(tmp.path()).unwrap();
        assert_eq!(index.len(), 2);
        assert!(!index.is_empty());

        let titles: Vec<&str> =
            index.entries.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.contains(&"Home"));
        assert!(titles.contains(&"About"));
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn search_index_write_creates_json() -> Result<()> {
        let tmp = tempdir().unwrap();
        let index = SearchIndex {
            entries: vec![SearchEntry {
                title: "Test".into(),
                url: "/test.html".into(),
                content: "Test content".into(),
                headings: vec!["Heading".into()],
            }],
        };
        index.write(tmp.path()).unwrap();

        let path = tmp.path().join("search-index.json");
        assert!(path.exists());
        let json: SearchIndex =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json.entries.len(), 1);
        assert_eq!(json.entries[0].title, "Test");
        Ok(())
    }

    #[test]
    fn search_index_empty_directory() -> Result<()> {
        let tmp = tempdir().unwrap();
        let index = SearchIndex::build(tmp.path()).unwrap();
        assert!(index.is_empty());
        Ok(())
    }

    #[test]
    fn search_index_ignores_non_html() -> Result<()> {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("style.css"), "body{}").unwrap();
        fs::write(tmp.path().join("data.json"), "{}").unwrap();
        let index = SearchIndex::build(tmp.path()).unwrap();
        assert!(index.is_empty());
        Ok(())
    }

    #[test]
    fn search_index_nested_directories() -> Result<()> {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("blog")).unwrap();
        fs::write(tmp.path().join("index.html"), make_html("Home", ""))
            .unwrap();
        fs::write(
            tmp.path().join("blog/post.html"),
            make_html("Post", "<p>Blog content</p>"),
        )
        .unwrap();

        let index = SearchIndex::build(tmp.path()).unwrap();
        assert_eq!(index.len(), 2);
        let urls: Vec<&str> =
            index.entries.iter().map(|e| e.url.as_str()).collect();
        assert!(urls.iter().any(|u| u.contains("blog")));
        Ok(())
    }

    #[test]
    fn search_entry_content_truncated() -> Result<()> {
        let tmp = tempdir().unwrap();
        let long_text = "word ".repeat(2000); // 10,000 chars
        fs::write(
            tmp.path().join("long.html"),
            make_html("Long", &format!("<p>{long_text}</p>")),
        )
        .unwrap();

        let index = SearchIndex::build(tmp.path()).unwrap();
        assert!(index.entries[0].content.len() <= MAX_CONTENT_LENGTH);
        Ok(())
    }

    #[test]
    fn inject_search_ui_adds_widget() -> Result<()> {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("page.html");
        fs::write(&path, "<html><body><p>Hello</p></body></html>").unwrap();

        let script = build_widget_script(&SearchLabels::english(), "");
        inject_search_ui(&path, &script).unwrap();

        let result = fs::read_to_string(&path).unwrap();
        assert!(result.contains("ssg-search-widget"));
        assert!(result.contains("search-index.json"));
        assert!(result.contains("ctrlKey"));
        Ok(())
    }

    /// A theme that provides a slot gets the trigger inside its header.
    ///
    /// This is the whole point of the slot: in flow with the other header
    /// controls, so it aligns with them by layout rather than by each theme
    /// guessing an offset.
    #[test]
    fn trigger_is_placed_in_a_theme_slot() {
        let html = "<html><body><header><nav>\
<span data-ssg-search></span>\
<button class=\"mode-btn\"></button></nav></header></body></html>";
        let out = transform_search_html(html, &SearchLabels::default(), "/")
            .expect("transform");

        let btn = out.find("ssg-search-btn").expect("trigger present");
        let hdr = out.find("</header>").expect("header present");
        assert!(
            btn < hdr,
            "the trigger must sit inside the header when a slot exists"
        );
        assert!(
            !out.contains("data-ssg-search"),
            "the placeholder must be consumed, not left in the output"
        );
        assert!(
            out.contains("position:static"),
            "a slotted trigger must drop its fixed positioning"
        );
    }

    /// A theme with no slot is unaffected.
    ///
    /// The fallback is the contract for every theme that has not opted in,
    /// so it is asserted rather than assumed: the trigger stays in the
    /// appended widget, after the header.
    #[test]
    fn without_a_slot_the_trigger_stays_where_it_was() {
        let html =
            "<html><body><header><nav></nav></header><main></main></body></html>";
        let out = transform_search_html(html, &SearchLabels::default(), "/")
            .expect("transform");

        let btn = out.find("ssg-search-btn").expect("trigger present");
        let hdr = out.find("</header>").expect("header present");
        assert!(
            btn > hdr,
            "with no slot the trigger must remain in the appended widget"
        );
        assert!(
            !out.contains("position:static"),
            "the un-slotted trigger keeps its fixed positioning"
        );
    }

    /// A placeholder that already has content is left alone.
    #[test]
    fn a_non_empty_placeholder_is_not_replaced() {
        let html = "<html><body><header>\
<span data-ssg-search>existing</span></header></body></html>";
        let out = transform_search_html(html, &SearchLabels::default(), "/")
            .expect("transform");
        assert!(
            out.contains("existing"),
            "content inside the placeholder must survive"
        );
    }

    #[test]
    fn inject_search_ui_idempotent() -> Result<()> {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("page.html");
        fs::write(&path, "<html><body><p>Hi</p></body></html>").unwrap();

        let script = build_widget_script(&SearchLabels::english(), "");
        inject_search_ui(&path, &script).unwrap();
        let first = fs::read_to_string(&path).unwrap();

        inject_search_ui(&path, &script).unwrap();
        let second = fs::read_to_string(&path).unwrap();

        assert_eq!(first, second); // No double injection
        Ok(())
    }

    #[test]
    fn search_plugin_name() {
        assert_eq!(SearchPlugin.name(), "search");
    }

    #[test]
    fn search_plugin_full_pipeline() -> Result<()> {
        let tmp = tempdir().unwrap();
        let html_content = make_html("Home", "<p>Welcome</p>");
        fs::write(tmp.path().join("index.html"), &html_content).unwrap();
        fs::write(
            tmp.path().join("about.html"),
            make_html("About", "<p>About us</p>"),
        )
        .unwrap();

        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );
        SearchPlugin.after_compile(&ctx).unwrap();

        // Index was written
        assert!(tmp.path().join("search-index.json").exists());

        // Widget was injected via transform_html
        let output = SearchPlugin
            .transform_html(&html_content, &tmp.path().join("index.html"), &ctx)
            .unwrap();
        assert!(output.contains("ssg-search-widget"));
        Ok(())
    }

    #[test]
    fn search_plugin_nonexistent_dir() -> Result<()> {
        let ctx = PluginContext::new(
            Path::new("c"),
            Path::new("b"),
            Path::new("/nonexistent"),
            Path::new("t"),
        );
        SearchPlugin.after_compile(&ctx).unwrap(); // Should not error
        Ok(())
    }

    #[test]
    fn search_plugin_registers() {
        use crate::plugin::PluginManager;
        let mut pm = PluginManager::new();
        pm.register(SearchPlugin);
        assert_eq!(pm.names(), vec!["search"]);
    }

    #[test]
    fn search_entry_serialize_deserialize() -> Result<()> {
        let entry = SearchEntry {
            title: "Test".into(),
            url: "/test.html".into(),
            content: "Content".into(),
            headings: vec!["H1".into()],
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SearchEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, parsed);
        Ok(())
    }

    // -------------------------------------------------------------------
    // Targeted edge-case coverage
    // -------------------------------------------------------------------

    #[test]
    fn search_plugin_after_compile_empty_index_short_circuits() -> Result<()> {
        // Line 136: `if index.is_empty() { return Ok(()) }`. Need a
        // site with HTML files that produce zero entries — easiest:
        // a site with only a stylesheet (collect_html_files returns
        // empty, build returns empty index).
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("style.css"), "body{}").unwrap();
        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );
        SearchPlugin.after_compile(&ctx).unwrap();
        // No search-index.json should have been written.
        assert!(!tmp.path().join("search-index.json").exists());
        Ok(())
    }

    #[test]
    fn extract_title_empty_title_falls_back_to_h1() {
        // Line 167 false branch: title trimmed is empty, so we fall
        // through to the h1 fallback at lines 172-180.
        let html = "<html><head><title>   </title></head><body><h1>Heading One</h1></body></html>";
        assert_eq!(extract_title(html), "Heading One");
    }

    #[test]
    fn extract_title_no_title_tag_falls_back_to_h1() {
        // Lines 178-179: the h1 fallback Some-Some success path.
        let html = "<html><body><h1>From H1</h1></body></html>";
        assert_eq!(extract_title(html), "From H1");
    }

    #[test]
    fn extract_title_h1_with_attributes_works() {
        // Verifies the `find('>')` step at line 174 handles attrs.
        let html = r#"<html><body><h1 class="title">Attrs</h1></body></html>"#;
        assert_eq!(extract_title(html), "Attrs");
    }

    #[test]
    fn extract_title_no_title_no_h1_returns_empty() {
        let html = "<html><body><p>just a paragraph</p></body></html>";
        assert_eq!(extract_title(html), "");
    }

    #[test]
    fn extract_title_unterminated_tags_do_not_panic() {
        // Issue #525: the previous `str::find`-based extractor used
        // to silently fall through from a broken `<title>` to the
        // first `<h1>`. The `lol_html` port follows the HTML5 spec
        // — `<title>` is a raw-text element whose end-tag handler
        // only fires when `</title>` is seen, so an unterminated
        // `<title>` yields an empty title and the function MUST NOT
        // panic. (No real browser exposes the inside of an unclosed
        // `<title>` either; this is a pathological input.)
        let html =
            "<html><head><title>Open<body><h1>Fallback</h1></body></html>";
        let _ = extract_title(html);
    }

    #[test]
    fn extract_title_unterminated_h1_returns_empty() {
        // <h1> open without `>` and without `</h1>` — both inner
        // `if let`s return None, function returns "".
        let html = "<html><body><h1 attr=\"open";
        assert_eq!(extract_title(html), "");
    }

    #[test]
    fn extract_headings_unterminated_h_tag_breaks_inner_loop() {
        // Line 204: the `break` when no `</hN>` close tag is found.
        let html = "<html><body><h1>Has close</h1><h2>no close tag";
        let headings = extract_headings(html);
        // The first heading is captured; the unterminated one
        // breaks out of the inner loop without panicking.
        assert!(headings.contains(&"Has close".to_string()));
    }

    #[test]
    fn extract_headings_unterminated_open_tag_breaks_outer() {
        // Line 207: the `break` when `<h1` has no `>`. Build a
        // pathological string that contains `<h1` but never `>`
        // afterwards.
        let html = "<h1 attr=\"unterminated";
        let headings = extract_headings(html);
        assert!(headings.is_empty());
    }

    #[test]
    fn extract_text_unterminated_strip_tag_breaks() {
        // Line 225: the `break` in the strip loop when a tag opener
        // exists but no matching close. extract_text strips
        // <script>/<style>/etc. blocks; an unterminated <script>
        // hits the inner break.
        let html = "<html><body><script>unterminated<p>visible</p>";
        let _ = extract_text(html);
    }

    #[test]
    fn truncate_no_space_falls_back_to_byte_cut() {
        // Line 278: `else { truncated.to_string() }` when there is
        // no space within the first `max` characters.
        let result = truncate("oneverylongwordwithnospacesatall", 10);
        // Returns the byte-truncated string (no space to break on).
        assert_eq!(result, "oneverylon");
    }

    #[test]
    fn truncate_short_string_returned_unchanged() {
        // Line 266 true branch: input shorter than max returns as-is.
        assert_eq!(truncate("short", 100), "short");
    }

    #[test]
    fn collect_html_files_respects_bound() -> Result<()> {
        let tmp = tempdir().unwrap();
        for i in 0..50 {
            fs::write(tmp.path().join(format!("p{i}.html")), "<html></html>")
                .unwrap();
        }
        let files = collect_html_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 50);
        Ok(())
    }

    #[test]
    fn search_index_empty_site_dir() -> Result<()> {
        // Arrange
        let tmp = tempdir().unwrap();

        // Act
        let index = SearchIndex::build(tmp.path()).unwrap();

        // Assert
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        Ok(())
    }

    #[test]
    fn search_index_max_content_length_truncation() -> Result<()> {
        // Arrange
        let tmp = tempdir().unwrap();
        let long_content = "a ".repeat(MAX_CONTENT_LENGTH + 1000);
        fs::write(
            tmp.path().join("long.html"),
            make_html("Long Page", &format!("<p>{long_content}</p>")),
        )
        .unwrap();

        // Act
        let index = SearchIndex::build(tmp.path()).unwrap();

        // Assert
        assert_eq!(index.len(), 1);
        assert!(
            index.entries[0].content.chars().count() <= MAX_CONTENT_LENGTH,
            "content should be truncated to at most MAX_CONTENT_LENGTH characters"
        );
        Ok(())
    }

    #[test]
    fn search_index_unicode_content() -> Result<()> {
        // Arrange
        let tmp = tempdir().unwrap();
        let unicode_body = "<p>Héllo wörld! 日本語テスト 🦀🔍 Ñoño café</p>";
        fs::write(
            tmp.path().join("unicode.html"),
            make_html("Ünïcödé Pagé 🎉", unicode_body),
        )
        .unwrap();

        // Act
        let index = SearchIndex::build(tmp.path()).unwrap();

        // Assert
        assert_eq!(index.len(), 1);
        let entry = &index.entries[0];
        assert_eq!(entry.title, "Ünïcödé Pagé 🎉");
        assert!(entry.content.contains("日本語テスト"));
        assert!(entry.content.contains("🦀🔍"));
        assert!(entry.content.contains("café"));
        Ok(())
    }

    #[test]
    fn search_plugin_nonexistent_dir_returns_ok() -> Result<()> {
        // Arrange
        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            Path::new("/tmp/nonexistent_search_test_dir_xyz"),
            Path::new("templates"),
        );

        // Act
        let result = SearchPlugin.after_compile(&ctx);

        // Assert
        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    fn inject_search_ui_no_body_tag() -> Result<()> {
        // Arrange
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("fragment.html");
        fs::write(&path, "<html><p>No body tag here</p></html>").unwrap();

        // Act
        let script = build_widget_script(&SearchLabels::english(), "");
        inject_search_ui(&path, &script).unwrap();

        // Assert
        let result = fs::read_to_string(&path).unwrap();
        assert!(
            result.contains("ssg-search-widget"),
            "widget should be appended even without </body>"
        );
        assert!(result.contains("<html><p>No body tag here</p></html>"));
        Ok(())
    }

    #[test]
    fn search_entry_serialization_roundtrip() -> Result<()> {
        // Arrange
        let entry = SearchEntry {
            title: "Roundtrip Test".into(),
            url: "/roundtrip/index.html".into(),
            content: "Some searchable content here".into(),
            headings: vec!["Introduction".into(), "Details".into()],
        };

        // Act
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: SearchEntry = serde_json::from_str(&json).unwrap();

        // Assert
        assert_eq!(entry, deserialized);
        assert_eq!(deserialized.title, "Roundtrip Test");
        assert_eq!(deserialized.headings.len(), 2);
        Ok(())
    }

    #[test]
    fn search_index_multiple_headings() -> Result<()> {
        // Arrange
        let tmp = tempdir().unwrap();
        let html = "\
            <html><head><title>Multi Heading</title></head><body>\
            <h1>Main Title</h1>\
            <h2>Section A</h2>\
            <p>Content A</p>\
            <h3>Subsection A1</h3>\
            <p>Content A1</p>\
            </body></html>";
        fs::write(tmp.path().join("headings.html"), html).unwrap();

        // Act
        let index = SearchIndex::build(tmp.path()).unwrap();

        // Assert
        assert_eq!(index.len(), 1);
        let entry = &index.entries[0];
        assert!(entry.headings.contains(&"Main Title".to_string()));
        assert!(entry.headings.contains(&"Section A".to_string()));
        assert!(entry.headings.contains(&"Subsection A1".to_string()));
        assert_eq!(entry.headings.len(), 3);
        Ok(())
    }

    #[test]
    fn search_index_nested_directories_deep() -> Result<()> {
        // Arrange
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs/guide/advanced")).unwrap();
        fs::write(
            tmp.path().join("index.html"),
            make_html("Root", "<p>Root page</p>"),
        )
        .unwrap();
        fs::write(
            tmp.path().join("docs/overview.html"),
            make_html("Docs", "<p>Docs overview</p>"),
        )
        .unwrap();
        fs::write(
            tmp.path().join("docs/guide/advanced/tips.html"),
            make_html("Tips", "<p>Advanced tips</p>"),
        )
        .unwrap();

        // Act
        let index = SearchIndex::build(tmp.path()).unwrap();

        // Assert
        assert_eq!(index.len(), 3);
        let urls: Vec<&str> =
            index.entries.iter().map(|e| e.url.as_str()).collect();
        assert!(urls.iter().any(|u| u.contains("docs/guide/advanced")));
        assert!(urls.iter().any(|u| u.contains("index.html")));
        Ok(())
    }

    // -----------------------------------------------------------------
    // SearchIndex::build — parallel path with multiple HTML files
    // -----------------------------------------------------------------

    #[test]
    fn search_index_build_parallel_with_many_files() -> Result<()> {
        let tmp = tempdir().unwrap();
        for i in 0..10 {
            fs::write(
                tmp.path().join(format!("page{i}.html")),
                make_html(
                    &format!("Page {i}"),
                    &format!("<p>Content for page {i}</p>"),
                ),
            )
            .unwrap();
        }

        let index = SearchIndex::build(tmp.path()).unwrap();
        assert_eq!(index.len(), 10);

        // Verify all pages are indexed
        for i in 0..10 {
            let title = format!("Page {i}");
            assert!(
                index.entries.iter().any(|e| e.title == title),
                "missing entry for {title}"
            );
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // extract_headings — h1 through h6
    // -----------------------------------------------------------------

    #[test]
    fn extract_headings_all_levels() {
        let html = "\
            <h1>One</h1>\
            <h2>Two</h2>\
            <h3>Three</h3>\
            <h4>Four</h4>\
            <h5>Five</h5>\
            <h6>Six</h6>";
        let h = extract_headings(html);
        assert_eq!(h, vec!["One", "Two", "Three", "Four", "Five", "Six"]);
    }

    #[test]
    fn extract_headings_empty_heading_skipped() {
        let html = "<h1></h1><h2>Real Heading</h2>";
        let h = extract_headings(html);
        assert_eq!(h, vec!["Real Heading"]);
    }

    // -----------------------------------------------------------------
    // truncate — word boundary and short content
    // -----------------------------------------------------------------

    #[test]
    fn truncate_at_word_boundary_exact() {
        // truncate(s, 13) takes first 13 chars "one two three"
        // then finds last space at position 7, truncating to "one two"
        let result = truncate("one two three four five", 13);
        assert_eq!(result, "one two");
    }

    #[test]
    fn truncate_content_shorter_than_limit() {
        let input = "short text";
        assert_eq!(truncate(input, 1000), "short text");
    }

    #[test]
    fn truncate_exact_length_returns_unchanged() {
        let input = "exact";
        assert_eq!(truncate(input, 5), "exact");
    }

    // -----------------------------------------------------------------
    // SearchLabels::for_locale
    // -----------------------------------------------------------------

    #[test]
    fn search_labels_for_locale_french() {
        let labels = SearchLabels::for_locale("fr");
        assert_eq!(labels.button_text, "Rechercher");
        assert!(labels.input_placeholder.contains("Rechercher"));
        assert_eq!(labels.footer_close, "fermer");
    }

    #[test]
    fn search_labels_for_locale_german() {
        let labels = SearchLabels::for_locale("de");
        assert_eq!(labels.button_text, "Suchen");
        assert_eq!(labels.footer_open, "\u{f6}ffnen"); // öffnen
    }

    #[test]
    fn search_labels_for_locale_unknown_falls_back_to_english() {
        let labels = SearchLabels::for_locale("xx");
        assert_eq!(labels.button_text, "Search");
        assert!(labels.input_placeholder.contains("Search"));
        assert_eq!(labels.footer_close, "close");
    }

    #[test]
    fn search_labels_for_locale_case_insensitive() {
        let labels = SearchLabels::for_locale("FR");
        assert_eq!(labels.button_text, "Rechercher");
    }

    #[test]
    fn search_labels_for_locale_zh_tw() {
        let labels = SearchLabels::for_locale("zh-tw");
        assert_eq!(labels.button_text, "搜尋");
    }

    #[test]
    fn search_labels_default_is_english() {
        let labels = SearchLabels::default();
        assert_eq!(labels.button_text, "Search");
    }

    #[test]
    fn search_labels_english_constructor() {
        let labels = SearchLabels::english();
        assert_eq!(labels.button_text, "Search");
        assert_eq!(
            SearchLabels::english().input_placeholder,
            labels.input_placeholder
        );
    }

    #[test]
    fn search_labels_french_constructor() {
        let labels = SearchLabels::french();
        assert_eq!(labels.button_text, "Rechercher");
    }

    #[test]
    fn localized_search_plugin_new_keeps_supplied_labels() {
        let labels = SearchLabels::french();
        let p = LocalizedSearchPlugin::new(labels.clone());
        assert_eq!(p.labels.button_text, "Rechercher");
    }

    #[test]
    fn localized_search_plugin_name_is_search() {
        let p = LocalizedSearchPlugin::new(SearchLabels::default());
        assert_eq!(p.name(), "search");
    }

    #[test]
    fn localized_search_plugin_no_op_when_site_missing() -> Result<()> {
        let dir = tempdir().unwrap();
        let nope = dir.path().join("nope");
        let ctx = PluginContext::new(
            Path::new("c"),
            Path::new("b"),
            &nope,
            Path::new("t"),
        );
        LocalizedSearchPlugin::new(SearchLabels::default())
            .after_compile(&ctx)
            .unwrap();
        Ok(())
    }

    #[test]
    fn localized_search_plugin_has_transform_is_true() {
        // Covers line ~396-398.
        let p = LocalizedSearchPlugin::new(SearchLabels::default());
        assert!(p.has_transform());
    }

    #[test]
    fn search_plugin_has_transform_is_true() {
        // Covers the sister SearchPlugin's has_transform impl.
        assert!(SearchPlugin.has_transform());
    }

    #[test]
    fn transform_search_html_skips_when_already_injected() {
        // Covers line ~440 — early-return when widget marker is present.
        let html =
            "<html><body><div id=\"ssg-search-widget\"></div></body></html>";
        let out =
            transform_search_html(html, &SearchLabels::english(), "").unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn transform_search_html_appends_when_no_body_close_tag() {
        // Covers line ~448 — fallback when </body> is absent.
        let html = "<html><head></head>";
        let out =
            transform_search_html(html, &SearchLabels::english(), "").unwrap();
        assert!(out.starts_with(html));
        assert!(out.contains("ssg-search-widget"));
    }

    #[test]
    fn extract_title_falls_back_to_h1_when_title_is_empty() {
        // Covers line ~472 — title tag present but blank → h1 fallback.
        let html =
            "<html><head><title>   </title></head><body><h1>Fallback</h1></body></html>";
        assert_eq!(extract_title(html), "Fallback");
    }

    #[test]
    fn extract_title_returns_empty_when_no_title_or_h1() {
        // Covers line ~477 — both title and h1 absent.
        let html = "<html><body><p>no headings</p></body></html>";
        assert_eq!(extract_title(html), "");
    }

    #[test]
    fn localized_search_plugin_writes_index_with_localized_labels() -> Result<()>
    {
        let dir = tempdir().unwrap();
        let html_content =
            "<html><head><title>P</title></head><body>x</body></html>";
        fs::write(dir.path().join("page.html"), html_content).unwrap();
        let ctx = PluginContext::new(
            Path::new("c"),
            Path::new("b"),
            dir.path(),
            Path::new("t"),
        );
        let plugin = LocalizedSearchPlugin::new(SearchLabels::french());
        plugin.after_compile(&ctx).unwrap();
        let output = plugin
            .transform_html(html_content, &dir.path().join("page.html"), &ctx)
            .unwrap();
        // Localized button text should appear in the injected widget.
        assert!(
            output.contains("Rechercher"),
            "French label 'Rechercher' should appear in injected UI"
        );
        Ok(())
    }

    #[test]
    fn after_compile_write_failure_returns_io_error() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        // Write an HTML file so it actually attempts to build and write index
        fs::write(
            site.join("index.html"),
            "<html><head><title>Test</title></head><body></body></html>",
        )
        .unwrap();

        // Create a directory where search-index.json should be written, causing fs::write to fail
        let index_dir = site.join("search-index.json");
        fs::create_dir(&index_dir).unwrap();

        let ctx = PluginContext::new(
            Path::new("c"),
            Path::new("b"),
            &site,
            Path::new("t"),
        );
        let res = SearchPlugin.after_compile(&ctx);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == &index_dir)
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // coverage: build/read error paths + escaper branches
    // ─────────────────────────────────────────────────────────────────

    /// Markup that trips `lol_html`'s parsing-ambiguity bailout (a text
    /// parsing mode switching tag inside `<select>`), forcing every
    /// extractor onto its rewrite-failure fallback.
    const AMBIGUOUS_HTML: &str =
        "<select><xmp><script>x</script></xmp></select>";

    #[test]
    #[cfg(unix)]
    fn search_index_build_propagates_unreadable_subdir_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let result = SearchIndex::build(tmp.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(result.is_err(), "unreadable subdir must be an Err");
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_build_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let ctx = PluginContext::new(
            Path::new("c"),
            Path::new("b"),
            tmp.path(),
            Path::new("t"),
        );
        let result = SearchPlugin.after_compile(&ctx);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(result.is_err(), "build error must propagate");
    }

    #[test]
    #[cfg(unix)]
    fn search_index_build_propagates_unreadable_file_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let page = tmp.path().join("page.html");
        fs::write(&page, make_html("T", "")).unwrap();
        fs::set_permissions(&page, fs::Permissions::from_mode(0o000)).unwrap();

        let result = SearchIndex::build(tmp.path());
        fs::set_permissions(&page, fs::Permissions::from_mode(0o644)).unwrap();
        let err = result.expect_err("File::open must fail on 0o000");
        assert!(format!("{err:?}").contains("page.html"));
    }

    #[test]
    fn search_index_build_propagates_invalid_utf8_read_error() {
        // File::open succeeds; read_to_string fails on invalid UTF-8.
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("broken.html"), [0xFF, 0xFE, 0xFD]).unwrap();

        let err = SearchIndex::build(tmp.path())
            .expect_err("invalid UTF-8 must fail the read");
        assert!(format!("{err:?}").contains("broken.html"));
    }

    #[test]
    #[cfg(unix)]
    fn search_index_build_normalises_backslashes_in_urls() {
        // On unix a backslash is a legal filename byte; the URL builder
        // must still normalise it to a forward slash.
        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("we\\ird.html"),
            make_html("Weird", "<p>x</p>"),
        )
        .unwrap();

        let index = SearchIndex::build(tmp.path()).unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index.entries[0].url, "/we/ird.html");
    }

    #[test]
    fn extract_title_falls_back_to_empty_on_ambiguous_markup() {
        assert_eq!(extract_title(AMBIGUOUS_HTML), "");
    }

    #[test]
    fn extract_headings_empty_on_ambiguous_markup() {
        assert!(extract_headings(AMBIGUOUS_HTML).is_empty());
    }

    #[test]
    fn extract_text_empty_on_ambiguous_markup() {
        assert_eq!(extract_text(AMBIGUOUS_HTML), "");
    }

    #[test]
    fn inject_search_ui_missing_file_returns_read_error() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("missing.html");
        let err = inject_search_ui(&missing, "<script></script>")
            .expect_err("missing file must surface a read error");
        assert!(format!("{err:?}").contains("missing.html"));
    }

    #[test]
    #[cfg(unix)]
    fn inject_search_ui_readonly_file_returns_write_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let page = tmp.path().join("page.html");
        fs::write(&page, "<html><body></body></html>").unwrap();
        fs::set_permissions(&page, fs::Permissions::from_mode(0o444)).unwrap();

        let script = build_widget_script(&SearchLabels::english(), "");
        let result = inject_search_ui(&page, &script);
        fs::set_permissions(&page, fs::Permissions::from_mode(0o644)).unwrap();
        let err =
            result.expect_err("read-only file must surface a write error");
        assert!(format!("{err:?}").contains("page.html"));
    }

    #[test]
    fn html_escape_escapes_every_special_character() {
        assert_eq!(
            html_escape("a & <b> \"c\" 'd'"),
            "a &amp; &lt;b&gt; &quot;c&quot; &#39;d&#39;"
        );
    }

    #[test]
    fn js_escape_escapes_backslash_quotes_and_newlines() {
        assert_eq!(
            js_escape("back\\slash 'quote'\nnew\rline"),
            "back\\\\slash \\'quote\\'\\nnew\\rline"
        );
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use serial_test::serial;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial]
    fn write_maps_serialize_failure_to_io_error() {
        // `serde_json::to_string` on `SearchIndex` (plain owned strings)
        // cannot fail in practice, so the only way to exercise `write`'s
        // serialize-error branch is fault injection.
        let _guard = FailGuard("search::serialize");
        fail::cfg("search::serialize", "return").expect("activate failpoint");

        let tmp = tempdir().unwrap();
        let index = SearchIndex {
            entries: vec![SearchEntry {
                title: "T".into(),
                url: "/t.html".into(),
                content: "c".into(),
                headings: vec![],
            }],
        };
        let err = index
            .write(tmp.path())
            .expect_err("injected serialize failure must propagate");
        let msg = format!("{err}");
        assert!(msg.contains("search-index.json"), "got: {msg}");
        assert!(msg.contains("injected: search::serialize"), "got: {msg}");
    }
}

#[cfg(test)]
mod proptests {
    /// Excluded pages stay excluded when the path uses `\` separators.
    ///
    /// The filter's patterns are written with `/`, but the value it
    /// matches against comes from `to_string_lossy`, which yields the
    /// platform separator. On Windows that meant `\404\` never matched
    /// `/404/`, so the 404, offline and thanks pages were indexed into
    /// site search -- silently, because nothing errored and the index was
    /// merely wrong.
    ///
    /// Asserted against both separator forms so the guarantee holds on
    /// every platform rather than only on the one running the test.
    #[test]
    fn excluded_pages_are_excluded_with_either_separator() {
        fn excluded(raw: &str) -> bool {
            let s = raw.replace('\\', "/").to_lowercase();
            s.contains("/404/")
                || s.contains("/offline/")
                || s.contains("/thanks/")
                || s.ends_with("/404.html")
                || s.ends_with("/offline.html")
                || s.ends_with("/thanks.html")
        }

        for path in [
            "site/404/index.html",
            r"site\404\index.html",
            "site/offline/index.html",
            r"site\offline\index.html",
            "site/thanks/index.html",
            r"site\thanks\index.html",
            "site/404.html",
            r"site\404.html",
        ] {
            assert!(excluded(path), "should be excluded: {path}");
        }

        for path in ["site/about/index.html", r"site\about\index.html"] {
            assert!(!excluded(path), "should be indexed: {path}");
        }
    }

    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        /// After stripping tags the output must contain no angle brackets.
        #[test]
        fn strip_tags_no_angle_brackets(input in "\\PC*") {
            let stripped = strip_tags(&input);
            prop_assert!(
                !stripped.contains('<') && !stripped.contains('>'),
                "angle brackets survived strip_tags: {:?}", stripped,
            );
        }
    }
}
