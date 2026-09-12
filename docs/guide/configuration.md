<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Configuration

SSG is configured via a TOML file (`ssg.toml`), CLI flags, or environment variables.

## ssg.toml Fields

```toml
# Required
site_name = "mysite"
content_dir = "content"
output_dir = "public"
template_dir = "templates"

# Site metadata
base_url = "https://example.com"
site_title = "My Site"
site_description = "A fast static site"
language = "en"

# Optional: dev server directory
serve_dir = "public"

# Optional: i18n (see i18n guide)
[i18n]
default_locale = "en"
locales = ["en", "fr", "de"]
url_prefix = "sub_path"   # or "sub_domain"
```

### Field Reference

| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `site_name` | String | Yes | Project name, used by `--new` scaffolding |
| `content_dir` | Path | Yes | Directory containing Markdown content |
| `output_dir` | Path | Yes | Directory for generated output |
| `template_dir` | Path | Yes | Directory containing Tera templates |
| `serve_dir` | Path | No | Directory served by the dev server |
| `base_url` | URL | Yes | Canonical base URL for the site |
| `site_title` | String | Yes | Displayed in templates and meta tags |
| `site_description` | String | Yes | Used in meta description and feeds |
| `language` | String | Yes | BCP 47 language code (e.g. `en`, `fr`) |
| `i18n` | Table | No | Multi-locale configuration (see [i18n](i18n.md)) |

## CLI Flag Overrides

CLI flags override `ssg.toml` values:

```sh
ssg -f ssg.toml -o dist   # overrides output_dir to "dist"
```

See [CLI Reference](cli.md) for all flags.

## Environment Variables

| Variable | Default | Description |
| :--- | :--- | :--- |
| `SSG_HOST` | `127.0.0.1` | Dev server bind address. Set to `0.0.0.0` for WSL2 or Codespaces. |
| `SSG_PORT` | `3000` | Dev server port |

## JSON Schema

SSG can generate a JSON Schema for the configuration format using the `schema` module. This enables editor autocompletion and validation in IDEs that support JSON Schema for TOML (e.g. via `taplo`).

The schema covers all top-level fields and the `[i18n]` section.

## Configuration Precedence

1. **CLI flags** (highest priority)
2. **Configuration file** (`ssg.toml` via `-f`)
3. **Built-in defaults** (lowest priority)

## Validation

SSG validates all configuration at startup:

- Paths are checked for safety (no `..` traversal, no symlinks)
- URLs are validated for correct format
- Config file size is capped to prevent abuse
- Missing required directories produce clear error messages

## Example: Minimal Config

```toml
site_name = "blog"
content_dir = "content"
output_dir = "public"
template_dir = "templates"
base_url = "https://blog.example.com"
site_title = "My Blog"
site_description = "Thoughts and tutorials"
language = "en"
```

## Listings

A listing is a named, filtered, paginated view of your dated pages, with
its own URL space. Without any, a site gets what it always got: every
dated page at `/page/N/`, newest first.

```toml
[[listings]]
name     = "archive"        # /archive/ and /archive/page/N/
title    = "Archive"
per_page = 20

[[listings]]
name     = "rust"
title    = "Writing about Rust"
tag      = "rust"
after    = "2026-01-01"
by_year  = true             # also /rust/2026/, /rust/2025/, …
```

| Field | Effect |
|---|---|
| `name` | URL segment and directory. Required. |
| `title` | Heading. Defaults to `name`. |
| `per_page` | Items per page. Defaults to the plugin's setting. |
| `tag`, `category`, `topic` | Only pages carrying that term. Case-insensitive. |
| `language` | Only pages in that language. |
| `after`, `before` | Date bounds, inclusive, `YYYY-MM-DD`. |
| `by_year` | Also emit `/{name}/{year}/` for each year present. |

Filters are optional and combine with AND, so a listing with none of them
is every dated page under a name you chose. Page 1 is written at
`/{name}/`, not `/{name}/page/1/`.

A listing matching no pages writes nothing and says so on the console —
an empty directory is a poor way to discover a typo in a filter. An
unknown field in a `[[listings]]` section is an error rather than being
ignored, for the same reason: a silently dropped filter produces a
listing that looks right and lists the wrong pages.

There is deliberately no custom-predicate field. A predicate is code, and
a configuration file that grows an expression language has usually taken
a wrong turn; a site that needs one can write a plugin, which is the
supported way to run code during a build.

## Next Steps

- [Content](content.md) — frontmatter and Markdown authoring
- [i18n](i18n.md) — multi-locale configuration
- [CLI Reference](cli.md) — all flags and environment variables
