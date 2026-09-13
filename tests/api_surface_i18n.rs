//! #588: the extraction of `ssg-i18n` must not change ssg's public API.
//!
//! The nine items that were crate-private before the move had to become
//! `pub` in `ssg-i18n` to cross the crate boundary. Re-exporting them
//! publicly here would widen ssg's API silently, so they are re-exported
//! `pub(crate)`. This file is the positive half of that claim; the
//! negative half cannot be written as a passing test (naming a private
//! item does not compile), so it is asserted by `compile_fail` doctests
//! in the crate instead.

#[allow(unused_imports)]
use ssg::i18n::{
    generate_lang_switcher_html, negotiate_locale, parse_accept_language,
    I18nConfig, UrlPrefixStrategy,
};

/// The five items that were public before the extraction are still
/// reachable from outside the crate — this file importing them is the
/// assertion.
#[test]
fn previously_public_items_are_still_public() {
    let parsed = parse_accept_language("fr;q=0.9,en");
    assert_eq!(parsed.first().map(String::as_str), Some("en"));
    assert_eq!(negotiate_locale(&parsed, &["fr".to_string()], "fr"), "fr");
    let cfg = I18nConfig {
        default_locale: "en".to_string(),
        locales: vec!["en".to_string()],
        url_prefix: UrlPrefixStrategy::SubPath,
    };
    assert_eq!(cfg.default_locale, "en");
    let html = generate_lang_switcher_html(
        &["en".to_string(), "fr".to_string()],
        "en",
        "about/",
        "https://example.com",
        &UrlPrefixStrategy::SubPath,
    );
    assert!(html.contains("fr"), "{html}");
}
