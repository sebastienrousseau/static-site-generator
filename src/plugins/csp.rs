// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Content Security Policy hardening plugin.
//!
//! Extracts inline `<style>` and `<script>` blocks into external files
//! with Subresource Integrity (SRI) hashes, eliminating the need for
//! `'unsafe-inline'` in the Content-Security-Policy header.

use crate::cmd::SriAlgorithm;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use anyhow::Result;
use std::{fs, path::Path};

/// Canonical Content-Security-Policy string emitted by the CSP plugin.
///
/// Returned by [`computed_policy`] and consumed by downstream emitters
/// (e.g. the `edge_headers` postprocess plugin) that need to forward
/// the same policy as an HTTP header instead of a `<meta>` tag.
///
/// The string is intentionally `'unsafe-inline'`-free; this matches the
/// post-extraction posture enforced by [`CspPlugin::transform_html`]
/// and [`inject_csp_meta`] (which strip `'unsafe-inline'` from any
/// preexisting `<meta>` policy on the way through).
pub const DEFAULT_CSP_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' https: data:; font-src 'self' https:; connect-src 'self'; frame-ancestors 'none'";

/// Content-Security-Policy template with `{script_hashes}` and
/// `{style_hashes}` slots (spec B4, v0.0.47 plan §3 item 2.4).
///
/// [`render_policy_template`] expands each slot into zero or more
/// space-prefixed `'sha256-…'` source expressions. With both slots
/// empty the rendered string is byte-identical to
/// [`DEFAULT_CSP_POLICY`], so pages without inline blocks fall back
/// to exactly the global policy — the invariant is pinned by a unit
/// test in this module.
///
/// This constant is the single template notion for the CSP plugin; a
/// future `[security.csp] template` knob in `ssg.toml` overrides it by
/// passing the configured string to [`render_policy_template`] — the
/// rendering path already accepts an arbitrary template.
///
/// # Examples
///
/// ```rust
/// use ssg::csp::{
///     render_policy_template, DEFAULT_CSP_POLICY, DEFAULT_CSP_POLICY_TEMPLATE,
/// };
///
/// assert!(DEFAULT_CSP_POLICY_TEMPLATE.contains("{script_hashes}"));
/// assert!(DEFAULT_CSP_POLICY_TEMPLATE.contains("{style_hashes}"));
///
/// // Both slots empty ⇒ byte-identical to the global policy.
/// let rendered =
///     render_policy_template(DEFAULT_CSP_POLICY_TEMPLATE, &[], &[]);
/// assert_eq!(rendered, DEFAULT_CSP_POLICY);
/// ```
pub const DEFAULT_CSP_POLICY_TEMPLATE: &str = "default-src 'self'; script-src 'self'{script_hashes}; style-src 'self'{style_hashes}; img-src 'self' https: data:; font-src 'self' https:; connect-src 'self'; frame-ancestors 'none'";

/// Returns the canonical Content-Security-Policy string that the CSP
/// plugin's inline-extraction posture is designed to enforce.
///
/// This is the single source of truth for the CSP header value: every
/// other emitter (deploy adapters, edge-headers postprocess plugin)
/// reads from this function rather than recomputing or hardcoding the
/// string, ensuring that platform-level HTTP headers and the
/// `<meta http-equiv>` injection stay in lock-step.
///
/// Returns a borrowed reference to [`DEFAULT_CSP_POLICY`] today; the
/// signature is intentionally a borrowed `&'static str` so callers can
/// pass it directly into `format!` / `write!` without an extra
/// allocation.
///
/// # Examples
///
/// ```rust
/// use ssg::csp::computed_policy;
///
/// let policy = computed_policy();
/// assert!(policy.contains("default-src"));
/// ```
#[must_use]
pub const fn computed_policy() -> &'static str {
    DEFAULT_CSP_POLICY
}

/// CSP source hashes for the inline blocks remaining on a single page
/// (spec B4, plan §3 item 2.4).
///
/// Each entry is a bare `sha256-<base64>` token — the caller wraps it
/// in single quotes when splicing it into a CSP directive. Hashes are
/// listed in document order with duplicates removed, so the same
/// input HTML always produces the same vector (determinism gate).
///
/// # Examples
///
/// ```rust
/// use ssg::csp::page_inline_hashes;
///
/// let html = "<style>body{margin:0}</style><script>init()</script>";
/// let hashes = page_inline_hashes(html);
/// assert_eq!(hashes.scripts.len(), 1);
/// assert_eq!(hashes.styles.len(), 1);
/// assert!(!hashes.is_empty());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageCspHashes {
    /// Hashes of inline `<script>` bodies (no `src=` attribute),
    /// including non-executable blocks such as JSON-LD structured
    /// data — hash-listing them keeps the policy valid for UAs that
    /// apply `script-src` to data blocks.
    pub scripts: Vec<String>,
    /// Hashes of inline `<style>` bodies.
    pub styles: Vec<String>,
}

impl PageCspHashes {
    /// Returns `true` when the page has no inline blocks at all.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::csp::PageCspHashes;
    ///
    /// assert!(PageCspHashes::default().is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.scripts.is_empty() && self.styles.is_empty()
    }
}

/// Computes the CSP source hashes for every inline block on a page.
///
/// Unlike the extraction pass ([`CspPlugin::transform_html`]), this
/// scan does **not** skip JSON-LD or livereload-marked scripts: it
/// hashes whatever is still inline in the HTML it is given, because
/// its consumers (the `edge_headers` postprocess plugin) run on the
/// final page bytes and need the policy to match what actually ships.
///
/// CSP directive source hashes are always SHA-256 for the broadest UA
/// compatibility; the `[security] sri_algorithm` knob governs only
/// SRI `integrity=` attributes (see the `compute_sri` doc comment).
///
/// # Examples
///
/// ```rust
/// use ssg::csp::page_inline_hashes;
///
/// let html = r#"<script type="application/ld+json">{"@type":"Thing"}</script>"#;
/// let hashes = page_inline_hashes(html);
/// assert_eq!(hashes.scripts.len(), 1);
/// assert!(hashes.scripts[0].starts_with("sha256-"));
/// assert!(hashes.styles.is_empty());
/// ```
#[must_use]
pub fn page_inline_hashes(html: &str) -> PageCspHashes {
    let hash =
        |content: &str| SriAlgorithm::Sha256.integrity(content.as_bytes());

    // One parse for both tags: see `collect_inline_script_and_style`.
    let (raw_scripts, raw_styles) = collect_inline_script_and_style(html);

    let dedup = |raw: Vec<String>| {
        let mut out: Vec<String> = Vec::with_capacity(raw.len());
        for content in raw {
            let h = hash(&content);
            if !out.contains(&h) {
                out.push(h);
            }
        }
        out
    };

    let scripts = dedup(raw_scripts);
    let styles = dedup(raw_styles);

    PageCspHashes { scripts, styles }
}

/// Renders a CSP policy template, expanding the `{script_hashes}` and
/// `{style_hashes}` slots into space-prefixed `'sha256-…'` sources.
///
/// Empty slices render to an empty string, so a template rendered
/// with no hashes reduces to its hash-free form (for
/// [`DEFAULT_CSP_POLICY_TEMPLATE`] that is exactly
/// [`DEFAULT_CSP_POLICY`]). The output never contains
/// `'unsafe-inline'` unless the template itself does.
///
/// # Examples
///
/// ```rust
/// use ssg::csp::{render_policy_template, DEFAULT_CSP_POLICY, DEFAULT_CSP_POLICY_TEMPLATE};
///
/// let empty = render_policy_template(DEFAULT_CSP_POLICY_TEMPLATE, &[], &[]);
/// assert_eq!(empty, DEFAULT_CSP_POLICY);
///
/// let one = render_policy_template(
///     DEFAULT_CSP_POLICY_TEMPLATE,
///     &["sha256-abc".to_string()],
///     &[],
/// );
/// assert!(one.contains("script-src 'self' 'sha256-abc';"));
/// ```
#[must_use]
pub fn render_policy_template(
    template: &str,
    script_hashes: &[String],
    style_hashes: &[String],
) -> String {
    let expand = |hashes: &[String]| -> String {
        let mut out = String::new();
        for h in hashes {
            out.push_str(" '");
            out.push_str(h);
            out.push('\'');
        }
        out
    };
    template
        .replace("{script_hashes}", &expand(script_hashes))
        .replace("{style_hashes}", &expand(style_hashes))
}

/// Computes the per-page Content-Security-Policy for a built HTML
/// page, or `None` when the page has no inline blocks and the global
/// [`computed_policy`] applies unchanged (spec B4).
///
/// The returned policy is [`DEFAULT_CSP_POLICY_TEMPLATE`] rendered
/// with the page's inline SHA-256 source hashes — hash-strict, never
/// containing `'unsafe-inline'`.
///
/// # Examples
///
/// ```rust
/// use ssg::csp::page_policy;
///
/// assert!(page_policy("<html><head></head><body></body></html>").is_none());
///
/// let html = r#"<script type="application/ld+json">{"@type":"Thing"}</script>"#;
/// let policy = page_policy(html).expect("inline JSON-LD yields a policy");
/// assert!(policy.contains("'sha256-"));
/// assert!(!policy.contains("unsafe-inline"));
/// ```
#[must_use]
pub fn page_policy(html: &str) -> Option<String> {
    let hashes = page_inline_hashes(html);
    if hashes.is_empty() {
        return None;
    }
    Some(render_policy_template(
        DEFAULT_CSP_POLICY_TEMPLATE,
        &hashes.scripts,
        &hashes.styles,
    ))
}

/// Collects the raw inner contents of every non-empty inline
/// `<tag>…</tag>` block, in document order. `<script>` elements with
/// a `src=` attribute are skipped (they are external, not inline).
/// Inline `<script>` and `<style>` bodies, collected in **one** parse.
///
/// `collect_inline_contents` walks the document once per tag, so hashing a
/// page for CSP cost two full parses — the parser is the expensive part, and
/// paying for it twice to read one document is waste, not safety.
///
/// Each element accumulates into its own slot and flushes on its end tag.
/// Text arrives in chunks and adjacent elements would otherwise concatenate,
/// which would hash two scripts as one and silently admit a policy that
/// matches neither.
fn collect_inline_script_and_style(html: &str) -> (Vec<String>, Vec<String>) {
    use std::cell::RefCell;
    use std::rc::Rc;

    use lol_html::{element, end_tag, text};

    use crate::util::html_rewriter::rewrite_html;

    type Slot = Rc<RefCell<Option<String>>>;
    type Sink = Rc<RefCell<Vec<String>>>;

    fn handlers<'a>(
        tag: &'a str,
        slot: &Slot,
        sink: &Sink,
    ) -> Vec<(
        std::borrow::Cow<'a, lol_html::Selector>,
        lol_html::ElementContentHandlers<'a>,
    )> {
        let slot_el = Rc::clone(slot);
        let sink_el = Rc::clone(sink);
        let slot_tx = Rc::clone(slot);

        let on_el = element!(tag, move |el| {
            // `src=` means external: there is no inline body to hash.
            if el.get_attribute("src").is_some() {
                *slot_el.borrow_mut() = None;
                return Ok(());
            }
            *slot_el.borrow_mut() = Some(String::new());
            let sink = Rc::clone(&sink_el);
            let slot = Rc::clone(&slot_el);
            let _ = el.on_end_tag(end_tag!(move |_end| {
                if let Some(body) = slot.borrow_mut().take() {
                    if !body.trim().is_empty() {
                        sink.borrow_mut().push(body);
                    }
                }
                Ok(())
            }));
            Ok(())
        });

        let on_text = text!(tag, move |chunk| {
            if let Some(buf) = slot_tx.borrow_mut().as_mut() {
                buf.push_str(chunk.as_str());
            }
            Ok(())
        });

        vec![on_el, on_text]
    }

    let script_slot: Slot = Rc::new(RefCell::new(None));
    let script_sink: Sink = Rc::new(RefCell::new(Vec::new()));
    let style_slot: Slot = Rc::new(RefCell::new(None));
    let style_sink: Sink = Rc::new(RefCell::new(Vec::new()));

    let mut all = handlers("script", &script_slot, &script_sink);
    all.extend(handlers("style", &style_slot, &style_sink));

    let _ = rewrite_html(html, all);

    let scripts = script_sink.borrow().clone();
    let styles = style_sink.borrow().clone();
    (scripts, styles)
}

/// Plugin that extracts inline styles/scripts to external files with SRI.
///
/// Runs in `after_compile` after all other content transforms but before
/// minification. For each HTML file:
///
/// 1. Finds `<style>…</style>` and `<script>…</script>` inline blocks
/// 2. Writes each block to `_csp/<hash>.css` or `_csp/<hash>.js`
/// 3. Replaces the inline block with a `<link>`/`<script src>` tag
///    including `integrity` and `crossorigin` attributes — SHA-384 by
///    default, configurable via `[security] sri_algorithm` in
///    `ssg.toml` (v0.0.47 plan §3 item 2.3)
/// 4. Rewrites any `<meta>` CSP tags to remove `'unsafe-inline'`
///
/// Blocks with `type="application/ld+json"` or `data-ssg-livereload`
/// attributes are skipped (structured data / dev-only scripts).
///
/// # Computed policy
///
/// The single string the plugin posture enforces is exposed via
/// [`computed_policy`]; downstream HTTP-header emitters (e.g. the
/// `edge_headers` postprocess plugin) call that function rather than
/// hardcoding a CSP string of their own, so the policy stays in
/// lock-step across `<meta>` injection and platform HTTP headers.
#[derive(Debug, Clone, Copy, Default)]
pub struct CspPlugin;

impl CspPlugin {
    /// Creates a new `CspPlugin`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::csp::CspPlugin;
    /// use ssg::plugin::Plugin;
    ///
    /// let p = CspPlugin::new();
    /// assert_eq!(p.name(), "csp");
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Plugin for CspPlugin {
    fn name(&self) -> &'static str {
        "csp"
    }

    fn has_transform(&self) -> bool {
        true
    }

    fn transform_html(
        &self,
        html: &str,
        path: &Path,
        ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        let csp_dir = ctx.site_dir.join("_csp");
        // `[security] sri_algorithm` from ssg.toml; absent config ⇒
        // SHA-384 (v0.0.47 plan §3 item 2.3).
        let sri_algorithm = ctx
            .config
            .as_ref()
            .map_or_else(SriAlgorithm::default, |c| c.security.sri_algorithm);
        // Extracted assets are referenced root-absolutely. When the site
        // is published under a sub-path (GitHub Pages project sites, and
        // any reverse-proxy mount), `/_csp/…` resolves against the domain
        // root and 404s, taking the whole stylesheet with it. Deriving the
        // prefix from `base_url` keeps the reference correct at any mount
        // point without the caller rewriting HTML afterwards.
        let url_prefix = ctx
            .config
            .as_ref()
            .map_or_else(String::new, |c| base_url_path_prefix(&c.base_url));
        let (rewritten, extracted) = extract_inline_blocks(
            html,
            &csp_dir,
            &ctx.site_dir,
            sri_algorithm,
            &url_prefix,
        )
        .map_err(|e| SsgError::io(e, path))?;

        if extracted > 0 {
            let final_html = remove_unsafe_inline_from_csp(&rewritten);
            Ok(final_html)
        } else {
            Ok(html.to_string())
        }
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        // Pre-create _csp/ dir so transform_html writers have the directory
        let csp_dir = ctx.site_dir.join("_csp");
        fs::create_dir_all(&csp_dir).with_path(&csp_dir)?;

        Ok(())
    }
}

/// Returns the path component of `base_url` as a prefix for
/// root-absolute asset references, without a trailing slash.
///
/// `https://example.com` and `https://example.com/` both yield `""`, so a
/// site at the domain root keeps emitting `/_csp/…` exactly as before.
/// `https://example.com/apex` yields `"/apex"`, making the emitted
/// reference `/apex/_csp/…`.
pub(crate) fn base_url_path_prefix(base_url: &str) -> String {
    let without_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let path = without_scheme
        .find('/')
        .map_or("", |i| &without_scheme[i..]);
    let trimmed = path.trim_end_matches('/');
    if trimmed == "/" {
        String::new()
    } else {
        trimmed.to_string()
    }
}

/// Extracts inline `<style>` and `<script>` blocks from HTML.
///
/// Returns `(rewritten_html, count_of_extracted_blocks)`.
fn extract_inline_blocks(
    html: &str,
    csp_dir: &Path,
    site_dir: &Path,
    sri_algorithm: SriAlgorithm,
    url_prefix: &str,
) -> Result<(String, usize)> {
    let mut result = html.to_string();
    let mut count = 0;
    let mut hoisted_links: Vec<String> = Vec::new();

    // Extract <style>…</style> blocks
    while let Some((before, content, after)) =
        find_inline_block(&result, "style")
    {
        let hash = fnv_hash(content.as_bytes());
        let filename = format!("{hash:016x}.css");
        let file_path = csp_dir.join(&filename);

        fs::create_dir_all(csp_dir)?;
        fs::write(&file_path, content.as_bytes())?;

        let sri = compute_sri(content.as_bytes(), sri_algorithm);
        let rel_path = file_path
            .strip_prefix(site_dir)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .replace('\\', "/");

        let link_tag = format!(
            "<link rel=\"stylesheet\" href=\"{}/{}\" integrity=\"{}\" crossorigin=\"anonymous\">",
            url_prefix, rel_path, sri
        );

        // The `<link>` is collected rather than dropped where the
        // `<style>` stood. Replacing in place put a stylesheet link in
        // `<body>` whenever the block it replaced was there — which for
        // every one of the nine published themes it was, twice per page.
        // HTML_CodeSniffer reports that as WCAG H59.1, and a stylesheet
        // discovered mid-body is a render-blocking fetch found late, so
        // it is a loading problem as much as a conformance one.
        hoisted_links.push(link_tag);
        result = format!("{before}{after}");
        count += 1;
    }

    // Put them at the end of `<head>`, in the order they were found, so
    // the cascade between them is unchanged. They now precede anything
    // in `<body>`, which is where a stylesheet belongs: a `<style>` in
    // the body was already being applied after the head's, and any
    // theme relying on that ordering would have been relying on
    // markup the CSP pass was about to rewrite anyway.
    if !hoisted_links.is_empty() {
        let block = hoisted_links.concat();
        // Parse rather than search for `</head>`. Splicing at the first byte
        // match puts the links inside a commented-out `</head>`, where they
        // are inert and nothing reports it (ssg#540 is the same fault found
        // elsewhere). `inject_before_head_close` uses the HTML rewriter and
        // returns its input unchanged when the document has no head, which
        // is the one case the fallback below still has to cover.
        let injected =
            crate::util::head_dom::inject_before_head_close(&result, &block);
        if injected == result {
            // No `<head>`: leave them where a browser will still find
            // them rather than dropping styling on the floor.
            result.push_str(&block);
        } else {
            result = injected;
        }
    }

    // Extract <script>…</script> blocks (skip JSON-LD and livereload)
    while let Some((before, opening_tag, content, after)) =
        find_inline_script(&result)
    {
        let hash = fnv_hash(content.as_bytes());
        let filename = format!("{hash:016x}.js");
        let file_path = csp_dir.join(&filename);

        fs::create_dir_all(csp_dir)?;
        fs::write(&file_path, content.as_bytes())?;

        let sri = compute_sri(content.as_bytes(), sri_algorithm);
        let rel_path = file_path
            .strip_prefix(site_dir)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .replace('\\', "/");

        // Preserve original attributes (e.g. type=module, async, defer,
        // data-*), but override src/integrity/crossorigin since we are
        // extracting the body to a new file and computing fresh SRI.
        let preserved = preserve_script_attrs(
            &opening_tag,
            &["src", "integrity", "crossorigin"],
        );
        let script_tag = if preserved.is_empty() {
            format!(
                "<script src=\"{url_prefix}/{rel_path}\" integrity=\"{sri}\" crossorigin=\"anonymous\"></script>"
            )
        } else {
            format!(
                "<script {preserved} src=\"{url_prefix}/{rel_path}\" integrity=\"{sri}\" crossorigin=\"anonymous\"></script>"
            )
        };

        result = format!("{before}{script_tag}{after}");
        count += 1;
    }

    Ok((result, count))
}

/// Parses attributes out of an opening `<script …>` tag and returns a
/// space-separated, normalised attribute string with `drop` attributes
/// removed. Boolean attributes (no `=`) are emitted unchanged.
///
/// Uses `lol_html` so quoting, whitespace, and case-folding all match
/// the HTML5 parser exactly — this is what the rest of the SSG
/// pipeline relies on for tag rewriting.
fn preserve_script_attrs(opening_tag: &str, drop: &[&str]) -> String {
    use crate::util::html_rewriter::rewrite_html;
    use lol_html::element;
    use std::cell::RefCell;
    use std::rc::Rc;

    // lol_html needs a closed element to fire the handler; wrap the
    // opening tag with an explicit close.
    let fragment = format!("{opening_tag}</script>");
    let collected: Rc<RefCell<Vec<(String, String)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let collected_cb = Rc::clone(&collected);

    let _ = rewrite_html(
        &fragment,
        vec![element!("script", move |el| {
            for attr in el.attributes() {
                collected_cb.borrow_mut().push((attr.name(), attr.value()));
            }
            Ok(())
        })],
    );

    let drop_lower: Vec<String> =
        drop.iter().map(|d| d.to_ascii_lowercase()).collect();

    let parts: Vec<String> = collected
        .borrow()
        .iter()
        .filter(|(name, _)| !drop_lower.contains(&name.to_ascii_lowercase()))
        .map(|(name, value)| {
            if value.is_empty() {
                name.clone()
            } else {
                let escaped = value.replace('"', "&quot;");
                format!("{name}=\"{escaped}\"")
            }
        })
        .collect();
    parts.join(" ")
}

/// Finds the first inline `<style>…</style>` block and returns
/// `(html_before, style_content, html_after)`.
fn find_inline_block<'a>(
    html: &'a str,
    tag: &str,
) -> Option<(&'a str, &'a str, &'a str)> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");

    // Same hazard as `find_inline_script`: a commented-out block would be
    // hoisted into a real external file and the page rewritten around it.
    let comments = comment_spans(html);
    let mut from = 0;
    let start = loop {
        let rel = html[from..].find(&open)?;
        let abs = from + rel;
        if inside_comment(&comments, abs) {
            from = abs + open.len();
            continue;
        }
        break abs;
    };
    let content_start = start + open.len();
    let content_end = html[content_start..].find(&close)? + content_start;
    let end = content_end + close.len();

    let content = &html[content_start..content_end];
    if content.trim().is_empty() {
        return None;
    }

    Some((&html[..start], content, &html[end..]))
}

/// Finds the first inline `<script>…</script>` block, skipping:
/// - `<script type="application/ld+json">` (structured data)
/// - `<script data-ssg-livereload>` (dev-only)
/// - `<script src="...">` (already external)
///
/// Returns `(before, opening_tag, content, after)` where `opening_tag`
/// is the full `<script …>` including angle brackets — the caller uses
/// it to preserve attributes such as `type=module`, `async`, `defer`,
/// and `data-*` when rewriting the tag.
/// Byte ranges covered by HTML comments, in document order.
///
/// The inline-block extractors reassemble the document by string surgery, so
/// they cannot be swapped for a streaming parser without restructuring the
/// whole extract-and-replace flow — on CSP/SRI code, which is not a change to
/// make in passing. Masking the comment spans fixes the demonstrated defect
/// exactly: a commented-out `<script>` was hoisted into a real external file
/// and the page rewritten around it (ssg#570).
fn comment_spans(html: &str) -> Vec<(usize, usize)> {
    let bytes = html.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while let Some(rel) = html[i..].find("<!--") {
        let start = i + rel;
        let after = start + 4;
        let end = html[after..]
            .find("-->")
            .map_or(bytes.len(), |r| after + r + 3);
        spans.push((start, end));
        i = end;
        if i >= bytes.len() {
            break;
        }
    }
    spans
}

/// True when `pos` falls inside an HTML comment.
fn inside_comment(spans: &[(usize, usize)], pos: usize) -> bool {
    spans.iter().any(|&(s, e)| pos >= s && pos < e)
}

fn find_inline_script(html: &str) -> Option<(String, String, String, String)> {
    let comments = comment_spans(html);
    let mut search_from = 0;

    loop {
        let rest = &html[search_from..];
        let start = rest.find("<script")?;
        let abs_start = search_from + start;

        // A `<script` inside a comment is not a script.
        if inside_comment(&comments, abs_start) {
            search_from = abs_start + "<script".len();
            continue;
        }

        // Find the end of the opening tag
        let tag_end = html[abs_start..].find('>')? + abs_start;
        let opening_tag = &html[abs_start..=tag_end];

        // Skip JSON-LD, livereload, and already-external scripts
        if opening_tag.contains("application/ld+json")
            || opening_tag.contains("data-ssg-livereload")
            || opening_tag.contains("src=")
        {
            search_from = tag_end + 1;
            continue;
        }

        let content_start = tag_end + 1;
        let close_tag = "</script>";
        let content_end =
            html[content_start..].find(close_tag)? + content_start;
        let end = content_end + close_tag.len();

        let content = &html[content_start..content_end];
        if content.trim().is_empty() {
            search_from = end;
            continue;
        }

        return Some((
            html[..abs_start].to_string(),
            opening_tag.to_string(),
            content.to_string(),
            html[end..].to_string(),
        ));
    }
}

/// Removes `'unsafe-inline'` from CSP `<meta>` tags in HTML.
///
/// Scoped to the policy value itself. A plain `html.replace` matched the token
/// anywhere on the page, so a document that quoted `'unsafe-inline'` while
/// explaining a policy — exactly what a security or documentation page does —
/// had the words silently deleted from its prose.
///
/// The token is removed together with the whitespace that separated it. Only
/// dropping the text turned `'self' 'unsafe-inline' 'unsafe-eval'` into
/// `'self'  'unsafe-eval'`, and the old `"  ;" -> " ;"` pass tidied just the
/// case where the token ended a directive.
fn remove_unsafe_inline_from_csp(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(start) = find_csp_meta_content(rest) {
        let (before, from_quote) = rest.split_at(start);
        let Some(quote) = from_quote.chars().next() else {
            break;
        };
        let Some(end_rel) = from_quote[1..].find(quote) else {
            break;
        };
        let value = &from_quote[1..=end_rel];

        out.push_str(before);
        out.push(quote);
        out.push_str(&strip_unsafe_inline_token(value));
        out.push(quote);

        rest = &from_quote[end_rel + 2..];
    }

    out.push_str(rest);
    out
}

/// Removes the `'unsafe-inline'` source expression from one policy string,
/// collapsing the separator it leaves behind.
fn strip_unsafe_inline_token(policy: &str) -> String {
    policy
        .split(';')
        .map(|directive| {
            let kept: Vec<&str> = directive
                .split_whitespace()
                .filter(|t| *t != "'unsafe-inline'")
                .collect();
            if kept.is_empty() {
                String::new()
            } else {
                // These policies are written with one space after each `;`,
                // so keeping it means only the removed token changes.
                let lead = if directive.starts_with(' ') { " " } else { "" };
                format!("{lead}{}", kept.join(" "))
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Locates the opening quote of a CSP `<meta>` tag's `content` attribute.
///
/// Returns an index into `html` pointing at the quote character itself, so the
/// caller can read the value without re-parsing the tag.
fn find_csp_meta_content(html: &str) -> Option<usize> {
    let mut search_from = 0usize;
    loop {
        let tag_rel = html[search_from..].find("<meta")?;
        let tag_start = search_from + tag_rel;
        let tag_end = html[tag_start..].find('>').map(|i| tag_start + i)?;
        let tag = &html[tag_start..tag_end];

        if tag.to_ascii_lowercase().contains("content-security-policy") {
            if let Some(attr_rel) = tag.find("content=") {
                let after = tag_start + attr_rel + "content=".len();
                if matches!(html.as_bytes().get(after), Some(b'"' | b'\'')) {
                    return Some(after);
                }
            }
        }
        search_from = tag_end;
    }
}

/// Directives a `<meta http-equiv>` policy must not carry.
///
/// The CSP specification only honours these when the policy arrives as
/// an HTTP header. Delivered in a `<meta>` element a browser ignores
/// them *and says so*: Chrome logs
///
/// > The Content Security Policy directive 'frame-ancestors' is ignored
/// > when delivered via a `<meta>` element.
///
/// which is a console error on every page — Lighthouse fails
/// `errors-in-console`, and the protection the directive was there to
/// provide was never in force. Emitting it is worse than omitting it,
/// because the policy reads as though the site is protected.
///
/// They stay in the policy used for header delivery, where they work.
const META_INELIGIBLE_DIRECTIVES: [&str; 4] =
    ["frame-ancestors", "report-uri", "report-to", "sandbox"];

/// Strips the directives a `<meta>`-delivered policy cannot carry.
///
/// # Examples
///
/// ```
/// use ssg::csp::policy_for_meta;
///
/// let p = "default-src 'self'; frame-ancestors 'none'";
/// assert_eq!(policy_for_meta(p), "default-src 'self'");
/// ```
#[must_use]
pub fn policy_for_meta(policy: &str) -> String {
    policy
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .filter(|d| {
            let name = d.split_whitespace().next().unwrap_or("");
            !META_INELIGIBLE_DIRECTIVES
                .iter()
                .any(|bad| name.eq_ignore_ascii_case(bad))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Inserts a `<meta http-equiv="Content-Security-Policy" content="...">`
/// tag immediately after the `<head>` opening tag.
///
/// Uses `lol_html` so the insertion point is the correct one regardless
/// of whitespace, comments, or `<title>` placement inside the head
/// (issue #525 AC7). If the document already contains a CSP meta tag
/// (either matching `policy` exactly or any other CSP policy), the
/// input is returned unchanged so successive calls are idempotent.
/// If no `<head>` element exists in the input, the function returns
/// the input verbatim — this matches the convention used by the
/// rest of the SSG HTML post-processors (no implicit head injection).
///
/// # Examples
///
/// ```rust
/// use ssg::csp::inject_csp_meta;
///
/// let html = "<html><head><title>t</title></head></html>";
/// let out = inject_csp_meta(html, "default-src 'self'");
/// assert!(out.contains("Content-Security-Policy"));
/// ```
///
/// # Errors
///
/// Returns the input unchanged when the underlying `lol_html` rewrite
/// fails; in practice the only failure mode is allocation exhaustion.
#[must_use]
pub fn inject_csp_meta(html: &str, policy: &str) -> String {
    use crate::util::html_rewriter::rewrite_html;
    use lol_html::element;
    use lol_html::html_content::ContentType;
    use std::cell::Cell;
    use std::rc::Rc;

    // Idempotency: if a CSP meta already exists anywhere in the
    // document, do nothing. lol_html visits comments-aware so this
    // false-positive-free.
    let already_present = Rc::new(Cell::new(false));
    let already_present_cb = Rc::clone(&already_present);
    let detect = element!(
        "meta[http-equiv=\"Content-Security-Policy\" i]",
        move |_el| {
            already_present_cb.set(true);
            Ok(())
        }
    );
    let _ = rewrite_html(html, vec![detect]);
    if already_present.get() {
        return html.to_string();
    }

    // Header-only directives are dropped here, not upstream: the same
    // policy is still correct for header delivery.
    let policy = policy_for_meta(policy);
    let injected = Rc::new(Cell::new(false));
    let injected_cb = Rc::clone(&injected);
    let head_handler = element!("head", move |el| {
        let meta = format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">"
        );
        el.prepend(&meta, ContentType::Html);
        injected_cb.set(true);
        Ok(())
    });

    rewrite_or_original(html, rewrite_html(html, vec![head_handler]))
}

/// Unwraps a rewrite result, returning the original HTML unchanged
/// when `lol_html` failed (in practice only allocation exhaustion).
fn rewrite_or_original(html: &str, res: Result<String, SsgError>) -> String {
    res.unwrap_or_else(|_| html.to_string())
}

/// FNV-1a 64-bit hash.
fn fnv_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Computes an SRI attribute value: `<algo>-<base64(digest)>`,
/// SHA-384 by default.
///
/// IMPORTANT DISTINCTION (v0.0.47 plan §3 item 2.3): this algorithm
/// knob governs only the `integrity=` **attribute** on externalized
/// assets. CSP **directive source hashes** — the `'sha256-…'` entries
/// inside a Content-Security-Policy header/meta value — must stay
/// SHA-256 for the broadest UA compatibility. SRI attributes and CSP
/// source hashes are different mechanisms; do not route CSP directive
/// hashes through this function's configurable algorithm.
fn compute_sri(data: &[u8], sri_algorithm: SriAlgorithm) -> String {
    sri_algorithm.integrity(data)
}

#[cfg(test)]
mod tests {

    /// The `<style>` extractor carries the same hazard as the script one.
    #[test]
    fn inline_block_extraction_skips_a_commented_block() {
        let html = concat!(
            "<html><head>",
            "<!-- <style>.commented{}</style> -->",
            "<style>.real{}</style>",
            "</head><body></body></html>"
        );
        let (_, content, _) =
            find_inline_block(html, "style").expect("real style found");
        assert_eq!(
            content.trim(),
            ".real{}",
            "a commented-out style must not be hoisted: {content:?}"
        );
    }

    /// ssg#570, second shape: the extractors that hoist inline blocks into
    /// external files scan for the same literal bytes. Matching a
    /// commented-out block would hoist the comment's body into a real file
    /// and rewrite the page around it — corrupting the document, not merely
    /// adding a spurious hash.
    #[test]
    fn inline_script_extraction_skips_a_commented_block() {
        let html = concat!(
            "<html><head>",
            "<!-- <script>commented()</script> -->",
            "<script>real()</script>",
            "</head><body></body></html>"
        );
        let found = find_inline_script(html);
        assert!(found.is_some(), "the real script should still be found");
        let (_, _, content, _) = found.unwrap();
        assert_eq!(
            content.trim(),
            "real()",
            "a commented-out script must not be hoisted: {content:?}"
        );
    }

    /// ssg#570: a `<script>` inside an HTML comment is not a script. The
    /// byte scan collects it anyway, so CSP gains a hash for code that can
    /// never execute — and the policy silently drifts from the document it
    /// is meant to describe.
    #[test]
    fn inline_collection_skips_a_script_inside_a_comment() {
        let html = concat!(
            "<html><head>",
            "<!-- <script>commented()</script> -->",
            "<script>real()</script>",
            "</head><body></body></html>"
        );
        let found = collect_inline_script_and_style(html).0;
        assert_eq!(
            found,
            vec!["real()"],
            "a commented-out script must not be hashed: {found:?}"
        );
    }

    use super::*;
    use tempfile::tempdir;

    /// `frame-ancestors` in a `<meta>` policy is ignored by every
    /// browser and logged as a console error, so the protection was
    /// never in force while the policy read as though it were.
    #[test]
    fn a_meta_policy_drops_directives_meta_cannot_carry() {
        let out = policy_for_meta(DEFAULT_CSP_POLICY);
        assert!(
            !out.contains("frame-ancestors"),
            "frame-ancestors must not reach a meta policy: {out}"
        );
        for kept in ["default-src", "script-src", "style-src", "img-src"] {
            assert!(out.contains(kept), "{kept} must survive: {out}");
        }
        assert!(!out.ends_with(';'), "no dangling separator: {out}");
    }

    #[test]
    fn policy_for_meta_drops_every_header_only_directive() {
        let p = "default-src 'self'; frame-ancestors 'none'; \
                 report-uri /r; report-to grp; sandbox allow-forms";
        assert_eq!(policy_for_meta(p), "default-src 'self'");
    }

    /// The injected tag itself must be clean, not merely the helper.
    #[test]
    fn the_injected_meta_tag_carries_no_frame_ancestors() {
        let html = "<html><head><title>t</title></head><body></body></html>";
        let out = inject_csp_meta(html, DEFAULT_CSP_POLICY);
        assert!(out.contains("Content-Security-Policy"), "{out}");
        assert!(
            !out.contains("frame-ancestors"),
            "the emitted tag must not carry it: {out}"
        );
    }

    /// A `<style>` block in the body used to leave its replacement
    /// `<link rel="stylesheet">` in the body. Every one of the nine
    /// published themes did exactly that, twice per page, and pa11y
    /// reported all eighteen as WCAG H59.1. A stylesheet found mid-body
    /// is also a render-blocking fetch discovered late.
    #[test]
    fn an_extracted_stylesheet_link_is_hoisted_into_head() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path();
        let html = concat!(
            "<html><head><title>t</title></head>",
            "<body><p>copy</p><style>body{margin:0}</style></body></html>"
        );
        let (out, n) = extract_inline_blocks(
            html,
            &site.join("_csp"),
            site,
            SriAlgorithm::Sha384,
            "",
        )
        .expect("extract");

        assert_eq!(n, 1, "one block should have been extracted");
        let head_end = out.find("</head>").expect("head");
        let link = out.find("<link rel=\"stylesheet\"").expect("link emitted");
        assert!(
            link < head_end,
            "the stylesheet link must land inside <head>, got: {out}"
        );
        assert!(
            !out[out.find("<body").unwrap()..]
                .contains("<link rel=\"stylesheet\""),
            "no stylesheet link may remain in <body>: {out}"
        );
    }

    /// A `</head>` inside a comment is not the head's end tag.
    ///
    /// The hoist spliced at the first byte match of the literal string, so a
    /// commented-out `</head>` earlier in the document captured the links:
    /// they land inside the comment, inert, and the page loses its styling
    /// with no error anywhere. `head_dom` already parses instead of
    /// searching, and ssg#540 records the same fault being fixed there;
    /// this path was still doing it by hand.
    #[test]
    fn hoisted_links_ignore_a_head_close_inside_a_comment() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path();
        let html = concat!(
            "<html><head><!-- </head> --><title>T</title></head><body>",
            "<style>.a{color:red}</style>",
            "</body></html>"
        );
        let (out, n) = extract_inline_blocks(
            html,
            &site.join("_csp"),
            site,
            SriAlgorithm::Sha384,
            "",
        )
        .expect("extract");
        assert_eq!(n, 1);

        let link = out
            .find("<link rel=\"stylesheet\"")
            .expect("a stylesheet link was emitted");
        let comment_end = out.find("-->").expect("the comment survives");
        assert!(
            link > comment_end,
            "the link was spliced inside the comment, where it does nothing:\n{out}"
        );
    }

    /// Two blocks keep their relative order, so the cascade between
    /// them survives the move.
    #[test]
    fn hoisted_links_keep_their_relative_order() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path();
        let html = concat!(
            "<html><head></head><body>",
            "<style>.a{color:red}</style>",
            "<style>.b{color:blue}</style>",
            "</body></html>"
        );
        let (out, n) = extract_inline_blocks(
            html,
            &site.join("_csp"),
            site,
            SriAlgorithm::Sha384,
            "",
        )
        .expect("extract");
        assert_eq!(n, 2);
        // The file named first in the document must be linked first.
        let links: Vec<&str> = out
            .match_indices("<link rel=\"stylesheet\"")
            .map(|(i, _)| &out[i..i + 90])
            .collect();
        assert_eq!(links.len(), 2, "both links present: {out}");
        let a = fs::read_to_string(
            site.join("_csp").join(
                links[0]
                    .split("href=\"/")
                    .nth(1)
                    .and_then(|s| s.split('"').next())
                    .and_then(|s| s.rsplit('/').next())
                    .expect("first href"),
            ),
        )
        .expect("first file");
        assert!(a.contains("red"), "first link should be the first style");
    }

    #[test]
    fn extract_style_block() {
        let html = "<html><head><style>body { color: red; }</style></head><body></body></html>";
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");

        let (result, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();

        assert_eq!(count, 1);
        assert!(result.contains("<link rel=\"stylesheet\""));
        // Default SRI algorithm is SHA-384 (v0.0.47 plan §3 item 2.3).
        assert!(result.contains("integrity=\"sha384-"));
        assert!(!result.contains("<style>"));
    }

    #[test]
    fn extract_style_block_default_sha384_exact_vector() {
        // Known vector: the style body is written to disk verbatim
        // (no minification in this plugin), so the integrity value is
        // exactly base64(SHA-384("body { color: red; }")).
        let html = "<html><head><style>body { color: red; }</style></head><body></body></html>";
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");

        let (result, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();

        assert_eq!(count, 1);
        assert!(
            result.contains(
                "integrity=\"sha384-BN8siYsJqlPeNsRFs2pYbTW0uiUBy9v6JVVKpHaS+KNqD0ZFotD5OFKMkI6/s6sb\""
            ),
            "expected exact SHA-384 SRI vector; got: {result}"
        );
    }

    #[test]
    fn base_url_path_prefix_extracts_sub_path_mount_points() {
        assert_eq!(base_url_path_prefix("https://example.com"), "");
        assert_eq!(base_url_path_prefix("https://example.com/"), "");
        assert_eq!(base_url_path_prefix("https://example.com/apex"), "/apex");
        assert_eq!(base_url_path_prefix("https://example.com/apex/"), "/apex");
        assert_eq!(base_url_path_prefix("https://e.com/a/b/"), "/a/b");
        // Missing scheme still yields the path component.
        assert_eq!(base_url_path_prefix("example.com/apex"), "/apex");
    }

    /// Regression: a site published under a sub-path emitted
    /// `href="/_csp/…"`, which resolves against the domain root and 404s,
    /// so the extracted stylesheet never loaded on GitHub Pages project
    /// sites.
    #[test]
    fn extracted_assets_are_prefixed_for_sub_path_deploys() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let csp = site.join("_csp");
        fs::create_dir_all(&csp).unwrap();

        let (out, count) = extract_inline_blocks(
            "<style>body{color:red}</style><script>var x=1;</script>",
            &csp,
            &site,
            SriAlgorithm::default(),
            "/apex",
        )
        .unwrap();

        assert_eq!(count, 2);
        assert!(
            out.contains("href=\"/apex/_csp/"),
            "stylesheet must carry the sub-path prefix: {out}"
        );
        assert!(
            out.contains("src=\"/apex/_csp/"),
            "script must carry the sub-path prefix: {out}"
        );
        assert!(
            !out.contains("href=\"/_csp/"),
            "no unprefixed root-absolute reference may survive: {out}"
        );
    }

    #[test]
    fn transform_html_sri_algorithm_config_override_emits_sha256() {
        // `[security] sri_algorithm = "sha256"` back-compat knob
        // (v0.0.47 plan §3 item 2.3), wired through PluginContext
        // config: exact SHA-256 vector for "body { color: red; }".
        use crate::cmd::{SecurityConfig, SsgConfig};

        let html = "<html><head><style>body { color: red; }</style></head><body></body></html>";
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        let config = SsgConfig::builder()
            .security(SecurityConfig {
                sri_algorithm: SriAlgorithm::Sha256,
            })
            .build()
            .unwrap();
        let ctx = PluginContext::with_config(
            dir.path(),
            dir.path(),
            &site,
            dir.path(),
            config,
        );
        let out = CspPlugin
            .transform_html(html, &site.join("index.html"), &ctx)
            .unwrap();
        assert!(
            out.contains(
                "integrity=\"sha256-XeYlw2NVzOfB1UCIJqCyGr+0n7bA4fFslFpvKu84IAw=\""
            ),
            "expected exact SHA-256 SRI vector; got: {out}"
        );
        assert!(!out.contains("sha384-"), "override must win: {out}");
    }

    #[test]
    fn extract_script_block() {
        let html =
            "<html><body><script>console.log('hi');</script></body></html>";
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");

        let (result, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();

        assert_eq!(count, 1);
        assert!(result.contains("<script src="));
        // Default SRI algorithm is SHA-384 (v0.0.47 plan §3 item 2.3).
        assert!(result.contains("integrity=\"sha384-"));
        assert!(!result.contains("console.log"));
    }

    #[test]
    fn skips_jsonld_scripts() {
        let html = r#"<html><body><script type="application/ld+json">{"@type":"Thing"}</script></body></html>"#;
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");

        let (result, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();

        assert_eq!(count, 0);
        assert!(result.contains("application/ld+json"));
    }

    #[test]
    fn skips_livereload_scripts() {
        let html = r#"<html><body><script data-ssg-livereload>ws.connect();</script></body></html>"#;
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");

        let (result, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();

        assert_eq!(count, 0);
        assert!(result.contains("data-ssg-livereload"));
    }

    #[test]
    fn skips_external_scripts() {
        let html =
            r#"<html><body><script src="/app.js"></script></body></html>"#;
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");

        let (result, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();

        assert_eq!(count, 0);
        assert_eq!(result, html);
    }

    #[test]
    fn removes_unsafe_inline_from_csp() {
        let html = r#"<meta http-equiv="Content-Security-Policy" content="script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'">"#;
        let result = remove_unsafe_inline_from_csp(html);
        assert!(!result.contains("unsafe-inline"));
    }

    #[test]
    fn removing_unsafe_inline_leaves_no_double_space() {
        // The token sits between two others, so dropping just the text used to
        // leave `'self'  'unsafe-eval'` behind in every built page.
        let html = r#"<meta http-equiv="Content-Security-Policy" content="script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://example.com">"#;
        let result = remove_unsafe_inline_from_csp(html);
        assert!(!result.contains("  "), "double space in {result}");
        assert!(result.contains("script-src 'self' 'unsafe-eval';"));
        assert!(result.contains("style-src 'self' https://example.com"));
    }

    #[test]
    fn leaves_prose_mentioning_unsafe_inline_alone() {
        // A page explaining a policy quotes the token in its own text. The
        // previous whole-document replace deleted it from the prose.
        let html = concat!(
            r#"<meta http-equiv="Content-Security-Policy" content="script-src 'self' 'unsafe-inline'">"#,
            r#"<p>Set <code>script-src 'self' 'unsafe-inline'</code> to allow it.</p>"#
        );
        let result = remove_unsafe_inline_from_csp(html);
        assert!(
            result.contains("<code>script-src 'self' 'unsafe-inline'</code>"),
            "prose was rewritten: {result}"
        );
        // ...while the policy itself is still stripped.
        let meta_end = result.find("<p>").expect("prose follows the meta");
        assert!(!result[..meta_end].contains("unsafe-inline"));
    }

    #[test]
    fn leaves_documents_without_a_csp_meta_untouched() {
        let html = r#"<p>The token 'unsafe-inline' weakens a policy.</p>"#;
        assert_eq!(remove_unsafe_inline_from_csp(html), html);
    }

    #[test]
    fn skips_empty_style_blocks() {
        let html = "<html><head><style>  </style></head></html>";
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");

        let (_, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn csp_plugin_name() {
        assert_eq!(CspPlugin.name(), "csp");
    }

    #[test]
    fn csp_plugin_skips_missing_site_dir() {
        let ctx = PluginContext::new(
            Path::new("/tmp/c"),
            Path::new("/tmp/b"),
            Path::new("/nonexistent/site"),
            Path::new("/tmp/t"),
        );
        assert!(CspPlugin.after_compile(&ctx).is_ok());
    }

    #[test]
    fn csp_plugin_processes_html_files() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let html = "<html><head><style>body{color:red}</style></head><body><script>alert(1)</script></body></html>";

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        CspPlugin.after_compile(&ctx).unwrap();

        let output = CspPlugin
            .transform_html(html, &site.join("index.html"), &ctx)
            .unwrap();
        assert!(output.contains("<link rel=\"stylesheet\""));
        assert!(output.contains("<script src="));
        assert!(!output.contains("body{color:red}"));
        assert!(!output.contains("alert(1)"));
        assert!(site.join("_csp").exists());
    }

    #[test]
    fn fnv_hash_deterministic() {
        let h1 = fnv_hash(b"hello");
        let h2 = fnv_hash(b"hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn fnv_hash_different_inputs() {
        assert_ne!(fnv_hash(b"a"), fnv_hash(b"b"));
    }

    #[test]
    fn compute_sri_format() {
        // Default algorithm ⇒ sha384- prefix; explicit sha256/sha512
        // overrides carry their own prefixes.
        let sri = compute_sri(b"test", SriAlgorithm::default());
        assert!(sri.starts_with("sha384-"));
        assert!(
            compute_sri(b"test", SriAlgorithm::Sha256).starts_with("sha256-")
        );
        assert!(
            compute_sri(b"test", SriAlgorithm::Sha512).starts_with("sha512-")
        );
    }

    // ── CspPlugin::new + inject_csp_meta coverage ───────────────────

    #[test]
    fn csp_plugin_new_constructs_unit_struct() {
        let p = CspPlugin::new();
        assert_eq!(p.name(), "csp");
        assert!(p.has_transform());
    }

    #[test]
    fn inject_csp_meta_adds_meta_when_absent() {
        let html = "<html><head><title>T</title></head><body></body></html>";
        let out = inject_csp_meta(html, "default-src 'self'");
        assert!(out.contains("http-equiv=\"Content-Security-Policy\""));
        assert!(out.contains("default-src 'self'"));
    }

    #[test]
    fn inject_csp_meta_is_idempotent_when_meta_already_present() {
        let html = r#"<html><head><meta http-equiv="Content-Security-Policy" content="default-src 'self'"></head></html>"#;
        let out = inject_csp_meta(html, "script-src 'self'");
        // Should not duplicate.
        let count = out
            .matches("http-equiv=\"Content-Security-Policy\"")
            .count();
        assert_eq!(count, 1, "must not duplicate CSP meta tag");
        // And must not insert the new policy.
        assert!(!out.contains("script-src 'self'"));
    }

    #[test]
    fn inject_csp_meta_handles_no_head_gracefully() {
        let html = "<html><body>no head</body></html>";
        let out = inject_csp_meta(html, "default-src 'self'");
        // Without a <head>, lol_html returns the input unchanged.
        assert_eq!(out, html);
    }

    #[test]
    fn preserve_script_attrs_keeps_type_module() {
        let out = preserve_script_attrs(
            r#"<script type="module">"#,
            &["src", "integrity", "crossorigin"],
        );
        assert!(out.contains(r#"type="module""#), "got: {out}");
    }

    #[test]
    fn preserve_script_attrs_keeps_boolean_async_defer() {
        let out = preserve_script_attrs(
            "<script async defer>",
            &["src", "integrity", "crossorigin"],
        );
        assert!(out.contains("async"), "got: {out}");
        assert!(out.contains("defer"), "got: {out}");
    }

    #[test]
    fn preserve_script_attrs_drops_listed_attrs() {
        let out = preserve_script_attrs(
            r#"<script src="/x.js" integrity="sha384-foo" crossorigin="anonymous" data-id="9">"#,
            &["src", "integrity", "crossorigin"],
        );
        assert!(!out.contains("src="), "got: {out}");
        assert!(!out.contains("integrity="), "got: {out}");
        assert!(!out.contains("crossorigin="), "got: {out}");
        assert!(out.contains(r#"data-id="9""#), "got: {out}");
    }

    #[test]
    fn preserve_script_attrs_empty_when_no_attrs() {
        let out = preserve_script_attrs("<script>", &["src"]);
        assert_eq!(out, "");
    }

    #[test]
    fn extract_inline_script_preserves_type_module() {
        let html = r#"<html><body><script type="module">import x from '/m.js';</script></body></html>"#;
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");
        let (out, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();
        assert_eq!(count, 1);
        assert!(out.contains(r#"type="module""#), "got: {out}");
        assert!(out.contains("integrity=\"sha384-"), "got: {out}");
    }

    #[test]
    fn extract_inline_script_preserves_data_attrs() {
        let html = r#"<html><body><script data-domain="example.com">window.x=1;</script></body></html>"#;
        let dir = tempdir().unwrap();
        let csp_dir = dir.path().join("_csp");
        let (out, count) = extract_inline_blocks(
            html,
            &csp_dir,
            dir.path(),
            SriAlgorithm::default(),
            "",
        )
        .unwrap();
        assert_eq!(count, 1);
        assert!(out.contains(r#"data-domain="example.com""#), "got: {out}");
    }

    // ── per-page CSP (spec B4, plan §3 item 2.4) ────────────────────

    /// The exact base64(SHA-256(...)) of `{"@type":"Thing"}` — the
    /// JSON-LD body used across the B4 tests.
    const JSONLD_BODY: &str = r#"{"@type":"Thing"}"#;

    #[test]
    fn template_with_empty_slots_is_exactly_the_global_policy() {
        // Invariant promised by DEFAULT_CSP_POLICY_TEMPLATE's docs:
        // no hashes ⇒ byte-identical to DEFAULT_CSP_POLICY.
        assert_eq!(
            render_policy_template(DEFAULT_CSP_POLICY_TEMPLATE, &[], &[]),
            DEFAULT_CSP_POLICY
        );
    }

    #[test]
    fn page_inline_hashes_includes_jsonld_blocks() {
        let html = format!(
            r#"<html><body><script type="application/ld+json">{JSONLD_BODY}</script></body></html>"#
        );
        let hashes = page_inline_hashes(&html);
        assert_eq!(hashes.scripts.len(), 1);
        let expected = SriAlgorithm::Sha256.integrity(JSONLD_BODY.as_bytes());
        assert_eq!(hashes.scripts[0], expected);
    }

    #[test]
    fn page_inline_hashes_skips_external_scripts_and_empty_blocks() {
        let html = r#"<html><body>
            <script src="/app.js"></script>
            <script>   </script>
            <style></style>
        </body></html>"#;
        assert!(page_inline_hashes(html).is_empty());
    }

    #[test]
    fn page_inline_hashes_orders_by_document_position_and_dedups() {
        let html =
            "<script>aaa</script><script>bbb</script><script>aaa</script>";
        let hashes = page_inline_hashes(html);
        assert_eq!(hashes.scripts.len(), 2, "duplicate block deduped");
        assert_eq!(
            hashes.scripts[0],
            SriAlgorithm::Sha256.integrity(b"aaa"),
            "document order preserved"
        );
        assert_eq!(hashes.scripts[1], SriAlgorithm::Sha256.integrity(b"bbb"));
    }

    #[test]
    fn page_inline_hashes_collects_styles_separately() {
        let html = "<style>body{color:red}</style><script>alert(1)</script>";
        let hashes = page_inline_hashes(html);
        assert_eq!(hashes.styles.len(), 1);
        assert_eq!(hashes.scripts.len(), 1);
        assert_eq!(
            hashes.styles[0],
            SriAlgorithm::Sha256.integrity(b"body{color:red}")
        );
    }

    #[test]
    fn page_policy_is_hash_strict_never_unsafe_inline() {
        // test_csp_strict analogue (spec B4 acceptance): when hashes
        // are present the policy carries them and no 'unsafe-inline'.
        let html = format!(
            r#"<script type="application/ld+json">{JSONLD_BODY}</script>"#
        );
        let policy = page_policy(&html).expect("policy for inline JSON-LD");
        let expected = SriAlgorithm::Sha256.integrity(JSONLD_BODY.as_bytes());
        assert!(
            policy.contains(&format!("script-src 'self' '{expected}'")),
            "policy must carry the exact sha256 source: {policy}"
        );
        assert!(!policy.contains("unsafe-inline"));
    }

    #[test]
    fn page_policy_none_without_inline_blocks() {
        assert!(page_policy(
            "<html><head><title>t</title></head><body>x</body></html>"
        )
        .is_none());
    }

    #[test]
    fn page_policy_is_deterministic() {
        let html = "<style>a{}</style><script>x=1</script>";
        assert_eq!(page_policy(html), page_policy(html));
    }

    #[test]
    fn csp_plugin_transform_html_no_inline_blocks_returns_unchanged() {
        let html =
            "<html><head><title>X</title></head><body><p>hi</p></body></html>";
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let out = CspPlugin
            .transform_html(html, &site.join("index.html"), &ctx)
            .unwrap();
        assert_eq!(out, html);
    }

    // -------------------------------------------------------------------
    // Parser edge branches
    // -------------------------------------------------------------------

    #[test]
    fn page_inline_hashes_dedupes_identical_style_blocks() {
        let html = "<style>a{color:red}</style><style>a{color:red}</style>";
        let hashes = page_inline_hashes(html);
        assert_eq!(hashes.styles.len(), 1);
    }

    #[test]
    fn inline_collection_skips_prefix_tag_names() {
        // `<styles>` must not match a `<style` opener lookup.
        let html = "<styles>ignored</styles><style>a{}</style>";
        let out = collect_inline_script_and_style(html).1;
        assert_eq!(out, vec!["a{}"]);
    }

    #[test]
    fn inline_collection_accepts_slash_after_tag_name() {
        // `<style/` is still a tag boundary for the opener check.
        let html = "<style/>a{}</style>";
        let out = collect_inline_script_and_style(html).1;
        assert_eq!(out, vec!["a{}"]);
    }

    #[test]
    fn inline_collection_stops_when_opening_tag_unterminated() {
        let html = "<style media=all";
        assert!(collect_inline_script_and_style(html).1.is_empty());
    }

    #[test]
    fn inline_collection_stops_when_close_tag_missing() {
        let html = "<style>a{} no closing fence";
        assert!(collect_inline_script_and_style(html).1.is_empty());
    }

    #[test]
    fn find_inline_block_returns_none_without_close_tag() {
        assert!(find_inline_block("<style>a{}", "style").is_none());
    }

    #[test]
    fn find_inline_script_returns_none_when_opening_unterminated() {
        assert!(find_inline_script("<script").is_none());
    }

    #[test]
    fn find_inline_script_returns_none_without_close_tag() {
        assert!(find_inline_script("<script>var x = 1;").is_none());
    }

    #[test]
    fn find_inline_script_skips_empty_script_then_finds_real_one() {
        let html = "<script>   </script><script>var x = 1;</script>";
        let (_, _, content, _) = find_inline_script(html).unwrap();
        assert_eq!(content, "var x = 1;");
    }

    #[test]
    fn rewrite_or_original_returns_input_on_error() {
        let err = SsgError::io(
            std::io::Error::other("synthetic rewrite failure"),
            "<lol_html>",
        );
        assert_eq!(rewrite_or_original("<p>x</p>", Err(err)), "<p>x</p>");
    }

    // -------------------------------------------------------------------
    // IO error branches
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_fails_when_csp_dir_squatted_by_file() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("_csp"), "not a dir").unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let err = CspPlugin.after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn transform_html_fails_when_csp_dir_squatted_by_file() {
        // extract_inline_blocks' create_dir_all fails, and the io
        // error is wrapped with the page path.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("_csp"), "not a dir").unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let err = CspPlugin
            .transform_html(
                "<style>a{}</style>",
                &site.join("index.html"),
                &ctx,
            )
            .unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn extract_inline_blocks_script_dir_create_fails_when_squatted_by_file() {
        // Script-only page: the style loop never runs, so the script
        // loop's own create_dir_all hits the squatted `_csp` file.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("_csp"), "not a dir").unwrap();

        let res = extract_inline_blocks(
            "<script>var x = 1;</script>",
            &site.join("_csp"),
            &site,
            SriAlgorithm::default(),
            "",
        );
        assert!(res.is_err());
    }

    #[test]
    fn extract_inline_blocks_style_write_fails_when_squatted_by_dir() {
        // The style payload filename is deterministic (FNV-1a of the
        // content); a directory squatting it makes fs::write fail.
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let csp_dir = site.join("_csp");
        let content = "a{color:red}";
        let squat =
            csp_dir.join(format!("{:016x}.css", fnv_hash(content.as_bytes())));
        fs::create_dir_all(&squat).unwrap();

        let res = extract_inline_blocks(
            &format!("<style>{content}</style>"),
            &csp_dir,
            &site,
            SriAlgorithm::default(),
            "",
        );
        assert!(res.is_err());
    }

    #[test]
    fn extract_inline_blocks_script_write_fails_when_squatted_by_dir() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let csp_dir = site.join("_csp");
        let content = "var x = 1;";
        let squat =
            csp_dir.join(format!("{:016x}.js", fnv_hash(content.as_bytes())));
        fs::create_dir_all(&squat).unwrap();

        let res = extract_inline_blocks(
            &format!("<script>{content}</script>"),
            &csp_dir,
            &site,
            SriAlgorithm::default(),
            "",
        );
        assert!(res.is_err());
    }
}
