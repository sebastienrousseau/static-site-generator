<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Templates

SSG uses the [MiniJinja](https://docs.rs/minijinja) template engine for rendering HTML from content.

## Template Directory

Templates live in the directory specified by `-t` / `template_dir`. The
compile stage renders each content file through a StaticWeaver layout
at the root of that directory, then the template plugin wraps the
rendered content in the MiniJinja layouts under `tera/`:

```text
templates/
  template.html   # StaticWeaver layouts read by the compile stage
  index.html
  page.html
  post.html
  tera/
    base.html     # MiniJinja base layout with shared structure
    page.html     # Default page template
    post.html     # Blog post template
    index.html    # Home / listing page
```

`ssg --new <name>` scaffolds both sets. A MiniJinja layout receives the
page's rendered content as `page.content`, not the compiled document, so
`base.html` owns the `<html>`, `<head>` and `<main>` of every page.

## MiniJinja Basics

MiniJinja uses `{{ }}` for expressions and `{% %}` for logic:

```html
<h1>{{ page.title }}</h1>
<p>Published on {{ page.date }}</p>
{{ page.content | safe }}
```

## Template Inheritance

Define a base layout with blocks that child templates override:

### base.html

```html
<!DOCTYPE html>
<html lang="{{ site.language }}">
<head>
  <meta charset="utf-8">
  <title>{% block title %}{{ site.title }}{% endblock %}</title>
  {% block head %}{% endblock %}
</head>
<body>
  <header>{% block header %}{% endblock %}</header>
  <main>{% block content %}{% endblock %}</main>
  <footer>{% block footer %}&copy; {{ site.title }}{% endblock %}</footer>
</body>
</html>
```

### page.html

```html
{% extends "base.html" %}

{% block title %}{{ page.title }} | {{ site.title }}{% endblock %}

{% block content %}
  <article>
    <h1>{{ page.title }}</h1>
    {{ page.content | safe }}
  </article>
{% endblock %}
```

### post.html

```html
{% extends "base.html" %}

{% block title %}{{ page.title }} | {{ site.title }}{% endblock %}

{% block content %}
  <article>
    <h1>{{ page.title }}</h1>
    <time datetime="{{ page.date }}">{{ page.date }}</time>
    {{ page.content | safe }}
  </article>
{% endblock %}
```

## Available Variables

### `site` Object

| Variable | Description |
| :--- | :--- |
| `site.title` | Site title from config |
| `site.description` | Site description |
| `site.base_url` | Base URL |
| `site.language` | Language code |

### `page` Object

| Variable | Description |
| :--- | :--- |
| `page.title` | Page title from frontmatter |
| `page.date` | Publication date |
| `page.description` | Meta description |
| `page.content` | Rendered HTML content (use `\| safe`) |
| `page.url` | Page URL path |
| `page.tags` | List of tags |
| `page.categories` | List of categories |
| `page.draft` | Whether the page is a draft |

## Control Flow

**Conditionals:**

```html
{% if page.draft %}
  <span class="badge">Draft</span>
{% endif %}
```

**Loops:**

```html
{% for tag in page.tags %}
  <a href="/tags/{{ tag }}">{{ tag }}</a>
{% endfor %}
```

## Filters

MiniJinja has built-in filters. Common ones:

```html
{{ page.title | upper }}
{{ page.title | truncate(length=50) }}
{{ page.date | date(format="%B %d, %Y") }}
{{ page.content | safe }}
{{ page.content | striptags | truncate(length=160) }}
```

## Per-Page Template Override

Set `template` in frontmatter to use a specific template:

```yaml
---
title: "Custom Layout"
template: "custom.html"
---
```

## Bundled Templates and Themes

SSG includes 7 bundled templates and 3 themes:

- **minimal** — clean, lightweight design
- **docs** — documentation-focused layout
- **full** — feature-rich with navigation and sidebar

## Feature Flag

MiniJinja templating requires the `templates` feature (enabled by default). The `TemplatePlugin` handles rendering during the `after_compile` phase.

## Next Steps

- [Content](content.md) — frontmatter fields available in templates
- [Plugins](plugins.md) — how plugins interact with template rendering
- [SEO](seo.md) — meta tags injected into templates
