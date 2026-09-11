// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! View Transitions API + lazy navigation (issue #547).
//!
//! Opt-in via `transitions = true` in `ssg.toml`. When enabled, the
//! plugin:
//!
//! 1. Writes `_transitions/ssg-transitions.js` to the site dir — a tiny
//!    (~3 KB) client script that:
//!    - Intercepts same-origin `<a>` clicks
//!    - Fetches the next page, swaps its `<main>` content
//!    - Wraps the swap in `document.startViewTransition(...)` where
//!      supported (Chromium + Safari 18+ as of 2026-06)
//!    - Falls back to the browser's default full-reload navigation in
//!      non-supporting browsers (Firefox stable as of 2026-06)
//!    - Skips cross-origin links, modified clicks
//!      (`ctrl`/`cmd`/`shift`/`alt`), non-`GET` targets and any link
//!      that opts out via `data-no-transition` or `target="_blank"`
//! 2. Injects a `<script type="module" defer>` tag and a tiny
//!    `<style>` block that names `header` and `footer` as persistent
//!    transition roots (so they don't animate across navigations —
//!    AC6).
//! 3. Dispatches a `ssg:after-swap` `CustomEvent` after each swap so
//!    the islands loader (and any other listener) can re-hydrate
//!    components on the new page.
//!
//! ## Architecture
//!
//! Builds on:
//!
//! - `<ssg-island>` (`crate::plugins::islands`) — exposes
//!   `connectedCallback` / `disconnectedCallback`, so swapping
//!   `<main>` detaches old islands' listeners cleanly (AC5).
//! - `LiveReloadPlugin` (`crate::server::livereload`) — in dev mode,
//!   the reload handler is upgraded to wrap `location.reload()` in
//!   `startViewTransition()` when structural changes occur (AC7).
//!
//! ## Bundle size budget
//!
//! The injected script must stay ≤ 5 KB uncompressed. Verified by a
//! unit test in this file.

use crate::cmd::SsgConfig;
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::util::html_rewriter::inject_before_body_close_or_append;
use std::{fs, path::Path};

/// Where the client script is written, relative to `site_dir`.
const SCRIPT_DIR: &str = "_transitions";

/// Public script URL used by the injected `<script>` tag.
const SCRIPT_URL: &str = "/_transitions/ssg-transitions.js";

/// Filename of the client script (also the cache marker).
const SCRIPT_FILENAME: &str = "ssg-transitions.js";

/// HTML comment / attribute we use to detect prior injection so the
/// plugin is idempotent (`transform_html` may run multiple times in
/// some incremental-rebuild paths).
const INJECT_MARKER: &str = "data-ssg-transitions";

/// Hard ceiling on the script size — keeps the network cost minimal.
/// Bumping this constant requires updating the issue acceptance budget.
#[cfg(test)]
const MAX_SCRIPT_BYTES: usize = 5 * 1024;

/// Plugin that injects the View Transitions API client + style hooks.
///
/// Opt-in via [`SsgConfig::transitions`]; when `false` the plugin is
/// never registered (see `core::pipeline::register_default_plugins`).
///
/// # Examples
///
/// ```
/// use ssg::plugin::Plugin;
/// use ssg::view_transitions::ViewTransitionsPlugin;
/// assert_eq!(ViewTransitionsPlugin::new().name(), "view-transitions");
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewTransitionsPlugin;

impl ViewTransitionsPlugin {
    /// Creates a new instance.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::view_transitions::ViewTransitionsPlugin;
    /// let _plugin = ViewTransitionsPlugin::new();
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Returns whether `cfg` opts in to transitions.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::cmd::SsgConfig;
    /// use ssg::view_transitions::ViewTransitionsPlugin;
    /// let cfg = SsgConfig::builder()
    ///     .site_name("t".into())
    ///     .base_url("http://example.com".into())
    ///     .build()
    ///     .unwrap();
    /// assert!(!ViewTransitionsPlugin::enabled(&cfg));
    /// ```
    #[must_use]
    pub const fn enabled(cfg: &SsgConfig) -> bool {
        cfg.transitions
    }
}

impl Plugin for ViewTransitionsPlugin {
    fn name(&self) -> &'static str {
        "view-transitions"
    }

    fn has_transform(&self) -> bool {
        true
    }

    fn transform_html(
        &self,
        html: &str,
        _path: &Path,
        _ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        if html.contains(INJECT_MARKER) {
            return Ok(html.to_string());
        }

        // Don't inject into HTML fragments without a closing </body> —
        // those are likely partials, not full pages.
        if !html.contains("</body>") && !html.contains("</html>") {
            return Ok(html.to_string());
        }

        let head_block = format!("    {INLINE_STYLE}\n");
        let script_tag = format!(
            "    <script type=\"module\" defer {INJECT_MARKER} src=\"{SCRIPT_URL}\"></script>\n"
        );

        // Inject style into <head> when present so persistent
        // transition roots are named before paint.
        // Parser-backed: a `</head>` inside a comment or script in the head
        // is not the head's end tag, and a byte splice cannot tell the
        // difference (ssg#540).
        let with_style =
            crate::util::head_dom::inject_before_head_close(html, &head_block);

        // Inject script just before </body> so the DOM is ready.
        let with_script =
            inject_before_body_close_or_append(&with_style, &script_tag);

        Ok(with_script)
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }
        if ctx.dry_run {
            return Ok(());
        }

        let dir = ctx.site_dir.join(SCRIPT_DIR);
        fs::create_dir_all(&dir).with_path(&dir)?;

        let path = dir.join(SCRIPT_FILENAME);
        fs::write(&path, VIEW_TRANSITIONS_JS).with_path(&path)?;

        log::info!(
            "[view-transitions] wrote client script ({} bytes)",
            VIEW_TRANSITIONS_JS.len(),
        );
        Ok(())
    }
}

/// Inline `<style>` block that names persistent transition roots.
///
/// Browsers without View Transitions support ignore the property
/// silently, so this is safe to ship unconditionally.
const INLINE_STYLE: &str = "<style data-ssg-transitions-style>\
header[role=\"banner\"],body>header{view-transition-name:ssg-header}\
footer[role=\"contentinfo\"],body>footer{view-transition-name:ssg-footer}\
main{view-transition-name:ssg-main}\
@media (prefers-reduced-motion: reduce){::view-transition-group(*),\
::view-transition-old(*),::view-transition-new(*){animation:none!important}}\
</style>";

/// The injected client script. Kept ≤ 5 KB — verified by a unit test.
///
/// Intentionally hand-written (not minified) for readability and
/// auditability. Browser support note: View Transitions API is
/// chromium-stable since 111 and Safari-stable since 18 (2024-09).
/// Firefox: in nightly behind a flag as of 2026-06 — falls back to
/// full-page navigation gracefully (AC2).
pub const VIEW_TRANSITIONS_JS: &str = r#"// SSG View Transitions client — issue #547
// Same-origin nav interception + lazy hydration coordination.
(() => {
  const NS = 'ssg-transitions';
  if (window[NS]) return; // idempotent
  window[NS] = true;

  const supportsVT = typeof document.startViewTransition === 'function';

  // --- Same-origin click interception -----------------------------------
  function shouldIntercept(ev, link) {
    if (ev.defaultPrevented) return false;
    if (ev.button !== 0) return false;
    if (ev.ctrlKey || ev.metaKey || ev.shiftKey || ev.altKey) return false;
    if (!link || !link.href) return false;
    if (link.target && link.target !== '_self') return false;
    if (link.hasAttribute('download')) return false;
    if (link.dataset && link.dataset.noTransition !== undefined) return false;
    const url = new URL(link.href, location.href);
    if (url.origin !== location.origin) return false; // AC3
    if (url.pathname === location.pathname && url.search === location.search) {
      return false; // pure-hash nav — let the browser handle it
    }
    return true;
  }

  async function fetchPage(url) {
    const res = await fetch(url, { credentials: 'same-origin' });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const text = await res.text();
    return new DOMParser().parseFromString(text, 'text/html');
  }

  function swap(doc) {
    // Swap <main> and update <title>. Header/footer stay in place
    // (they're named as persistent transition roots via CSS).
    const nextMain = doc.querySelector('main');
    const curMain = document.querySelector('main');
    if (nextMain && curMain) {
      // Tell outgoing islands to detach. Web Components' own
      // disconnectedCallback also runs after replaceWith, but firing
      // an explicit event lets userland clean up too (AC5).
      curMain.querySelectorAll('ssg-island').forEach((el) => {
        try { el.dispatchEvent(new CustomEvent('ssg:detach')); } catch (e) {}
        if (typeof el.detach === 'function') {
          try { el.detach(); } catch (e) {}
        }
      });
      curMain.replaceWith(nextMain);
    }
    if (doc.title) document.title = doc.title;

    // Re-fire DOMContentLoaded-equivalent so other listeners
    // (analytics, lazy-load shims) can rebind on the new page.
    document.dispatchEvent(
      new CustomEvent('ssg:after-swap', { detail: { url: location.href } })
    );
  }

  async function navigate(url, push) {
    try {
      const doc = await fetchPage(url);
      const run = () => swap(doc);
      if (supportsVT) {
        // startViewTransition returns a ViewTransition handle.
        // Awaiting `.finished` lets us catch animation errors.
        const t = document.startViewTransition(run);
        if (t && t.finished) {
          t.finished.catch(() => {}); // swallow user-aborted cancels
        }
      } else {
        run(); // AC2 — graceful fallback (no animation)
      }
      if (push) history.pushState({ ssgvt: 1 }, '', url);
      // Scroll to top for new navigations (mirrors browser default).
      window.scrollTo({ top: 0, left: 0, behavior: 'instant' });
    } catch (err) {
      // Network / parse failure: fall back to a hard navigation so
      // the user still gets to the destination.
      location.href = url;
    }
  }

  document.addEventListener('click', (ev) => {
    const link = ev.target && ev.target.closest && ev.target.closest('a[href]');
    if (!shouldIntercept(ev, link)) return;
    ev.preventDefault();
    navigate(link.href, true);
  });

  window.addEventListener('popstate', (ev) => {
    // Only handle popstates we originated — avoid stealing native
    // anchor-only navigations the browser still owns.
    if (ev.state && ev.state.ssgvt) navigate(location.href, false);
  });

  // --- HMR coordination (dev only) --------------------------------------
  // The livereload client (src/server/livereload.rs) consults this
  // flag before calling location.reload(). When transitions are
  // enabled, structural reloads are wrapped in startViewTransition so
  // the cross-fade is smooth even for full reloads (AC7).
  window.__ssgTransitionsReload = function (reload) {
    if (supportsVT) {
      try {
        document.startViewTransition(() => reload());
        return;
      } catch (e) {}
    }
    reload();
  };
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ctx_for(site: &Path) -> PluginContext {
        PluginContext::new(
            Path::new("/tmp/c"),
            Path::new("/tmp/b"),
            site,
            Path::new("/tmp/t"),
        )
    }

    #[test]
    fn plugin_name_is_stable() {
        assert_eq!(ViewTransitionsPlugin::new().name(), "view-transitions");
    }

    #[test]
    fn plugin_has_transform() {
        assert!(ViewTransitionsPlugin::new().has_transform());
    }

    #[test]
    fn script_is_within_budget() {
        // Hard budget — bumping this requires updating the issue AC.
        // (Plain assert: lazily-formatted message args would be
        // uncovered regions.)
        assert!(VIEW_TRANSITIONS_JS.len() <= MAX_SCRIPT_BYTES);
    }

    #[test]
    fn script_includes_supports_check() {
        // AC2: must detect support and fall back.
        assert!(VIEW_TRANSITIONS_JS.contains("startViewTransition"));
        assert!(VIEW_TRANSITIONS_JS.contains("supportsVT"));
    }

    #[test]
    fn script_includes_cross_origin_guard() {
        // AC3: same-origin only.
        assert!(VIEW_TRANSITIONS_JS.contains("url.origin !== location.origin"));
    }

    #[test]
    fn script_includes_modified_click_guard() {
        // No interception for cmd/ctrl/middle-click etc.
        assert!(VIEW_TRANSITIONS_JS.contains("metaKey"));
        assert!(VIEW_TRANSITIONS_JS.contains("ctrlKey"));
    }

    #[test]
    fn script_dispatches_lifecycle_event() {
        // AC5: islands on outgoing pages get a chance to detach.
        assert!(VIEW_TRANSITIONS_JS.contains("ssg:detach"));
        assert!(VIEW_TRANSITIONS_JS.contains("ssg:after-swap"));
    }

    #[test]
    fn script_exposes_hmr_hook() {
        // AC7: livereload calls window.__ssgTransitionsReload(reload).
        assert!(VIEW_TRANSITIONS_JS.contains("__ssgTransitionsReload"));
    }

    #[test]
    fn style_names_persistent_roots() {
        // AC6: header + footer are persistent.
        assert!(INLINE_STYLE.contains("ssg-header"));
        assert!(INLINE_STYLE.contains("ssg-footer"));
    }

    #[test]
    fn style_honours_prefers_reduced_motion() {
        // Accessibility: animations must not fire when the user opts out.
        assert!(INLINE_STYLE.contains("prefers-reduced-motion"));
        assert!(INLINE_STYLE.contains("animation:none"));
    }

    #[test]
    fn transform_adds_script_and_style() {
        let html = "<html><head><title>x</title></head><body><main>x</main></body></html>";
        let ctx = ctx_for(Path::new("/tmp/s"));
        let out = ViewTransitionsPlugin::new()
            .transform_html(html, Path::new("/tmp/x.html"), &ctx)
            .unwrap();
        assert!(out.contains(SCRIPT_URL));
        assert!(out.contains(INJECT_MARKER));
        assert!(out.contains("data-ssg-transitions-style"));
    }

    #[test]
    fn transform_is_idempotent() {
        let html = "<html><head></head><body></body></html>";
        let ctx = ctx_for(Path::new("/tmp/s"));
        let plugin = ViewTransitionsPlugin::new();
        let once = plugin
            .transform_html(html, Path::new("/tmp/x.html"), &ctx)
            .unwrap();
        let twice = plugin
            .transform_html(&once, Path::new("/tmp/x.html"), &ctx)
            .unwrap();
        assert_eq!(once, twice);
        assert_eq!(twice.matches(SCRIPT_URL).count(), 1);
    }

    #[test]
    fn transform_skips_fragment_html() {
        // A partial like a sitemap fragment without <body> shouldn't
        // get the script injected.
        let html = "<div><p>partial</p></div>";
        let ctx = ctx_for(Path::new("/tmp/s"));
        let out = ViewTransitionsPlugin::new()
            .transform_html(html, Path::new("/tmp/x.html"), &ctx)
            .unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn transform_injects_style_into_head() {
        let html = "<html><head><meta charset=\"utf-8\"></head><body><main></main></body></html>";
        let ctx = ctx_for(Path::new("/tmp/s"));
        let out = ViewTransitionsPlugin::new()
            .transform_html(html, Path::new("/tmp/x.html"), &ctx)
            .unwrap();
        // Style block must land inside <head>.
        let head_end = out.find("</head>").unwrap();
        let style_pos = out.find("data-ssg-transitions-style").unwrap();
        assert!(style_pos < head_end);
    }

    #[test]
    fn transform_injects_script_before_body_end() {
        let html = "<html><head></head><body><main>x</main></body></html>";
        let ctx = ctx_for(Path::new("/tmp/s"));
        let out = ViewTransitionsPlugin::new()
            .transform_html(html, Path::new("/tmp/x.html"), &ctx)
            .unwrap();
        let body_end = out.rfind("</body>").unwrap();
        let script_pos = out.find(SCRIPT_URL).unwrap();
        assert!(script_pos < body_end);
    }

    #[test]
    fn transform_handles_missing_head_gracefully() {
        // Some emitters produce <html><body> without an explicit <head>.
        let html = "<html><body><main>x</main></body></html>";
        let ctx = ctx_for(Path::new("/tmp/s"));
        let out = ViewTransitionsPlugin::new()
            .transform_html(html, Path::new("/tmp/x.html"), &ctx)
            .unwrap();
        // Script still injected even when style cannot be placed.
        assert!(out.contains(SCRIPT_URL));
    }

    #[test]
    fn after_compile_writes_script_file() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        let ctx = ctx_for(&site);
        ViewTransitionsPlugin::new().after_compile(&ctx).unwrap();

        let path = site.join(SCRIPT_DIR).join(SCRIPT_FILENAME);
        assert!(path.exists());
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("startViewTransition"));
    }

    #[test]
    fn after_compile_is_noop_when_site_missing() {
        let ctx = ctx_for(Path::new("/nonexistent/site/dir/xyz"));
        assert!(ViewTransitionsPlugin::new().after_compile(&ctx).is_ok());
    }

    #[test]
    fn after_compile_respects_dry_run() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        let ctx = ctx_for(&site).with_dry_run(true);
        ViewTransitionsPlugin::new().after_compile(&ctx).unwrap();

        assert!(!site.join(SCRIPT_DIR).exists());
    }

    #[test]
    fn enabled_reads_config_flag() {
        let mut cfg = SsgConfig::builder()
            .site_name("t".into())
            .base_url("http://example.com".into())
            .build()
            .unwrap();
        assert!(!ViewTransitionsPlugin::enabled(&cfg));
        cfg.transitions = true;
        assert!(ViewTransitionsPlugin::enabled(&cfg));
    }

    #[test]
    fn script_includes_history_push() {
        // Smooth-nav must update history so back/forward work.
        assert!(VIEW_TRANSITIONS_JS.contains("history.pushState"));
        assert!(VIEW_TRANSITIONS_JS.contains("popstate"));
    }

    #[test]
    fn script_includes_download_and_target_guards() {
        // Don't intercept download or target=_blank links.
        assert!(VIEW_TRANSITIONS_JS.contains("download"));
        assert!(VIEW_TRANSITIONS_JS.contains("_self"));
    }

    #[test]
    fn script_includes_opt_out_attribute() {
        // Explicit per-link opt-out via data-no-transition.
        assert!(VIEW_TRANSITIONS_JS.contains("noTransition"));
    }

    #[test]
    fn script_falls_back_on_fetch_failure() {
        // 5xx / network errors should still get the user to the page.
        assert!(VIEW_TRANSITIONS_JS.contains("location.href = url"));
    }

    #[test]
    fn transform_appends_script_when_html_close_but_no_body_close() {
        // `</html>` passes the fragment guard, but with no `</body>`
        // the script is appended at the end.
        let plugin = ViewTransitionsPlugin::new();
        let dir = tempdir().unwrap();
        let html = "<html><head></head>x</html>";
        let out = plugin
            .transform_html(html, Path::new("i.html"), &ctx_for(dir.path()))
            .unwrap();
        assert!(out.ends_with("</script>\n"));
    }

    #[test]
    fn after_compile_fails_when_script_dir_squatted_by_file() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join(SCRIPT_DIR), "not a dir").unwrap();
        let err = ViewTransitionsPlugin::new()
            .after_compile(&ctx_for(&site))
            .unwrap_err();
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn after_compile_fails_when_script_file_squatted_by_dir() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(site.join(SCRIPT_DIR).join(SCRIPT_FILENAME))
            .unwrap();
        let err = ViewTransitionsPlugin::new()
            .after_compile(&ctx_for(&site))
            .unwrap_err();
        assert!(!format!("{err}").is_empty());
    }
}
