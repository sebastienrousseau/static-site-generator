// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Live-reload script injection plugin.
//!
//! Injects a WebSocket-based live-reload client into all HTML files in
//! the site directory when the development server starts.
//!
//! # How it works
//!
//! 1. The `LiveReloadPlugin` hooks into the `on_serve` lifecycle event.
//! 2. It walks all HTML files in the site directory.
//! 3. It injects a `<script>` tag before `</body>` that opens a WebSocket
//!    connection to a configurable port (default 35729).
//! 4. When the server sends a `"reload"` message, the page reloads.
//! 5. On disconnect, the script auto-reconnects with exponential backoff
//!    (1s, 2s, 4s, capped at 10s) and shows a small "Connecting..."
//!    indicator in the bottom-right corner.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use std::fs;
use std::path::{Path, PathBuf};

/// Default WebSocket port for the live-reload server.
pub const DEFAULT_PORT: u16 = 35729;

/// Maximum number of HTML files to process.
const MAX_FILES: usize = 50_000;

/// Marker attribute used to detect whether the script has already been injected.
const MARKER: &str = "ssg-livereload";

/// Plugin that injects a live-reload script into all HTML files.
///
/// The injected script opens a WebSocket connection and reloads the page
/// when it receives a `"reload"` message. It reconnects automatically
/// with exponential backoff on disconnect.
///
/// # Example
///
/// ```rust
/// use ssg::plugin::PluginManager;
/// use ssg::livereload::LiveReloadPlugin;
///
/// let mut pm = PluginManager::new();
/// pm.register(LiveReloadPlugin::new());
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LiveReloadPlugin {
    /// WebSocket port the live-reload client connects to.
    port: u16,
}

impl LiveReloadPlugin {
    /// Creates a new `LiveReloadPlugin` with the default port (35729).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::livereload::{LiveReloadPlugin, DEFAULT_PORT};
    ///
    /// let p = LiveReloadPlugin::new();
    /// assert_eq!(p.port(), DEFAULT_PORT);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self { port: DEFAULT_PORT }
    }

    /// Creates a new `LiveReloadPlugin` with a custom WebSocket port.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::livereload::LiveReloadPlugin;
    ///
    /// let p = LiveReloadPlugin::with_port(8765);
    /// assert_eq!(p.port(), 8765);
    /// ```
    #[must_use]
    pub const fn with_port(port: u16) -> Self {
        Self { port }
    }

    /// Returns the configured port.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::livereload::LiveReloadPlugin;
    ///
    /// let p = LiveReloadPlugin::with_port(8080);
    /// assert_eq!(p.port(), 8080);
    /// ```
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }
}

impl Default for LiveReloadPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for LiveReloadPlugin {
    fn name(&self) -> &'static str {
        "livereload"
    }

    fn on_serve(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        let html_files = collect_html_files(&ctx.site_dir)?;
        if html_files.is_empty() {
            return Ok(());
        }

        for path in &html_files {
            inject_livereload(path, self.port)?;
        }

        println!(
            "[livereload] Injected live-reload script into {} HTML file(s) (port {})",
            html_files.len(),
            self.port,
        );
        Ok(())
    }
}

/// Collect all `.html` files under `dir` (iterative, bounded).
fn collect_html_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files_bounded_count(dir, "html", MAX_FILES)
}

/// Inject the live-reload script into a single HTML file.
///
/// Inserts a `<script>` block before `</body>`. The script:
/// 1. Opens a WebSocket to `ws://localhost:{port}`
/// 2. Reloads on receiving a `"reload"` message
/// 3. Reconnects with exponential backoff (1s, 2s, 4s, max 10s)
/// 4. Shows a "Connecting..." indicator during reconnection
///
/// The injection is idempotent — if the marker is already present,
/// the file is left unchanged.
fn inject_livereload(path: &Path, port: u16) -> Result<(), SsgError> {
    let html = fs::read_to_string(path).with_path(path)?;

    if html.contains(MARKER) {
        return Ok(()); // Already injected
    }

    let script = livereload_script(port);

    let injected = if let Some(pos) = html.rfind("</body>") {
        format!("{}{}{}", &html[..pos], script, &html[pos..])
    } else {
        format!("{html}{script}")
    };

    fs::write(path, injected).with_path(path)?;
    Ok(())
}

/// Generate the live-reload script tag for a given port.
fn livereload_script(port: u16) -> String {
    format!(
        r"
<!-- SSG Live-Reload -->
<script data-ssg-livereload>
(function(){{
  var url='ws://localhost:{port}',delay=1000,maxDelay=10000,indicator=null;
  try{{var sp=sessionStorage.getItem('ssg-scroll');if(sp){{sessionStorage.removeItem('ssg-scroll');var p=JSON.parse(sp);setTimeout(function(){{scrollTo(p.x,p.y);}},50);}}}}catch(se){{}}
  function showIndicator(){{
    if(indicator)return;
    indicator=document.createElement('div');
    indicator.id='ssg-livereload';
    indicator.textContent='Connecting\u2026';
    indicator.style.cssText='position:fixed;bottom:8px;right:8px;z-index:99999;'
      +'background:rgba(0,0,0,0.75);color:#fff;padding:6px 12px;border-radius:6px;'
      +'font:13px/1 -apple-system,system-ui,sans-serif;pointer-events:none';
    document.body.appendChild(indicator);
  }}
  function hideIndicator(){{
    if(indicator){{indicator.remove();indicator=null;}}
  }}
  function showOverlay(msg){{
    hideOverlay();
    var d=document.createElement('div');
    d.id='ssg-error-overlay';
    d.style.cssText='position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,0.85);color:#fff;font-family:monospace;font-size:14px;z-index:999999;padding:32px;overflow:auto;';
    var c=document.createElement('div');
    c.style.cssText='max-width:800px;margin:0 auto;';
    var hdr=document.createElement('div');
    hdr.style.cssText='display:flex;justify-content:space-between;align-items:center;margin-bottom:16px;';
    var title=document.createElement('span');
    title.style.cssText='color:#ff6b6b;font-size:18px;font-weight:bold;';
    title.textContent='Build Error';
    var btn=document.createElement('button');
    btn.textContent='\u2715';
    btn.style.cssText='background:none;border:1px solid #666;color:#fff;padding:4px 12px;cursor:pointer;border-radius:4px;';
    btn.addEventListener('click',hideOverlay);
    hdr.appendChild(title);
    hdr.appendChild(btn);
    c.appendChild(hdr);
    if(msg.file){{
      var fp=document.createElement('div');
      fp.style.cssText='color:#ffd93d;margin-bottom:8px;';
      fp.textContent=msg.file+(msg.line?':'+msg.line:'');
      c.appendChild(fp);
    }}
    var pre=document.createElement('pre');
    pre.style.cssText='background:#1a1a2e;padding:16px;border-radius:8px;border-left:4px solid #ff6b6b;overflow-x:auto;white-space:pre-wrap;word-break:break-word;';
    pre.textContent=msg.message;
    c.appendChild(pre);
    d.appendChild(c);
    document.body.appendChild(d);
  }}
  function hideOverlay(){{var e=document.getElementById('ssg-error-overlay');if(e)e.remove();}}
  function connect(){{
    try{{
      var ws=new WebSocket(url);
      ws.onopen=function(){{delay=1000;hideIndicator();}};
      ws.onmessage=function(e){{
        if(e.data==='reload'){{hideOverlay();try{{sessionStorage.setItem('ssg-scroll',JSON.stringify({{x:scrollX,y:scrollY}}));}}catch(se){{}}
          var doReload=function(){{location.reload();}};
          // Issue #547: when the transitions client is active, wrap the
          // reload in a view transition so structural HMR frames cross-fade.
          if(typeof window.__ssgTransitionsReload==='function'){{
            try{{window.__ssgTransitionsReload(doReload);}}catch(_){{doReload();}}
          }}else{{doReload();}}
        }}
        try{{var msg=JSON.parse(e.data);
        if(msg.type==='error'){{showOverlay(msg);}}
        else if(msg.type==='clear-error'){{hideOverlay();}}
        else if(msg.type==='css-reload'||msg.type==='hmr-css'){{
          var links=document.querySelectorAll('link[rel=stylesheet]');
          links.forEach(function(link){{
            var href=link.getAttribute('href');
            if(href){{link.setAttribute('href',href.split('?')[0]+'?v='+Date.now());}}
          }});
        }}
        else if(msg.type==='hmr-html'){{
          // Partial HTML reload (AC3 / AC4) — fetch the current page
          // fresh and swap the <main> body so scroll position and
          // any form state outside <main> are preserved. If the URL
          // is not in the broadcast paths list we still do nothing
          // (other tabs handle their own pages). If <head> needs an
          // update the server sends 'reload' instead.
          var here=location.pathname;
          var hit=Array.isArray(msg.paths)&&msg.paths.some(function(p){{
            return p===here||p===here.replace(/\/$/,'')||p+'/'===here;
          }});
          if(!hit){{return;}}
          hideOverlay();
          fetch(location.href,{{cache:'no-store'}}).then(function(r){{
            return r.text();
          }}).then(function(html){{
            var parser=new DOMParser();
            var doc=parser.parseFromString(html,'text/html');
            var freshMain=doc.querySelector('main');
            var liveMain=document.querySelector('main');
            if(freshMain&&liveMain){{
              liveMain.replaceWith(freshMain);
            }}else if(doc.body&&document.body){{
              document.body.innerHTML=doc.body.innerHTML;
            }}else{{
              location.reload();
            }}
          }}).catch(function(){{location.reload();}});
        }}
        else if(msg.type==='reload'){{
          hideOverlay();
          try{{sessionStorage.setItem('ssg-scroll',JSON.stringify({{x:scrollX,y:scrollY}}));}}catch(se){{}}
          location.reload();
        }}
        }}catch(x){{}}
      }};
      ws.onclose=function(){{
        var d=delay;
        delay=Math.min(delay*2,maxDelay);
        setTimeout(connect,d);
      }};
      ws.onerror=function(){{}};
    }}catch(e){{}}
  }}
  // Only connect in development (localhost) and limit retries
  // to avoid console error spam when the WS server is not running
  if(location.hostname==='localhost'||location.hostname==='127.0.0.1'||location.hostname==='0.0.0.0'){{
    if(document.readyState==='loading'){{
      document.addEventListener('DOMContentLoaded',connect);
    }}else{{
      connect();
    }}
  }}
}})();
</script>
"
    )
}

/// Returns a WebSocket message for CSS-only reload.
#[must_use]
#[allow(dead_code)]
pub fn css_reload_message(css_path: &str) -> String {
    serde_json::json!({
        "type": "css-reload",
        "file": css_path,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_html(body: &str) -> String {
        format!(
            "<html><head><title>Test</title></head>\
             <body>{body}</body></html>"
        )
    }

    #[test]
    fn inject_adds_script() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("page.html");
        fs::write(&path, make_html("<p>Hello</p>")).expect("write");

        inject_livereload(&path, DEFAULT_PORT).expect("inject");

        let result = fs::read_to_string(&path).expect("read");
        assert!(result.contains(MARKER));
        assert!(result.contains("WebSocket"));
        assert!(result.contains("35729"));
        assert!(result.contains("location.reload()"));
    }

    #[test]
    fn inject_before_closing_body() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("page.html");
        fs::write(&path, make_html("<p>Hi</p>")).expect("write");

        inject_livereload(&path, DEFAULT_PORT).expect("inject");

        let result = fs::read_to_string(&path).expect("read");
        let script_pos = result.find(MARKER).unwrap();
        let body_pos = result.rfind("</body>").unwrap();
        assert!(script_pos < body_pos);
    }

    #[test]
    fn inject_idempotent() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("page.html");
        fs::write(&path, make_html("<p>Hi</p>")).expect("write");

        inject_livereload(&path, DEFAULT_PORT).expect("inject");
        let first = fs::read_to_string(&path).expect("read");

        inject_livereload(&path, DEFAULT_PORT).expect("inject");
        let second = fs::read_to_string(&path).expect("read");

        assert_eq!(first, second);
    }

    #[test]
    fn inject_custom_port() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("page.html");
        fs::write(&path, make_html("<p>Hi</p>")).expect("write");

        inject_livereload(&path, 9999).expect("inject");

        let result = fs::read_to_string(&path).expect("read");
        assert!(result.contains("9999"));
        assert!(!result.contains("35729"));
    }

    #[test]
    fn inject_no_body_tag() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("page.html");
        fs::write(&path, "<html><p>No body tag</p></html>").expect("write");

        inject_livereload(&path, DEFAULT_PORT).expect("inject");

        let result = fs::read_to_string(&path).expect("read");
        assert!(result.contains(MARKER));
    }

    #[test]
    fn skip_non_html_files() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("style.css"), "body{}").expect("write");
        fs::write(tmp.path().join("data.json"), "{}").expect("write");
        fs::write(tmp.path().join("readme.txt"), "hello").expect("write");

        let files = collect_html_files(tmp.path()).expect("collect");
        assert!(files.is_empty(), "{:?}", files);
    }

    #[test]
    fn empty_directory() {
        let tmp = tempdir().expect("tempdir");
        let files = collect_html_files(tmp.path()).expect("collect");
        assert!(files.is_empty(), "{:?}", files);
    }

    #[test]
    fn nonexistent_directory() {
        let ctx = PluginContext::new(
            Path::new("c"),
            Path::new("b"),
            Path::new("/nonexistent_dir_ssg_test"),
            Path::new("t"),
        );
        let plugin = LiveReloadPlugin::new();
        assert!(plugin.on_serve(&ctx).is_ok());
    }

    #[test]
    fn plugin_name() {
        assert_eq!(LiveReloadPlugin::new().name(), "livereload");
    }

    #[test]
    fn plugin_registration() {
        use crate::plugin::PluginManager;
        let mut pm = PluginManager::new();
        pm.register(LiveReloadPlugin::new());
        assert_eq!(pm.names(), vec!["livereload"]);
    }

    #[test]
    fn with_port_constructor() {
        let plugin = LiveReloadPlugin::with_port(8080);
        assert_eq!(plugin.port(), 8080);
    }

    #[test]
    fn default_port_value() {
        let plugin = LiveReloadPlugin::new();
        assert_eq!(plugin.port(), 35729);
    }

    #[test]
    fn default_trait_impl() {
        let plugin = LiveReloadPlugin::default();
        assert_eq!(plugin.port(), DEFAULT_PORT);
    }

    #[test]
    fn on_serve_injects_all_html_files() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("index.html"), make_html("<p>Home</p>"))
            .expect("write index");
        fs::write(tmp.path().join("about.html"), make_html("<p>About</p>"))
            .expect("write about");
        fs::write(tmp.path().join("style.css"), "body{}").expect("write css");

        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );
        LiveReloadPlugin::new().on_serve(&ctx).expect("on_serve");

        let index = fs::read_to_string(tmp.path().join("index.html"))
            .expect("read index");
        let about = fs::read_to_string(tmp.path().join("about.html"))
            .expect("read about");
        let css =
            fs::read_to_string(tmp.path().join("style.css")).expect("read css");

        assert!(index.contains(MARKER));
        assert!(about.contains(MARKER));
        assert!(!css.contains(MARKER));
    }

    #[test]
    fn script_contains_reconnect_backoff() {
        let script = livereload_script(DEFAULT_PORT);
        assert!(script.contains("delay*2"));
        assert!(script.contains("maxDelay"));
        assert!(script.contains("10000"));
    }

    #[test]
    fn script_contains_connecting_indicator() {
        let script = livereload_script(DEFAULT_PORT);
        assert!(script.contains("Connecting"));
        assert!(script.contains("showIndicator"));
        assert!(script.contains("hideIndicator"));
        assert!(script.contains("bottom"));
        assert!(script.contains("right"));
    }

    #[test]
    fn livereload_custom_port() {
        // Arrange
        let port: u16 = 44444;

        // Act
        let script = livereload_script(port);

        // Assert — custom port appears, default does not
        assert!(script.contains("44444"));
        assert!(!script.contains("35729"));
    }

    #[test]
    fn livereload_plugin_no_html_files() {
        // Arrange
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("style.css"), "body{}").expect("write");
        fs::write(tmp.path().join("data.json"), "{}").expect("write");

        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );

        // Act
        let result = LiveReloadPlugin::new().on_serve(&ctx);

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn livereload_plugin_idempotent() {
        // Arrange
        let tmp = tempdir().expect("tempdir");
        let html_path = tmp.path().join("page.html");
        fs::write(&html_path, make_html("<p>Hello</p>")).expect("write");

        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );

        // Act — run the full plugin twice
        LiveReloadPlugin::new()
            .on_serve(&ctx)
            .expect("first on_serve");
        let after_first = fs::read_to_string(&html_path).expect("read");

        LiveReloadPlugin::new()
            .on_serve(&ctx)
            .expect("second on_serve");
        let after_second = fs::read_to_string(&html_path).expect("read");

        // Assert — content identical, no double injection
        assert_eq!(after_first, after_second);
        // The marker string appears in both the data attribute and the
        // indicator id within a single injection, so count the script tags.
        let script_count = after_second.matches("data-ssg-livereload").count();
        assert_eq!(script_count, 1, "script tag should appear exactly once");
    }

    #[test]
    fn livereload_script_contains_reconnect_logic() {
        // Arrange & Act
        let script = livereload_script(DEFAULT_PORT);

        // Assert — script has exponential backoff reconnection
        assert!(script.contains("delay*2"), "should double the delay");
        assert!(script.contains("maxDelay"), "should cap the delay");
        assert!(script.contains("setTimeout"), "should schedule reconnect");
        assert!(script.contains("connect"), "should call connect again");
    }

    #[test]
    fn livereload_plugin_nonexistent_dir() {
        // Arrange
        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            Path::new("/absolutely/nonexistent/directory/for/test"),
            Path::new("templates"),
        );

        // Act
        let result = LiveReloadPlugin::new().on_serve(&ctx);

        // Assert — returns Ok, does not error on missing directory
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn on_serve_propagates_inject_read_failure() {
        use std::os::unix::fs::PermissionsExt;

        // An unreadable HTML file makes inject_livereload fail inside
        // the on_serve loop, exercising its `?` propagation.
        let tmp = tempdir().expect("tempdir");
        let locked = tmp.path().join("locked.html");
        fs::write(&locked, make_html("<p>Hi</p>")).expect("write");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .expect("chmod");

        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );
        let res = LiveReloadPlugin::new().on_serve(&ctx);

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o644));
        assert!(res.is_err(), "unreadable HTML file must fail on_serve");
    }

    #[test]
    #[cfg(unix)]
    fn inject_fails_on_readonly_file() {
        use std::os::unix::fs::PermissionsExt;

        // Readable but not writable: the read succeeds, the marker is
        // absent, and the final fs::write errors — exercising the write
        // `?` branch of inject_livereload.
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("frozen.html");
        fs::write(&path, make_html("<p>Hi</p>")).expect("write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444))
            .expect("chmod");

        let res = inject_livereload(&path, DEFAULT_PORT);

        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
        assert!(res.is_err(), "read-only HTML file must fail the write");
    }

    #[test]
    fn test_script_contains_error_overlay() {
        let script = livereload_script(DEFAULT_PORT);
        assert!(
            script.contains("showOverlay"),
            "script must contain showOverlay function"
        );
        assert!(
            script.contains("hideOverlay"),
            "script must contain hideOverlay function"
        );
        assert!(
            script.contains("ssg-error-overlay"),
            "script must contain overlay element id"
        );
    }

    #[test]
    fn test_script_backward_compat() {
        let script = livereload_script(DEFAULT_PORT);
        assert!(
            script.contains("'reload'"),
            "script must still handle plain 'reload' messages"
        );
    }

    #[test]
    fn test_script_contains_css_reload() {
        let script = livereload_script(DEFAULT_PORT);
        assert!(
            script.contains("css-reload"),
            "script must contain css-reload handler"
        );
    }

    #[test]
    fn test_script_coordinates_with_view_transitions() {
        // AC7: when the transitions client is loaded (issue #547),
        // the reload handler must defer to its wrapper so structural
        // HMR frames go through startViewTransition().
        let script = livereload_script(DEFAULT_PORT);
        assert!(
            script.contains("__ssgTransitionsReload"),
            "reload path must consult window.__ssgTransitionsReload"
        );
        assert!(
            script.contains("location.reload()"),
            "reload path must still call location.reload() as the fallback"
        );
    }

    #[test]
    fn test_script_contains_scroll_preservation() {
        let script = livereload_script(DEFAULT_PORT);
        assert!(
            script.contains("ssg-scroll"),
            "script must contain scroll preservation key"
        );
        assert!(
            script.contains("sessionStorage"),
            "script must use sessionStorage for scroll"
        );
    }

    #[test]
    fn test_css_reload_message() {
        let msg = css_reload_message("styles/main.css");
        let parsed: serde_json::Value =
            serde_json::from_str(&msg).expect("valid JSON");
        assert_eq!(parsed["type"], "css-reload");
        assert_eq!(parsed["file"], "styles/main.css");
    }

    #[test]
    fn test_inject_fails_on_nonexistent_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("nonexistent.html");
        let res = inject_livereload(&path, DEFAULT_PORT);
        assert!(res.is_err());
    }

    #[test]
    fn test_inject_fails_on_directory() {
        let tmp = tempdir().unwrap();
        let res = inject_livereload(tmp.path(), DEFAULT_PORT);
        assert!(res.is_err());
    }

    #[test]
    #[cfg(unix)]
    fn test_on_serve_collect_html_files_error() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let unreadable = tmp.path().join("unreadable");
        fs::create_dir(&unreadable).unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000))
            .unwrap();

        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            &unreadable,
            Path::new("templates"),
        );
        let res = LiveReloadPlugin::new().on_serve(&ctx);

        let _ =
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755));
        assert!(res.is_err());
    }
}
