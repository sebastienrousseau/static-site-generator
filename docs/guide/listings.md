<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Listings

A listing is a named, filtered, paginated archive. You declare it in
config. SSG writes it during the build.

Before listings, pagination produced one unnamed sequence of pages. You
could have `/page/2/`, but you could not have `/rust/page/2/` beside
`/2025/page/2/`. Listings fix that.

## A first listing

```toml
[[listings]]
name = "archive"
title = "Everything, newest first"
per_page = 20
```

That writes `/archive/` and `/archive/page/2/` onwards. Every dated page
is included, because no filter is set.

`name` is both the URL segment and the directory name. It is the only
required field.

## Filters

Each filter narrows the set. Combining them narrows it further, so a
listing with a `tag` and a `language` holds only pages carrying both.

| Field | Keeps pages that |
| :--- | :--- |
| `tag` | carry this tag |
| `category` | carry this category |
| `topic` | carry this topic |
| `language` | are in this language |
| `after` | are dated on or after `YYYY-MM-DD` |
| `before` | are dated on or before `YYYY-MM-DD` |

```toml
[[listings]]
name = "rust"
title = "Rust articles"
tag = "rust"
per_page = 10

[[listings]]
name = "2025"
title = "Published in 2025"
after = "2025-01-01"
before = "2025-12-31"
```

## Year archives

Set `by_year` and SSG also writes `/{name}/{year}/` for each year that
has pages:

```toml
[[listings]]
name = "archive"
by_year = true
```

## A typo is an error

The config refuses fields it does not know. Write `tags` instead of
`tag` and the build stops with a message naming the field.

This is deliberate. A silently ignored filter produces a listing that
looks right and holds the wrong pages, and you may not notice for
months.

## Fields

| Field | Type | Default |
| :--- | :--- | :--- |
| `name` | string | required |
| `title` | string | falls back to `name` |
| `per_page` | integer | plugin default |
| `tag` | string | no filter |
| `category` | string | no filter |
| `topic` | string | no filter |
| `language` | string | no filter |
| `after` | `YYYY-MM-DD` | no filter |
| `before` | `YYYY-MM-DD` | no filter |
| `by_year` | boolean | `false` |

## Scale

Pagination reads front matter once and paginates in a single pass, so
page count does not drive the cost. `tests/listings_scale.rs` builds a
10,000-item corpus and fails if the run takes longer than the budget.

## See also

- [Topics](topics.md) — curated pillar pages over the same content
- [Content](content.md) — the front matter these filters read
