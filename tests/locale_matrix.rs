// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Locale matrix (#465).
//!
//! The test suite verified English and nothing else, so a bug that only
//! shows up in Japanese or Arabic had nothing to catch it. This builds
//! one site carrying pages in eleven locales across five scripts —
//! Latin, CJK, Hangul, Arabic and Hebrew — and asserts the properties
//! that a locale can break, for every locale at once.
//!
//! The matrix is data, not repetition: adding a locale is one row, and
//! every assertion then covers it.
//!
//! ## What is asserted, and what is not
//!
//! Asserted here: front matter survives the round trip in every script,
//! the page is emitted at its locale path, `<html lang>` carries the
//! page's own language, and non-ASCII titles and bodies come out
//! byte-for-byte intact.
//!
//! Not asserted here: canonical URLs, `og:` tags and hreflang. Those
//! come from plugins that need site configuration, and `compile_site`
//! builds without it — a test that ran them through this path would be
//! asserting their absence. They are covered in the SEO plugin's own
//! tests, and text direction is covered by
//! `seo_plugin::tests::a_right_to_left_language_marks_the_html_element`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// One row of the matrix.
struct Locale {
    /// BCP-47 tag, and the content subdirectory it lives in.
    code: &'static str,
    /// Title in the locale's own script.
    title: &'static str,
    /// A sentence of body text in the locale's own script.
    body: &'static str,
    /// Whether the script is written right to left.
    rtl: bool,
}

/// Eleven locales across five scripts.
///
/// The list is #465's: Latin (en, fr, de, es, it, pt), CJK (ja, zh),
/// Hangul (ko), Arabic (ar) and Hebrew (he). The two right-to-left
/// entries are the point of the last column.
const LOCALES: &[Locale] = &[
    Locale {
        code: "en",
        title: "Hello world",
        body: "A sentence of body text.",
        rtl: false,
    },
    Locale {
        code: "fr",
        title: "Bonjour le monde",
        body: "Une phrase de texte français.",
        rtl: false,
    },
    Locale {
        code: "de",
        title: "Hallo Welt",
        body: "Ein Satz mit Grüßen und Umlauten.",
        rtl: false,
    },
    Locale {
        code: "es",
        title: "Hola mundo",
        body: "Una frase con eñe y acentuación.",
        rtl: false,
    },
    Locale {
        code: "it",
        title: "Ciao mondo",
        body: "Una frase di testo italiano perché sì.",
        rtl: false,
    },
    Locale {
        code: "pt",
        title: "Olá mundo",
        body: "Uma frase com acentuação portuguesa.",
        rtl: false,
    },
    Locale {
        code: "ja",
        title: "こんにちは世界",
        body: "これは日本語の本文です。",
        rtl: false,
    },
    Locale {
        code: "zh",
        title: "你好世界",
        body: "这是一段中文正文。",
        rtl: false,
    },
    Locale {
        code: "ko",
        title: "안녕하세요 세계",
        body: "이것은 한국어 본문입니다.",
        rtl: false,
    },
    Locale {
        code: "ar",
        title: "مرحبا بالعالم",
        body: "هذا نص تجريبي باللغة العربية.",
        rtl: true,
    },
    Locale {
        code: "he",
        title: "שלום עולם",
        body: "זהו טקסט לדוגמה בעברית.",
        rtl: true,
    },
];

/// Builds the whole matrix as one site and returns its output root.
///
/// One build, not eleven: a per-locale build would not catch a locale
/// leaking into its neighbour, which is the failure this shape is for.
fn build_matrix() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (content, build, site, template) = (
        tmp.path().join("content"),
        tmp.path().join("build"),
        tmp.path().join("site"),
        tmp.path().join("templates"),
    );
    for d in [&content, &build, &site, &template] {
        fs::create_dir_all(d).expect("mkdir");
    }

    // Templates come from the checkout; without them there is nothing
    // to render into and the test would assert on an empty directory.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_tpl = workspace.join("examples/templates/en");
    // Not a skip. These templates are committed, so their absence is a
    // broken checkout — and a suite that quietly passes when it cannot
    // build anything is worse than one that fails.
    assert!(
        src_tpl.is_dir(),
        "missing {} — every assertion below would pass vacuously",
        src_tpl.display()
    );
    for entry in fs::read_dir(&src_tpl).expect("read templates").flatten() {
        let _ = fs::copy(entry.path(), template.join(entry.file_name()));
    }

    for loc in LOCALES {
        let dir = content.join(loc.code);
        fs::create_dir_all(&dir).expect("mkdir locale");
        let md = format!(
            "---\n\
             title: \"{title}\"\n\
             date: \"2026-01-15T09:00:00+00:00\"\n\
             description: \"{title}\"\n\
             language: \"{code}\"\n\
             layout: \"page\"\n\
             permalink: \"https://example.com/{code}/\"\n\
             charset: \"utf-8\"\n\
             hreflang: \"{code}\"\n\
             id: \"https://example.com\"\n\
             ---\n\n\
             # {title}\n\n{body}\n",
            title = loc.title,
            code = loc.code,
            body = loc.body,
        );
        fs::write(dir.join("index.md"), md).expect("write page");
    }

    ssg::compile_site(&build, &content, &site, &template).expect("compile");
    (tmp, site)
}

fn page_for(site: &Path, code: &str) -> Option<String> {
    let p = site.join(code).join("index.html");
    fs::read_to_string(p).ok()
}

#[test]
fn every_locale_emits_a_page() {
    let (_tmp, site) = build_matrix();
    let missing: Vec<&str> = LOCALES
        .iter()
        .filter(|l| page_for(&site, l.code).is_none())
        .map(|l| l.code)
        .collect();
    assert!(missing.is_empty(), "no page emitted for: {missing:?}");
}

/// Every page must declare its own language, not the site default.
#[test]
fn every_locale_declares_its_own_language() {
    let (_tmp, site) = build_matrix();
    let mut wrong = Vec::new();
    for loc in LOCALES {
        let Some(html) = page_for(&site, loc.code) else {
            continue;
        };
        let expected = format!(r#"lang="{}""#, loc.code);
        if !html.contains(&expected) {
            let got = html
                .split_once("<html")
                .and_then(|(_, r)| r.split_once('>'))
                .map_or_else(
                    || "<none>".to_string(),
                    |(t, _)| t.trim().to_string(),
                );
            wrong.push(format!("{}: wanted {expected}, <html{got}>", loc.code));
        }
    }
    assert!(
        wrong.is_empty(),
        "wrong <html lang>:\n  {}",
        wrong.join("\n  ")
    );
}

/// Non-ASCII titles must survive the round trip byte-for-byte.
///
/// This is the assertion that catches an encoding bug: a title that is
/// mojibake, entity-escaped or truncated fails here even though the
/// page builds and looks structurally fine.
#[test]
fn non_ascii_titles_survive_the_round_trip() {
    let (_tmp, site) = build_matrix();
    let mut lost = Vec::new();
    for loc in LOCALES {
        let Some(html) = page_for(&site, loc.code) else {
            continue;
        };
        if !html.contains(loc.title) {
            lost.push(loc.code);
        }
    }
    assert!(lost.is_empty(), "title did not survive for: {lost:?}");
}

#[test]
fn non_ascii_body_text_survives_the_round_trip() {
    let (_tmp, site) = build_matrix();
    let mut lost = Vec::new();
    for loc in LOCALES {
        let Some(html) = page_for(&site, loc.code) else {
            continue;
        };
        if !html.contains(loc.body) {
            lost.push(loc.code);
        }
    }
    assert!(lost.is_empty(), "body text did not survive for: {lost:?}");
}

/// A locale's content must not appear on another locale's page.
///
/// The one-build shape exists for this: eleven separate builds could
/// not detect a page picking up its neighbour's text.
#[test]
fn locales_do_not_leak_into_each_other() {
    let (_tmp, site) = build_matrix();
    let mut leaks = Vec::new();
    for loc in LOCALES {
        let Some(html) = page_for(&site, loc.code) else {
            continue;
        };
        // Only the main content region: the language switcher in the
        // bundled chrome legitimately names every locale.
        let body = html
            .split_once("<main")
            .and_then(|(_, r)| r.split_once("</main>"))
            .map_or(String::new(), |(m, _)| m.to_string());
        if body.is_empty() {
            continue;
        }
        for other in LOCALES {
            if other.code != loc.code && body.contains(other.body) {
                leaks.push(format!(
                    "{} carries {} body text",
                    loc.code, other.code
                ));
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "locale leakage:\n  {}",
        leaks.join("\n  ")
    );
}

/// The matrix must actually span the scripts it claims to.
///
/// Without this the file could quietly shrink to English and every
/// other test above would still pass.
#[test]
fn the_matrix_spans_the_scripts_it_claims() {
    assert!(
        LOCALES.len() >= 11,
        "the matrix has shrunk to {} locales",
        LOCALES.len()
    );
    assert_eq!(
        LOCALES.iter().filter(|l| l.rtl).count(),
        2,
        "both right-to-left locales must be present"
    );
    for (script, code) in [
        ("CJK", "ja"),
        ("Han", "zh"),
        ("Hangul", "ko"),
        ("Arabic", "ar"),
        ("Hebrew", "he"),
    ] {
        assert!(
            LOCALES.iter().any(|l| l.code == code),
            "{script} locale {code} missing from the matrix"
        );
    }
    // Every row must carry text outside ASCII, or it is not testing
    // what this file exists to test.
    for loc in LOCALES.iter().filter(|l| l.code != "en") {
        assert!(
            !loc.title.is_ascii() || !loc.body.is_ascii(),
            "{} carries only ASCII — it cannot catch an encoding bug",
            loc.code
        );
    }
}
