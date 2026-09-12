<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# ssg-i18n

Locale negotiation, hreflang link generation and language-switcher
markup for multi-locale static sites — framework-agnostic.

This crate is part of the [SSG](https://crates.io/crates/ssg) workspace
but has **zero dependency** on SSG itself (no `Plugin` trait, no
`SsgError`, no file I/O). Every function takes `&str`/slices and returns
owned data, so it drops into the build pipeline of any Rust site or app
generator — [Leptos](https://leptos.dev),
[Dioxus](https://dioxuslabs.com), [Yew](https://yew.rs), a hand-rolled
SSG, or an edge worker doing content negotiation at request time.

Documentation lives on [docs.rs](https://docs.rs/ssg-i18n) and the
canonical README for the wider workspace is the [repository
root](https://github.com/sebastienrousseau/static-site-generator#readme).

## Why

The rules for multi-locale sites are small, fiddly and easy to get
subtly wrong: `Accept-Language` is quality-ordered and may name a region
you do not publish; `hreflang` sets must be reciprocal and carry an
`x-default`; a locale prefix belongs in the path *or* the host but not
both, and the root locale usually carries no prefix at all. Getting any
of these wrong is invisible in the browser and costly in search.

Isolating them here means they are testable without a site, and reusable
by tools that are not SSG.

## Installation

```toml
[dependencies]
ssg-i18n = "0.0.62"
```

## Content negotiation

```rust
use ssg_i18n::{negotiate_locale, parse_accept_language};

let preferred = parse_accept_language("fr-CA,fr;q=0.9,en;q=0.5");
let available = vec!["en".to_string(), "fr".to_string()];

// `fr-CA` is not published, so the base language wins over lower-q `en`.
assert_eq!(negotiate_locale(&preferred, &available, "en"), "fr");
```

`parse_accept_language` sorts by `q` descending (stable, so equal
qualities keep header order) and drops the `*` wildcard.
`negotiate_locale` tries an exact match first, then the base language,
and falls back to the default you pass.

## URL construction

```rust
use ssg_i18n::{build_url, UrlPrefixStrategy};

// Locale in the path.
assert_eq!(
    build_url("https://example.com", "fr", "about/", &UrlPrefixStrategy::SubPath, None),
    "https://example.com/fr/about/"
);

// Locale in the host.
assert_eq!(
    build_url("https://example.com", "fr", "about/", &UrlPrefixStrategy::SubDomain, None),
    "https://fr.example.com/about/"
);

// The root locale is served unprefixed.
assert_eq!(
    build_url("https://example.com", "en", "about/", &UrlPrefixStrategy::SubPath, Some("en")),
    "https://example.com/about/"
);
```

## hreflang and the language switcher

`build_hreflang_links` takes the translation-matrix row for a page — a
map of locale to that locale's own (possibly translated) path — and
emits the reciprocal `<link rel="alternate">` set including
`x-default`.

`generate_lang_switcher_html` renders switcher markup for a page;
`generate_lang_switcher_html_with_self_lang` is the variant that links
each entry to that locale's own path rather than the current path under
a different prefix.

For templates that want to place the switcher themselves,
`LANG_SWITCHER_ATTR` marks a placeholder element and
`find_lang_switcher_element` returns its byte span:

```rust
use ssg_i18n::{find_lang_switcher_element, LANG_SWITCHER_ATTR};

let html = format!("<nav {LANG_SWITCHER_ATTR}></nav>");
assert!(find_lang_switcher_element(&html).is_some());

// A placeholder the author has filled in is content, not a slot.
let filled = format!("<nav {LANG_SWITCHER_ATTR}>hand-written</nav>");
assert!(find_lang_switcher_element(&filled).is_none());
```

## Design notes

- **No parsing dependency.** The markup helpers are byte-span scans over
  `&str`, not a DOM pass — the caller keeps whatever HTML pipeline it
  already has.
- **Pure functions.** No global state, no file I/O, no framework
  coupling. `serde` is used only to deserialise [`I18nConfig`] from a
  site's configuration.

## License

Dual-licensed under [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0)
or [MIT](https://opensource.org/licenses/MIT), at your option.
