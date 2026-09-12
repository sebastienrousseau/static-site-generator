<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Content

SSG generates sites from Markdown files with YAML/TOML/JSON frontmatter.

## Frontmatter

Every Markdown file starts with a frontmatter block. YAML is the most common format:

```markdown
---
title: "Getting Started"
date: 2026-04-13
description: "An introduction to SSG"
schema: "post"
draft: false
tags: ["tutorial", "ssg"]
---

Your content here.
```

### Standard Frontmatter Fields

| Field | Type | Description |
| :--- | :--- | :--- |
| `title` | String | Page title, used in `<title>` and OG tags |
| `date` | Date | Publication date (ISO 8601: `YYYY-MM-DD`) |
| `description` | String | Meta description for SEO |
| `schema` | String | Content schema name for validation |
| `draft` | Bool | If `true`, excluded unless `--drafts` is passed |
| `tags` | List | Tags for taxonomy generation |
| `categories` | List | Categories for taxonomy generation |
| `topic_clusters` | String | Comma-separated topics this page belongs to — see [Topic pages](#topic-pages) |
| `template` | String | Override the default template for this page |
| `language` | String | Page language (BCP 47), overrides site default |
| `translation_key` | String | Pairs this page with its translations in other locales — see [i18n](i18n.md#translated-slugs). Required only when the translated pages have different slugs |

### Topic pages

`topic_clusters` groups pages under a subject rather than a keyword. A
page joins one or more topics by naming them:

```yaml
topic_clusters: "payments, post-quantum-cryptography"
```

Each topic gets `/topics/{slug}/`, and `/topics/` lists them all. Pages
self-assign, so adding an article to a topic is part of writing the
article rather than an edit to a central list — which is what makes two
articles mergeable without conflicting.

#### Curating a topic

Assignment is all a topic needs. What it cannot express is editorial
judgement: the title a human would choose, what the topic is *about*,
and which page should lead. `_data/topics.toml` supplies that, and every
field is optional:

```toml
[post-quantum-cryptography]
title  = "Post-Quantum Cryptography"
lede   = "Lattice-based cryptography, NIST PQC standards, and the
          harvest-now-decrypt-later threat."
banner = "/images/pqc.webp"
order  = ["quantum-safe-banking-index", "securing-the-ledger"]
```

| Field | Effect |
|---|---|
| `title` | Replaces the term where it is displayed. URLs do not move. |
| `lede` | A paragraph above the page list. |
| `banner` | An image at the top of the page. |
| `order` | Pages that should lead, in this order. Everything else follows, in the order it already had. |

#### What a topic page renders

A page is shown as a **card** when it declares a `description` or a
`banner`, and as a plain link when it declares neither. The two forms mix
on one page, because whether a card is possible belongs to the page, not
the topic. `date` is shown on the card when present.

Topic pages also carry structured data — `CollectionPage` describing the
page, `ItemList` describing its members in order, and `BreadcrumbList`
placing it under the hub. Tag and category pages do not: they are keyword
indexes rather than curated collections.

The hub at `/topics/` shows a card for any topic with a `lede` or
`banner`, and a link with a count for the rest.

All of it comes from the bundled templates, which a theme overrides in
the usual way — `archive.html` for a topic page, `taxonomy_index.html`
for the hub.

The file is looked for beside `content/`, then inside it. It is entirely
optional: without it, topic pages render exactly as they do today.

Curation and content drift apart, so neither kind of drift fails a
build. A slug in `order` that no longer belongs to the topic is skipped.
A `[section]` naming a topic no page carries is reported on stderr and
ignored — you will see it, but your build still finishes.

### Frontmatter Formats

SSG supports three formats, auto-detected by delimiter:

- **YAML** — delimited by `---`
- **TOML** — delimited by `+++`
- **JSON** — delimited by `{` and `}`

SSG also generates `.meta.json` sidecar files during the build for programmatic access to page metadata.

## Content Schemas

Define typed schemas in `content/content.schema.toml` to validate frontmatter at build time. Pages with `schema = "post"` are checked against the `post` schema.

See [Content Schemas](content-schema.md) for the full schema format.

Validate without building:

```sh
ssg --validate -c content
```

## GitHub Flavored Markdown (GFM)

SSG supports GFM extensions via the `MarkdownExtPlugin`:

- **Tables** — pipe-delimited tables with alignment
- **Strikethrough** — `~~deleted text~~`
- **Task lists** — `- [x] done` / `- [ ] todo`

## Shortcodes

Shortcodes are expanded before compilation. Syntax: `{{< name key="value" >}}`.

### Built-in Shortcodes

**YouTube embed:**

```markdown
{{< youtube id="dQw4w9WgXcQ" >}}
```

**GitHub Gist:**

```markdown
{{< gist user="octocat" id="1234567" >}}
```

**Figure with caption:**

```markdown
{{< figure src="/images/photo.jpg" alt="A photo" caption="Figure 1" >}}
```

**Admonition blocks:**

```markdown
{{< warning >}}
This is a warning.
{{< /warning >}}

{{< info >}}...{{< /info >}}
{{< tip >}}...{{< /tip >}}
{{< danger >}}...{{< /danger >}}
```

## Syntax Highlighting

Code blocks with language identifiers receive syntax highlighting via the `HighlightPlugin`:

````markdown
```rust
fn main() {
    println!("Hello, SSG!");
}
```
````

## Directory Structure

Content is organized in directories. Subdirectories create URL path segments:

```text
content/
  index.md          -> /index.html
  about.md          -> /about/index.html
  blog/
    first-post.md   -> /blog/first-post/index.html
    second-post.md  -> /blog/second-post/index.html
```

## Draft Content

Mark pages as drafts in frontmatter:

```yaml
draft: true
```

Drafts are excluded from production builds. Include them with `--drafts`:

```sh
ssg -c content -o public -t templates --drafts
```

## Next Steps

- [Content Schemas](content-schema.md) — typed validation for frontmatter
- [Templates](templates.md) — control how content is rendered
- [SEO](seo.md) — metadata generated from frontmatter
