<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Topics

A topic gathers related pages under one heading and gives them a page of
their own at `/topics/{slug}/`.

SSG builds that set from front matter. What it cannot work out is
editorial judgement: which topic deserves a real title, what the topic
is actually about, and which page should lead. A data file supplies
that.

## Assigning pages to a topic

Add `topic_clusters` to a page's front matter:

```yaml
---
title: "Quantum-safe banking index"
topic_clusters: ["post-quantum-cryptography"]
---
```

A page can belong to several topics. With no data file, SSG still builds
every topic page from these assignments alone.

## Curating a topic

Create `_data/topics.toml`. Each section is a topic slug:

```toml
[post-quantum-cryptography]
title  = "Post-Quantum Cryptography"
lede   = "Lattice cryptography and the harvest-now-decrypt-later threat."
banner = "/images/pqc.webp"
order  = ["quantum-safe-banking-index", "securing-the-ledger"]
```

| Field | Effect |
| :--- | :--- |
| `title` | Replaces the slug wherever the topic is shown. URLs do not move. |
| `lede` | One paragraph above the page list. |
| `banner` | Image above the page list. |
| `order` | Pages that should lead, in this order. |

Every field is optional, and so is the file. Without it, topic pages
render exactly as they did before.

`order` names the pages that come first. Everything else follows in the
order it already had, so you can promote two pages without listing the
other forty.

## Drift does not stop a build

Content moves. Curation lags behind it. Neither kind of drift fails a
build:

- A slug in `order` that has left the topic is skipped.
- A section naming a topic no page carries is reported on stderr and
  ignored.

A stale line in a data file is not a reason to stop shipping.

## What a topic page contains

Each page renders through your theme's template, so you keep control of
the markup. Pages appear as cards: title, description, date and banner
all come from the page's own front matter.

Topic pages also carry structured data — `CollectionPage`, `ItemList`
and `BreadcrumbList` — so a search engine can read the topic as a
collection rather than a list of links.

The hub at `/topics/` lists every topic, using curated titles and ledes
where you have written them.

## See also

- [Listings](listings.md) — filtered, paginated archives
- [SEO](seo.md) — the structured data these pages emit
