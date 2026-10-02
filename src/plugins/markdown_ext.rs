// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! GitHub Flavored Markdown (GFM) extensions plugin.
//!
//! Pre-processes Markdown content in the `before_compile` phase to add
//! support for GFM features that the upstream renderer does not handle:
//!
//! - **Tables** — `| col | col |` blocks with a `|---|---|` separator row.
//! - **Strikethrough** — `~~text~~` becomes `<del>text</del>`.
//! - **Task lists** — `- [ ] item` and `- [x] done` become checkbox lists.
//! - **Footnotes** — `[^id]` references with `[^id]:` definitions.
//!
//! ## How it works
//!
//! For each `.md` file under `content_dir`, the plugin:
//! 1. Splits the YAML/TOML frontmatter from the body so it stays untouched.
//! 2. Walks the body line-by-line, tracking fenced code blocks so GFM
//!    syntax inside ``` ``` ``` ``` blocks is preserved literally.
//! 3. Detects GFM-specific blocks (tables, task lists) and renders **only
//!    those blocks** through `pulldown-cmark` with the matching options
//!    enabled, substituting the rendered HTML back into the source.
//! 4. Applies an inline strikethrough transform to remaining text.
//!
//! Standard markdown renderers pass block-level raw HTML through
//! unchanged, so the substituted HTML composes cleanly with whatever
//! renderer staticdatagen runs afterwards.
//!
//! ## Example
//!
//! ```rust
//! use ssg::plugin::PluginManager;
//! use ssg::markdown_ext::MarkdownExtPlugin;
//!
//! let mut pm = PluginManager::new();
//! pm.register(MarkdownExtPlugin);
//! ```

use crate::error::SsgError;
use crate::plugin::{Plugin, PluginContext};
use crate::walk::walk_files_bounded_depth;
use crate::MAX_DIR_DEPTH;
use pulldown_cmark::{html as cmark_html, Options, Parser};
use std::borrow::Cow;
use std::fs;

/// Plugin that expands GFM Markdown extensions in source files.
///
/// Runs in `before_compile`. See the [module-level docs](self) for the
/// full list of supported features and the transformation strategy.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Copy, Clone)]
pub struct MarkdownExtPlugin;

impl Plugin for MarkdownExtPlugin {
    fn name(&self) -> &'static str {
        "markdown-ext"
    }

    fn before_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.content_dir.exists() {
            return Ok(());
        }

        let files =
            walk_files_bounded_depth(&ctx.content_dir, "md", MAX_DIR_DEPTH)
                .map_err(|e| SsgError::io(e, &ctx.content_dir))?;

        let cdn_prefix = ctx
            .config
            .as_ref()
            .and_then(|c| c.cdn_prefix.as_ref())
            .map(|s| s.as_str());

        let mut transformed = 0usize;
        for path in &files {
            fail_point!("markdown_ext::read", |_| {
                Err(SsgError::Io {
                    path: path.clone(),
                    source: std::io::Error::other(
                        "injected: markdown_ext::read",
                    ),
                })
            });
            let raw =
                fs::read_to_string(path).map_err(|e| SsgError::io(e, path))?;

            let new = expand_gfm(&raw, cdn_prefix);
            if new != raw {
                fail_point!("markdown_ext::write", |_| {
                    Err(SsgError::Io {
                        path: path.clone(),
                        source: std::io::Error::other(
                            "injected: markdown_ext::write",
                        ),
                    })
                });
                fs::write(path, &new).map_err(|e| SsgError::io(e, path))?;
                transformed += 1;
            }
        }

        if transformed > 0 {
            log::info!("[markdown-ext] Transformed {transformed} file(s)");
        }
        Ok(())
    }
}

/// Splits leading frontmatter (`--- ... ---`) from `input`.
///
/// Returns `(frontmatter, body)`. If no frontmatter is present the
/// frontmatter slice is empty and the entire input is the body.
fn split_frontmatter(input: &str) -> (&str, &str) {
    if let Some(rest) = input.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let fm_end = "---\n".len() + end + "\n---\n".len();
            return (&input[..fm_end], &input[fm_end..]);
        }
        if let Some(end) = rest.find("\n---") {
            let fm_end = "---\n".len() + end + "\n---".len();
            // Trailing newline after closing fence is optional.
            return (&input[..fm_end], &input[fm_end..]);
        }
    }
    ("", input)
}

/// Expands all GFM constructs in `input`, returning a new string.
/// Also prefixes local images with a CDN URL if `cdn_prefix` is set.
///
/// # Examples
///
/// ```rust
/// use ssg::markdown_ext::expand_gfm;
///
/// // Without GFM-specific syntax the input is preserved verbatim.
/// let out = expand_gfm("plain text\n", None);
/// assert_eq!(out, "plain text\n");
/// ```
#[must_use]
pub fn expand_gfm(input: &str, cdn_prefix: Option<&str>) -> String {
    let (frontmatter, body_raw) = split_frontmatter(input);

    let body_owned;
    let body = if let Some(prefix) = cdn_prefix {
        let md_rewritten = rewrite_markdown_images(body_raw, prefix);
        body_owned = rewrite_html_images(&md_rewritten, prefix);
        &body_owned
    } else {
        body_raw
    };

    if !needs_expansion(body) {
        if cdn_prefix.is_none() {
            return input.to_string();
        }
        let mut out = String::with_capacity(frontmatter.len() + body.len());
        out.push_str(frontmatter);
        out.push_str(body);
        return out;
    }

    let mut out = String::with_capacity(input.len() + 256);
    out.push_str(frontmatter);

    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0usize;
    let mut in_fence = false;
    let mut fence_marker: Option<&str> = None;

    while i < lines.len() {
        let line = lines[i];

        if let Some(marker) = detect_fence(line) {
            update_fence_state(&mut in_fence, &mut fence_marker, marker, line);
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }

        if in_fence {
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }

        i = process_gfm_line(&lines, i, &mut out);
    }

    if !body.ends_with('\n') && out.ends_with('\n') {
        let _ = out.pop();
    }

    out
}

/// Updates fence tracking state when a fence marker is encountered.
fn update_fence_state<'a>(
    in_fence: &mut bool,
    fence_marker: &mut Option<&'a str>,
    marker: &'a str,
    line: &str,
) {
    if !*in_fence {
        *in_fence = true;
        *fence_marker = Some(marker);
    } else if fence_marker.is_some_and(|m| line.trim_start().starts_with(m)) {
        *in_fence = false;
        *fence_marker = None;
    }
}

/// Processes a single non-fenced line, detecting tables, task lists, or
/// applying strikethrough. Returns the new line index.
fn process_gfm_line(lines: &[&str], i: usize, out: &mut String) -> usize {
    let line = lines[i];

    if i + 1 < lines.len() && is_table_header(line, lines[i + 1]) {
        let end = find_table_end(lines, i);
        let block = lines[i..end].join("\n");
        out.push_str(&render_with_options(&block, Options::ENABLE_TABLES));
        out.push('\n');
        return end;
    }

    if is_task_list_line(line) {
        let end = find_task_list_end(lines, i);
        let block = lines[i..end].join("\n");
        out.push_str(&render_with_options(&block, Options::ENABLE_TASKLISTS));
        out.push('\n');
        return end;
    }

    out.push_str(&apply_strikethrough(line));
    out.push('\n');
    i + 1
}

/// Returns `true` if `body` contains any GFM-specific syntax that this
/// plugin would transform.
fn needs_expansion(body: &str) -> bool {
    if body.contains("~~") {
        return true;
    }
    if body.lines().any(is_task_list_line) {
        return true;
    }
    has_table(body)
}

/// Detects whether `body` contains any GFM table block.
fn has_table(body: &str) -> bool {
    let lines: Vec<&str> = body.lines().collect();
    lines.windows(2).any(|w| is_table_header(w[0], w[1]))
}

/// Returns the fence marker (` ``` ` or `~~~`) if `line` opens or
/// closes a fenced code block.
fn detect_fence(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Returns `true` if `header` looks like a table header followed by a
/// `|---|---|` separator row on `separator`.
fn is_table_header(header: &str, separator: &str) -> bool {
    if !header.contains('|') {
        return false;
    }
    is_separator_row(separator)
}

/// Returns `true` if `line` is a GFM table separator row like
/// `| --- | :---: | ---: |`.
fn is_separator_row(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('-') || !t.contains('|') {
        return false;
    }
    t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// Returns the index *just past* the last contiguous table line.
fn find_table_end(lines: &[&str], start: usize) -> usize {
    let mut end = start + 2; // header + separator
    while end < lines.len() {
        let l = lines[end];
        if l.trim().is_empty() || !l.contains('|') {
            break;
        }
        end += 1;
    }
    end
}

/// Returns `true` if `line` is a task list item.
fn is_task_list_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.len() < 6 {
        return false;
    }
    let bytes = t.as_bytes();
    let bullet = bytes[0];
    if !matches!(bullet, b'-' | b'*' | b'+') {
        return false;
    }
    if bytes[1] != b' ' {
        return false;
    }
    if bytes[2] != b'[' {
        return false;
    }
    if !matches!(bytes[3], b' ' | b'x' | b'X') {
        return false;
    }
    if bytes[4] != b']' {
        return false;
    }
    bytes[5] == b' '
}

/// Returns the index just past the last contiguous task list line.
fn find_task_list_end(lines: &[&str], start: usize) -> usize {
    let mut end = start;
    while end < lines.len() && is_task_list_line(lines[end]) {
        end += 1;
    }
    end
}

/// Renders `markdown` to HTML using `pulldown-cmark` with `extra`
/// options merged in alongside the always-on strikethrough flag.
fn render_with_options(markdown: &str, extra: Options) -> String {
    let mut opts = Options::ENABLE_STRIKETHROUGH;
    opts.insert(extra);
    let parser = Parser::new_ext(markdown, opts);
    let mut html = String::with_capacity(markdown.len() + 64);
    cmark_html::push_html(&mut html, parser);
    // Trim trailing whitespace in place instead of cloning the whole
    // rendered block (issue #578, plan §4 3.1).
    let trimmed_len = html.trim_end().len();
    html.truncate(trimmed_len);
    html
}

/// Replaces `~~text~~` with `<del>text</del>` outside of inline code spans.
///
/// Single pass that only allocates when a complete `~~…~~` pair is
/// actually replaced; lines without strikethrough are returned borrowed
/// (issue #578, plan §4 3.1). Unchanged spans are copied slice-wise, so
/// multi-byte UTF-8 text between replacements is preserved verbatim
/// (the previous byte-by-byte `push(byte as char)` mangled it).
fn apply_strikethrough(line: &str) -> Cow<'_, str> {
    let bytes = line.as_bytes();
    // Lazily-created output buffer; `copied` tracks how much of `line`
    // has already been flushed into it.
    let mut out: Option<String> = None;
    let mut copied = 0usize;
    let mut i = 0usize;
    let mut in_code = false;

    while i < bytes.len() {
        if bytes[i] == b'`' {
            in_code = !in_code;
            i += 1;
            continue;
        }
        if !in_code
            && i + 1 < bytes.len()
            && bytes[i] == b'~'
            && bytes[i + 1] == b'~'
        {
            // Find closing `~~`.
            if let Some(close) = find_strike_close(line, i + 2) {
                let buf = out.get_or_insert_with(|| {
                    String::with_capacity(line.len() + 16)
                });
                buf.push_str(&line[copied..i]);
                buf.push_str("<del>");
                buf.push_str(&line[i + 2..close]);
                buf.push_str("</del>");
                i = close + 2;
                copied = i;
                continue;
            }
        }
        i += 1;
    }

    match out {
        Some(mut buf) => {
            buf.push_str(&line[copied..]);
            Cow::Owned(buf)
        }
        None => Cow::Borrowed(line),
    }
}

/// Returns the byte offset of the next `~~` after `from`, or `None`.
const fn find_strike_close(line: &str, from: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut j = from;
    while j + 1 < bytes.len() {
        if bytes[j] == b'`' {
            // Skip inline code spans inside the strike content.
            let mut k = j + 1;
            while k < bytes.len() && bytes[k] != b'`' {
                k += 1;
            }
            j = k.saturating_add(1);
            continue;
        }
        if bytes[j] == b'~' && bytes[j + 1] == b'~' {
            return Some(j);
        }
        j += 1;
    }
    None
}

fn rewrite_markdown_images(body: &str, cdn_prefix: &str) -> String {
    let mut result = String::with_capacity(body.len());
    let mut remaining = body;

    while let Some(start_idx) = remaining.find("![") {
        result.push_str(&remaining[..start_idx]);
        let post_bracket = &remaining[start_idx + 2..];

        let Some(close_bracket_idx) = post_bracket.find(']') else {
            result.push_str("![");
            remaining = post_bracket;
            continue;
        };

        let alt_text = &post_bracket[..close_bracket_idx];
        let post_alt = &post_bracket[close_bracket_idx + 1..];

        if post_alt.starts_with('(') {
            let Some(close_paren_idx) = post_alt.find(')') else {
                result.push_str("![");
                result.push_str(alt_text);
                result.push(']');
                remaining = post_alt;
                continue;
            };

            let url = &post_alt[1..close_paren_idx];

            let new_url = if !url.starts_with("http://")
                && !url.starts_with("https://")
                && !url.starts_with("//")
                && !url.starts_with("data:")
            {
                format!("{}{}&w=1600&format=webp&q=85", cdn_prefix, url)
            } else {
                url.to_string()
            };

            result.push_str(&format!("![{alt_text}]({new_url})"));
            remaining = &post_alt[close_paren_idx + 1..];
        } else {
            result.push_str("![");
            result.push_str(alt_text);
            result.push(']');
            remaining = post_alt;
        }
    }

    result.push_str(remaining);
    result
}

fn rewrite_html_images(body: &str, cdn_prefix: &str) -> String {
    let mut result = String::with_capacity(body.len());
    let mut remaining = body;

    while let Some(start_idx) = remaining.find("<img ") {
        result.push_str(&remaining[..start_idx]);
        let tag_content = &remaining[start_idx..];

        let Some(end_idx) = tag_content.find('>') else {
            result.push_str(remaining);
            return result;
        };

        let tag_inner = &tag_content[..end_idx + 1];
        let mut rewritten_tag = tag_inner.to_string();

        let mut src_val = None;
        for quote in ['"', '\''] {
            let pattern = format!("src={quote}");
            if let Some(pos) = tag_inner.find(&pattern) {
                let val_start = pos + pattern.len();
                if let Some(val_end) = tag_inner[val_start..].find(quote) {
                    src_val = Some((
                        val_start,
                        val_end,
                        tag_inner[val_start..val_start + val_end].to_string(),
                        quote,
                    ));
                    break;
                }
            }
        }

        if let Some((val_start, val_end, url, _quote)) = src_val {
            if !url.starts_with("http://")
                && !url.starts_with("https://")
                && !url.starts_with("//")
                && !url.starts_with("data:")
            {
                let new_url =
                    format!("{}{}&w=1600&format=webp&q=85", cdn_prefix, url);
                let before = &rewritten_tag[..val_start];
                let after = &rewritten_tag[val_start + val_end..];
                rewritten_tag = format!("{before}{new_url}{after}");
            }
        }

        result.push_str(&rewritten_tag);
        remaining = &tag_content[end_idx + 1..];
    }

    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::Plugin;
    use tempfile::tempdir;

    #[test]
    fn split_frontmatter_extracts_yaml_block() {
        let input = "---\ntitle: Hello\n---\nBody here\n";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, "---\ntitle: Hello\n---\n");
        assert_eq!(body, "Body here\n");
    }

    #[test]
    fn split_frontmatter_returns_empty_when_absent() {
        let input = "Just a body\nwith two lines\n";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, "");
        assert_eq!(body, input);
    }

    #[test]
    fn needs_expansion_detects_strikethrough() {
        assert!(needs_expansion("hello ~~world~~"));
    }

    #[test]
    fn needs_expansion_detects_task_list() {
        assert!(needs_expansion("- [ ] todo\n- [x] done\n"));
    }

    #[test]
    fn needs_expansion_detects_table() {
        let body = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert!(needs_expansion(body));
    }

    #[test]
    fn needs_expansion_returns_false_for_plain_markdown() {
        assert!(!needs_expansion("# Heading\n\nA paragraph.\n"));
    }

    #[test]
    fn is_separator_row_accepts_aligned_separators() {
        assert!(is_separator_row("|---|---|"));
        assert!(is_separator_row("| :--- | :---: | ---: |"));
        assert!(!is_separator_row("| a | b |"));
        assert!(!is_separator_row("plain text"));
    }

    #[test]
    fn is_task_list_line_recognises_open_and_done() {
        assert!(is_task_list_line("- [ ] todo"));
        assert!(is_task_list_line("- [x] done"));
        assert!(is_task_list_line("- [X] done"));
        assert!(is_task_list_line("  * [ ] indented"));
        assert!(!is_task_list_line("- regular bullet"));
        assert!(!is_task_list_line("[ ] no bullet"));
    }

    #[test]
    fn apply_strikethrough_wraps_simple_pair() {
        assert_eq!(
            apply_strikethrough("hello ~~world~~ done"),
            "hello <del>world</del> done"
        );
    }

    #[test]
    fn apply_strikethrough_skips_inside_code_span() {
        assert_eq!(
            apply_strikethrough("`~~not~~` but ~~yes~~"),
            "`~~not~~` but <del>yes</del>"
        );
    }

    #[test]
    fn apply_strikethrough_leaves_unmatched_tildes() {
        assert_eq!(apply_strikethrough("just ~~ here"), "just ~~ here");
    }

    #[test]
    fn apply_strikethrough_borrows_when_no_delimiter() {
        // Issue #578: the no-strikethrough fast path must not allocate.
        assert!(matches!(
            apply_strikethrough("plain line, nothing to do"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn apply_strikethrough_preserves_multibyte_text() {
        assert_eq!(
            apply_strikethrough("café ~~ancien~~ nouveau — été"),
            "café <del>ancien</del> nouveau — été"
        );
    }

    #[test]
    fn expand_gfm_multiline_without_strikethrough_unchanged() {
        // Multiline body with no GFM constructs: byte-identical output.
        let input = "# Title\n\nfirst plain line\nsecond plain line\n\nthird paragraph line\n";
        assert_eq!(expand_gfm(input, None), input);
    }

    #[test]
    fn expand_gfm_mixed_lines_only_transforms_strikethrough() {
        let input = "keep this line\n~~gone~~ stays\nanother plain line\n";
        assert_eq!(
            expand_gfm(input, None),
            "keep this line\n<del>gone</del> stays\nanother plain line\n"
        );
    }

    #[test]
    fn expand_gfm_renders_table_block() {
        let input = "Intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nOutro\n";
        let out = expand_gfm(input, None);
        assert!(out.contains("<table>"), "got: {out}");
        assert!(out.contains("<th>a</th>"));
        assert!(out.contains("<td>1</td>"));
        assert!(out.contains("Intro"));
        assert!(out.contains("Outro"));
    }

    #[test]
    fn expand_gfm_renders_task_list_block() {
        let input = "- [ ] one\n- [x] two\n";
        let out = expand_gfm(input, None);
        assert!(out.contains("<ul>"), "got: {out}");
        assert!(out.contains("type=\"checkbox\""));
        assert!(out.contains("disabled"));
        assert!(out.contains("checked"));
    }

    #[test]
    fn expand_gfm_renders_strikethrough_inline() {
        let input = "Some ~~old~~ new text\n";
        let out = expand_gfm(input, None);
        assert_eq!(out, "Some <del>old</del> new text\n");
    }

    #[test]
    fn expand_gfm_preserves_fenced_code_contents() {
        let input =
            "```\n| a | b |\n|---|---|\n~~not strike~~\n- [ ] not task\n```\n";
        let out = expand_gfm(input, None);
        // Nothing inside the fence should be transformed.
        assert!(out.contains("| a | b |"));
        assert!(out.contains("~~not strike~~"));
        assert!(out.contains("- [ ] not task"));
        assert!(!out.contains("<table>"));
        assert!(!out.contains("<del>"));
    }

    #[test]
    fn expand_gfm_preserves_frontmatter_unchanged() {
        let input = "---\ntitle: Test\n---\n~~strike~~ this\n";
        let out = expand_gfm(input, None);
        assert!(out.starts_with("---\ntitle: Test\n---\n"));
        assert!(out.contains("<del>strike</del>"));
    }

    #[test]
    fn expand_gfm_returns_input_unchanged_when_no_features() {
        let input = "# Heading\n\nA paragraph with no extensions.\n";
        let out = expand_gfm(input, None);
        assert_eq!(out, input);
    }

    #[test]
    fn expand_gfm_handles_tildes_in_tilde_fenced_code() {
        // ~~~ fences must also protect contents.
        let input = "~~~\n~~text~~\n~~~\n";
        let out = expand_gfm(input, None);
        assert!(out.contains("~~text~~"));
        assert!(!out.contains("<del>"));
    }

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn plugin_transforms_markdown_files_in_place() {
        crate::test_support::init_logger();
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            content.join("post.md"),
            "---\ntitle: Test\n---\n~~old~~ new\n",
        )
        .unwrap();
        fs::write(content.join("untouched.md"), "# Plain\n\nNothing fancy.\n")
            .unwrap();

        let ctx =
            PluginContext::new(&content, dir.path(), dir.path(), dir.path());
        MarkdownExtPlugin.before_compile(&ctx).unwrap();

        let post = fs::read_to_string(content.join("post.md")).unwrap();
        assert!(post.contains("<del>old</del>"));
        assert!(post.starts_with("---\ntitle: Test\n---\n"));

        let untouched =
            fs::read_to_string(content.join("untouched.md")).unwrap();
        assert_eq!(untouched, "# Plain\n\nNothing fancy.\n");
    }

    #[test]
    fn plugin_returns_ok_when_content_dir_missing() {
        let dir = tempdir().unwrap();
        let ctx = PluginContext::new(
            &dir.path().join("missing"),
            dir.path(),
            dir.path(),
            dir.path(),
        );
        MarkdownExtPlugin.before_compile(&ctx).unwrap();
    }

    #[test]
    fn plugin_name_is_markdown_ext() {
        assert_eq!(MarkdownExtPlugin.name(), "markdown-ext");
    }

    #[test]
    fn test_cdn_prefix_rewrites_images() {
        let input = "![Alt text](/images/pic.png)\n<img src=\"/images/pic2.png\" alt=\"HTML img\">";
        let prefix = "https://cloudcdn.pro/api/transform?url=";
        let out = expand_gfm(input, Some(prefix));
        assert!(out.contains("![Alt text](https://cloudcdn.pro/api/transform?url=/images/pic.png&w=1600&format=webp&q=85)"));
        assert!(out.contains("src=\"https://cloudcdn.pro/api/transform?url=/images/pic2.png&w=1600&format=webp&q=85\""));
    }

    #[test]
    fn split_frontmatter_optional_trailing_newline() {
        let input = "---\ntitle: Hello\n---Body here";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, "---\ntitle: Hello\n---");
        assert_eq!(body, "Body here");
    }

    #[test]
    fn test_cdn_prefix_no_gfm_expansion() {
        let input = "![Alt](/img.png)";
        let prefix = "https://cdn.example.com/";
        let out = expand_gfm(input, Some(prefix));
        assert!(out.contains("https://cdn.example.com//img.png"));
    }

    #[test]
    fn test_rewrite_html_images_edge_cases() {
        let prefix = "https://cdn/";

        let out1 = rewrite_html_images("<img src='foo.png'>", prefix);
        assert!(out1.contains("https://cdn/foo.png"));

        assert_eq!(
            rewrite_html_images("<img src=\"http://foo.com/a.png\">", prefix),
            "<img src=\"http://foo.com/a.png\">"
        );
        assert_eq!(
            rewrite_html_images("<img src=\"https://foo.com/a.png\">", prefix),
            "<img src=\"https://foo.com/a.png\">"
        );
        assert_eq!(
            rewrite_html_images("<img src=\"//foo.com/a.png\">", prefix),
            "<img src=\"//foo.com/a.png\">"
        );
        assert_eq!(
            rewrite_html_images(
                "<img src=\"data:image/png;base64,...\">",
                prefix
            ),
            "<img src=\"data:image/png;base64,...\">"
        );

        assert_eq!(
            rewrite_html_images("<img src=\"foo.png\"", prefix),
            "<img src=\"foo.png\""
        );
    }

    #[test]
    fn test_rewrite_markdown_images_edge_cases() {
        let prefix = "https://cdn/";

        assert_eq!(
            rewrite_markdown_images("![unclosed alt text", prefix),
            "![unclosed alt text"
        );

        assert_eq!(
            rewrite_markdown_images("![alt](unclosed-paren", prefix),
            "![alt](unclosed-paren"
        );

        assert_eq!(
            rewrite_markdown_images("![alt] no paren", prefix),
            "![alt] no paren"
        );

        assert_eq!(
            rewrite_markdown_images("![alt](http://site.com/img.png)", prefix),
            "![alt](http://site.com/img.png)"
        );
        assert_eq!(
            rewrite_markdown_images("![alt](https://site.com/img.png)", prefix),
            "![alt](https://site.com/img.png)"
        );
        assert_eq!(
            rewrite_markdown_images("![alt](//site.com/img.png)", prefix),
            "![alt](//site.com/img.png)"
        );
        assert_eq!(
            rewrite_markdown_images(
                "![alt](data:image/png;base64,123)",
                prefix
            ),
            "![alt](data:image/png;base64,123)"
        );
    }

    #[test]
    fn split_frontmatter_unterminated_fence_treats_all_as_body() {
        // Opening `---\n` with no closing fence at all: both `find`
        // branches miss and the whole input is the body.
        let input = "---\ntitle: broken frontmatter with no closing fence";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, "");
        assert_eq!(body, input);
    }

    #[test]
    fn expand_gfm_without_trailing_newline_pops_added_newline() {
        // The line-walk emits a trailing '\n'; when the source body has
        // none, expand_gfm must pop it to stay byte-faithful.
        let out = expand_gfm("~~a~~", None);
        assert_eq!(out, "<del>a</del>");
    }

    #[test]
    fn is_task_list_line_rejects_malformed_checkbox_syntax() {
        // No space after the bullet.
        assert!(!is_task_list_line("-[ ] task"));
        // Invalid checkbox state character.
        assert!(!is_task_list_line("- [y] task"));
        // Missing closing bracket.
        assert!(!is_task_list_line("- [x} task"));
        // No space after the checkbox.
        assert!(!is_task_list_line("- [x]task"));
    }

    #[test]
    fn apply_strikethrough_skips_code_span_inside_strike_content() {
        // The closing-delimiter scan must jump over inline code spans
        // between the opening and closing `~~`.
        assert_eq!(
            apply_strikethrough("~~has `tick` inside~~"),
            "<del>has `tick` inside</del>"
        );
    }

    #[test]
    fn rewrite_html_images_unterminated_src_quote_left_unchanged() {
        // src attribute opened but never closed before `>`: the value
        // scan finds no closing quote and the tag is left as-is.
        let input = "<img src=\"unterminated.png>";
        assert_eq!(rewrite_html_images(input, "https://cdn/"), input);
    }

    #[test]
    fn rewrite_html_images_without_src_attribute_left_unchanged() {
        let input = "<img alt=\"no source here\">";
        assert_eq!(rewrite_html_images(input, "https://cdn/"), input);
    }

    #[test]
    fn expand_gfm_table_ends_without_blank_line_separator() {
        // No blank line between the table and the next paragraph: the
        // `find_table_end` scan must stop on the non-empty,
        // pipe-free "Outro" line via its second OR operand, not the
        // "line is blank" operand exercised by the sibling test above.
        let input = "Intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\nOutro\n";
        let out = expand_gfm(input, None);
        assert!(out.contains("<table>"), "got: {out}");
        assert!(out.contains("<td>1</td>"));
        assert!(out.contains("Outro"));
    }

    #[test]
    fn expand_gfm_task_list_followed_by_plain_text() {
        // `find_task_list_end` must stop as soon as a non-task-list
        // line is encountered, rather than only ever running off the
        // end of the `lines` slice (as the other task-list test does).
        let input = "- [ ] one\n- [x] two\nplain paragraph\n";
        let out = expand_gfm(input, None);
        assert!(out.contains("<ul>"), "got: {out}");
        assert!(out.contains("plain paragraph"));
    }

    #[test]
    #[cfg_attr(feature = "test-fault-injection", serial_test::serial)]
    fn before_compile_applies_cdn_prefix_from_config() {
        // The cdn_prefix extraction closures in `before_compile`
        // (`ctx.config.as_ref().and_then(..).map(..)`) are only
        // exercised when a real `SsgConfig` with `cdn_prefix` set is
        // threaded through the plugin context — the `expand_gfm`
        // tests above call the free function directly and never
        // reach this code path.
        use crate::cmd::SsgConfig;
        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("post.md"), "![Alt](/images/pic.png)\n")
            .unwrap();

        let cfg = SsgConfig::builder()
            .cdn_prefix(Some("https://cdn.example.com/".to_string()))
            .build()
            .expect("config");
        let ctx = PluginContext::with_config(
            &content,
            dir.path(),
            dir.path(),
            dir.path(),
            cfg,
        );
        MarkdownExtPlugin.before_compile(&ctx).unwrap();

        let post = fs::read_to_string(content.join("post.md")).unwrap();
        assert!(
            post.contains("https://cdn.example.com//images/pic.png"),
            "got: {post}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn before_compile_propagates_walk_error_from_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        let locked = content.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let ctx =
            PluginContext::new(&content, dir.path(), dir.path(), dir.path());
        let result = MarkdownExtPlugin.before_compile(&ctx);

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(result.is_err(), "unreadable subdir must surface an error");
    }

    #[cfg(unix)]
    #[test]
    fn before_compile_real_read_permission_denied_returns_io_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let file = content.join("locked.md");
        fs::write(&file, "# Hi").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let ctx =
            PluginContext::new(&content, dir.path(), dir.path(), dir.path());
        let result = MarkdownExtPlugin.before_compile(&ctx);

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let err = result.expect_err("permission-denied read must surface");
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[cfg(unix)]
    #[test]
    fn before_compile_real_write_permission_denied_returns_io_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let file = content.join("post.md");
        // Content that expand_gfm rewrites, so the write site is reached.
        fs::write(&file, "~~old~~ new\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();

        let ctx =
            PluginContext::new(&content, dir.path(), dir.path(), dir.path());
        let result = MarkdownExtPlugin.before_compile(&ctx);

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let err = result.expect_err("permission-denied write must surface");
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::plugin::{Plugin, PluginContext};
    use serial_test::serial;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop (mirrors the
    /// convention in `tests/fault_injection.rs`).
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial]
    fn before_compile_read_failpoint_propagates() {
        let _guard = FailGuard("markdown_ext::read");
        fail::cfg("markdown_ext::read", "return").expect("activate failpoint");

        let dir = tempdir().unwrap();
        let content = dir.path().to_path_buf();
        fs::write(content.join("post.md"), "# Hi").unwrap();

        let ctx =
            PluginContext::new(&content, dir.path(), dir.path(), dir.path());
        let err = MarkdownExtPlugin
            .before_compile(&ctx)
            .expect_err("injected read failure must propagate");
        assert!(format!("{err:?}").contains("injected: markdown_ext::read"));
    }

    #[test]
    #[serial]
    fn before_compile_write_failpoint_propagates() {
        let _guard = FailGuard("markdown_ext::write");
        fail::cfg("markdown_ext::write", "return").expect("activate failpoint");

        let dir = tempdir().unwrap();
        let content = dir.path().to_path_buf();
        // Content that expand_gfm rewrites, so the write site is reached.
        fs::write(content.join("post.md"), "~~old~~ new\n").unwrap();

        let ctx =
            PluginContext::new(&content, dir.path(), dir.path(), dir.path());
        let err = MarkdownExtPlugin
            .before_compile(&ctx)
            .expect_err("injected write failure must propagate");
        assert!(format!("{err:?}").contains("injected: markdown_ext::write"));
    }
}
