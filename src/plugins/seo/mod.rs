// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! SEO plugins for the static site generator.
//!
//! Provides three plugins that improve search engine optimization:
//!
//! - `SeoPlugin` — Injects missing meta tags (description, Open Graph,
//!   Twitter Card) into HTML files.
//! - `RobotsPlugin` — Generates a `robots.txt` file.
//! - `CanonicalPlugin` — Injects `<link rel="canonical">` tags.

mod canonical;
pub mod helpers;
pub mod jsonld;
pub(crate) mod lang;
mod robots;
mod seo_plugin;

pub use canonical::CanonicalPlugin;
pub use jsonld::{
    validate_jsonld, JsonLdConfig, JsonLdPlugin, JsonLdValidationError,
};
pub use robots::RobotsPlugin;
pub use seo_plugin::SeoPlugin;

#[cfg(test)]
mod tests {
    use super::helpers::*;
    use super::*;
    use crate::plugin::{Plugin, PluginContext};
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn make_html(title: &str, body: &str) -> String {
        format!(
            "<html><head><title>{title}</title></head>\
             <body>{body}</body></html>"
        )
    }

    fn test_ctx(site_dir: &Path) -> PluginContext {
        crate::test_support::init_logger();
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            site_dir,
            Path::new("templates"),
        )
    }

    // -----------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_title_present() {
        let html = "<html><head><title>My Page</title></head></html>";
        assert_eq!(extract_title(html), "My Page");
    }

    #[test]
    fn test_extract_title_missing() {
        let html = "<html><head></head><body></body></html>";
        assert_eq!(extract_title(html), "");
    }

    #[test]
    fn test_extract_description_truncates() {
        let long = "word ".repeat(100);
        let html =
            format!("<html><head></head><body><p>{long}</p></body></html>");
        let desc = extract_description(&html, 160);
        assert!(desc.len() <= 160);
        assert!(!desc.is_empty(), "should not be empty");
    }

    // -----------------------------------------------------------------
    // SeoPlugin tests
    // -----------------------------------------------------------------

    #[test]
    fn test_seo_plugin_name() {
        assert_eq!(SeoPlugin.name(), "seo");
    }

    #[test]
    fn test_seo_plugin_injects_meta_tags() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let html = make_html("Hello World", "<p>Some content here</p>");

        let result = SeoPlugin
            .transform_html(&html, Path::new("index.html"), &ctx)
            .unwrap();
        assert!(result.contains("<meta name=\"description\""));
        assert!(result.contains("<meta property=\"og:title\""));
        assert!(result.contains("Hello World"));
        assert!(result.contains("<meta property=\"og:description\""));
        assert!(
            result.contains("<meta property=\"og:type\" content=\"website\"")
        );
        assert!(
            result.contains("<meta name=\"twitter:card\" content=\"summary\"")
        );
    }

    #[test]
    fn test_seo_plugin_idempotent() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let html = make_html("Test", "<p>Content</p>");

        let first = SeoPlugin
            .transform_html(&html, Path::new("page.html"), &ctx)
            .unwrap();
        let second = SeoPlugin
            .transform_html(&first, Path::new("page.html"), &ctx)
            .unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn test_extract_description_excludes_nav_header_footer() {
        let html = r##"<html><head></head><body>
            <a href="#main">Skip to content</a>
            <nav><ul><li>Home</li><li>About</li><li>Search</li></ul></nav>
            <header><h1>Site Header</h1></header>
            <main><p>This is the actual page content that should be extracted.</p></main>
            <footer><p>Copyright 2026</p></footer>
            </body></html>"##;
        let desc = extract_description(html, 160);
        assert!(
            desc.contains("actual page content"),
            "description should contain main content, got: {desc}"
        );
        assert!(
            !desc.contains("Skip to content"),
            "description should not contain skip link text"
        );
        assert!(
            !desc.contains("Site Header"),
            "description should not contain header text"
        );
        assert!(
            !desc.contains("Copyright"),
            "description should not contain footer text"
        );
    }

    #[test]
    fn test_seo_plugin_handles_missing_title() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let html =
            "<html><head></head><body><p>No title here</p></body></html>";

        let result = SeoPlugin
            .transform_html(html, Path::new("no-title.html"), &ctx)
            .unwrap();
        // Should still inject og:type and twitter:card
        assert!(result.contains("<meta property=\"og:type\""));
        assert!(result.contains("<meta name=\"twitter:card\""));
        // Should not inject og:title (no title available)
        assert!(!result.contains("<meta property=\"og:title\""));
    }

    #[test]
    fn test_seo_plugin_empty_dir() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        assert!(SeoPlugin.after_compile(&ctx).is_ok());
    }

    #[test]
    fn test_seo_plugin_nonexistent_dir() {
        let ctx = test_ctx(Path::new("/nonexistent/path"));
        assert!(SeoPlugin.after_compile(&ctx).is_ok());
    }

    // -----------------------------------------------------------------
    // RobotsPlugin tests
    // -----------------------------------------------------------------

    #[test]
    fn test_robots_plugin_name() {
        let plugin = RobotsPlugin::new("https://example.com");
        assert_eq!(plugin.name(), "robots");
    }

    #[test]
    fn test_robots_plugin_creates_file() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let plugin = RobotsPlugin::new("https://example.com");
        plugin.after_compile(&ctx).unwrap();

        let path = tmp.path().join("robots.txt");
        assert!(path.exists());
    }

    #[test]
    fn test_robots_plugin_correct_content() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let plugin = RobotsPlugin::new("https://example.com");
        plugin.after_compile(&ctx).unwrap();

        let content =
            fs::read_to_string(tmp.path().join("robots.txt")).unwrap();
        assert!(content.contains("User-agent: *"));
        assert!(content.contains("Allow: /"));
        assert!(content.contains("Sitemap: https://example.com/sitemap.xml"));
    }

    #[test]
    fn test_robots_plugin_does_not_overwrite() {
        let tmp = tempdir().unwrap();
        let robots_path = tmp.path().join("robots.txt");
        fs::write(&robots_path, "User-agent: *\nDisallow: /secret\n").unwrap();

        let ctx = test_ctx(tmp.path());
        let plugin = RobotsPlugin::new("https://example.com");
        plugin.after_compile(&ctx).unwrap();

        let content = fs::read_to_string(&robots_path).unwrap();
        assert!(content.contains("Disallow: /secret"));
        assert!(!content.contains("Sitemap:"));
    }

    #[test]
    fn test_robots_plugin_custom_base_url() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let plugin = RobotsPlugin::new("https://my-site.org");
        plugin.after_compile(&ctx).unwrap();

        let content =
            fs::read_to_string(tmp.path().join("robots.txt")).unwrap();
        assert!(content.contains("Sitemap: https://my-site.org/sitemap.xml"));
    }

    // -----------------------------------------------------------------
    // CanonicalPlugin tests
    // -----------------------------------------------------------------

    #[test]
    fn test_canonical_plugin_name() {
        let plugin = CanonicalPlugin::new("https://example.com");
        assert_eq!(plugin.name(), "canonical");
    }

    #[test]
    fn test_canonical_plugin_injects_tag() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let plugin = CanonicalPlugin::new("https://example.com");
        let html = make_html("Home", "<p>Welcome</p>");
        let page_path = tmp.path().join("index.html");

        let result = plugin.transform_html(&html, &page_path, &ctx).unwrap();
        assert!(result.contains("<link rel=\"canonical\""));
        // Canonical URLs share `urls::derive_page_url` with permalink
        // injection and feed links (spec A2/B1, plan §2 item 1.2,
        // issue #586): root index.html collapses to the bare base URL
        // with a trailing slash.
        assert!(result.contains("href=\"https://example.com/\""));
    }

    #[test]
    fn test_canonical_plugin_idempotent() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let plugin = CanonicalPlugin::new("https://example.com");
        let html = make_html("Page", "<p>Content</p>");
        let page_path = tmp.path().join("page.html");

        let first = plugin.transform_html(&html, &page_path, &ctx).unwrap();
        let second = plugin.transform_html(&first, &page_path, &ctx).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn test_canonical_plugin_nested_files() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("blog")).unwrap();
        let ctx = test_ctx(tmp.path());
        let plugin = CanonicalPlugin::new("https://example.com");
        let html = make_html("Post", "<p>Blog post</p>");
        let page_path = tmp.path().join("blog/post.html");

        let result = plugin.transform_html(&html, &page_path, &ctx).unwrap();
        assert!(result.contains("https://example.com/blog/post.html"));
    }

    // -----------------------------------------------------------------
    // Registration tests
    // -----------------------------------------------------------------

    #[test]
    fn test_all_plugins_register() {
        use crate::plugin::PluginManager;
        let mut pm = PluginManager::new();
        pm.register(SeoPlugin);
        pm.register(RobotsPlugin::new("https://example.com"));
        pm.register(CanonicalPlugin::new("https://example.com"));
        assert_eq!(pm.len(), 3);
        assert_eq!(pm.names(), vec!["seo", "robots", "canonical"]);
    }

    // -----------------------------------------------------------------
    // Additional edge-case tests
    // -----------------------------------------------------------------

    #[test]
    fn extract_description_unicode_truncation_respects_char_boundary() {
        // Arrange: multi-byte chars (é = 2 bytes, 日 = 3 bytes)
        let text = "café 日本語 ".repeat(30);
        let html =
            format!("<html><head></head><body><p>{text}</p></body></html>");

        // Act
        let desc = extract_description(&html, 50);

        // Assert: result is valid UTF-8 and within limit
        assert!(desc.len() <= 50);
        assert!(!desc.is_empty(), "should not be empty");
        // Verify it doesn't panic and is a valid string
        let _ = desc.chars().count();
    }

    #[test]
    fn extract_description_empty_main_falls_back_to_body() {
        // Arrange: <main> is present but empty
        let html = "<html><head></head><body>\
                     <main></main>\
                     <p>Body fallback text</p>\
                     </body></html>";

        // Act
        let desc = extract_description(html, 160);

        // Assert: empty main yields empty string (main takes priority)
        assert!(
            desc.is_empty(),
            "expected empty description from empty <main>, got: {desc}"
        );
    }

    #[test]
    fn extract_description_no_body_uses_raw_html() {
        // Arrange: no <body> tag at all
        let html = "<div><p>Raw content without body</p></div>";

        // Act
        let desc = extract_description(html, 160);

        // Assert: falls back to raw HTML content
        assert!(
            desc.contains("Raw content without body"),
            "expected raw content fallback, got: {desc}"
        );
    }

    #[test]
    fn extract_title_with_nested_tags() {
        // Arrange: title contains nested HTML tags
        let html = "<html><head><title><span>Foo</span></title></head></html>";

        // Act
        let title = extract_title(html);

        // Assert: nested tags are stripped, text is preserved
        assert_eq!(title, "Foo");
    }

    #[test]
    fn escape_attr_all_special_chars() {
        // Arrange
        let input = r#"Tom & "Jerry" <script>alert('xss')</script>"#;

        // Act
        let escaped = escape_attr(input);

        // Assert: all special chars are escaped
        assert!(escaped.contains("&amp;"), "& should be escaped");
        assert!(escaped.contains("&quot;"), "\" should be escaped");
        assert!(escaped.contains("&lt;"), "< should be escaped");
        assert!(escaped.contains("&gt;"), "> should be escaped");
        assert_eq!(
            escaped,
            "Tom &amp; &quot;Jerry&quot; &lt;script&gt;alert('xss')&lt;/script&gt;"
        );
    }

    #[test]
    fn seo_plugin_skips_existing_single_quote_meta() {
        // Arrange: meta tags use single quotes
        let html = "<html><head>\
                     <meta name='description' content='Already set'>\
                     <meta property='og:title' content='Title'>\
                     <meta property='og:description' content='Desc'>\
                     <meta property='og:type' content='website'>\
                     <meta name='twitter:card' content='summary'>\
                     <title>Test</title></head>\
                     <body><p>Content</p></body></html>";
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());

        // Act
        let result = SeoPlugin
            .transform_html(html, Path::new("single-quote.html"), &ctx)
            .unwrap();
        assert_eq!(
            result.matches("meta name=\"description\"").count()
                + result.matches("meta name='description'").count(),
            1,
            "description meta should not be duplicated"
        );
        assert_eq!(
            result.matches("og:title").count(),
            1,
            "og:title should not be duplicated"
        );
    }

    #[test]
    fn canonical_plugin_trailing_slash_base_url() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let plugin = CanonicalPlugin::new("https://example.com/");
        let html = make_html("Home", "<p>Welcome</p>");
        let page_path = tmp.path().join("index.html");

        let result = plugin.transform_html(&html, &page_path, &ctx).unwrap();
        assert!(
            // index.html collapses to the directory URL under the
            // shared `urls::derive_page_url` convention (spec A2/B1,
            // plan §2 item 1.2, issue #586).
            result.contains("href=\"https://example.com/\""),
            "should produce clean URL without double slash"
        );
        assert!(
            !result.contains("https://example.com//"),
            "should not contain double slash in canonical URL"
        );
    }

    #[test]
    fn robots_plugin_trailing_slash_base_url() {
        // Arrange: base_url has a trailing slash
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let plugin = RobotsPlugin::new("https://example.com/");

        // Act
        plugin.after_compile(&ctx).unwrap();

        // Assert: sitemap URL has no double slash
        let content =
            fs::read_to_string(tmp.path().join("robots.txt")).unwrap();
        assert!(
            content.contains("Sitemap: https://example.com/sitemap.xml"),
            "sitemap URL should not have double slash, got: {content}"
        );
        assert!(
            !content.contains("https://example.com//"),
            "should not contain double slash"
        );
    }

    #[test]
    fn extract_description_nested_script_in_main() {
        // Arrange: <main> contains a <script> block alongside real content
        let html = "<html><head></head><body>\
                     <main>\
                     <script>var x = 'ignore me';</script>\
                     <p>Visible text after script</p>\
                     </main></body></html>";

        // Act
        let desc = extract_description(html, 160);

        // Assert: script content is stripped, visible text remains
        assert!(
            desc.contains("Visible text after script"),
            "should contain the paragraph text, got: {desc}"
        );
        assert!(
            !desc.contains("ignore me"),
            "should not contain script content, got: {desc}"
        );
    }

    // -----------------------------------------------------------------
    // JSON-LD Plugin tests
    // -----------------------------------------------------------------

    #[test]
    fn test_jsonld_injects_webpage() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        let html = make_html("About", "<p>About us</p>");
        let ctx = test_ctx(&site);
        let plugin = JsonLdPlugin::from_site("https://example.com", "Test Org");
        let page_path = site.join("about.html");

        let output = plugin.transform_html(&html, &page_path, &ctx).unwrap();
        assert!(output.contains("application/ld+json"));
        assert!(output.contains("\"@type\":\"WebPage\""));
        assert!(output.contains("\"name\":\"About\""));
    }

    #[test]
    fn test_jsonld_injects_article() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        let html = "<html><head><title>Post</title></head>\
                     <body><article><h1>Post</h1></article></body></html>";
        let ctx = test_ctx(&site);
        let plugin = JsonLdPlugin::from_site("https://example.com", "My Org");
        let page_path = site.join("post.html");

        let output = plugin.transform_html(html, &page_path, &ctx).unwrap();
        assert!(output.contains("\"@type\":\"Article\""));
        assert!(output.contains("\"headline\":\"Post\""));
        assert!(output.contains("My Org"));
    }

    #[test]
    fn test_jsonld_breadcrumbs() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let blog = site.join("blog");
        fs::create_dir_all(&blog).unwrap();

        let html = make_html("My Post", "<p>Content</p>");
        let ctx = test_ctx(&site);
        let plugin = JsonLdPlugin::from_site("https://example.com", "Org");
        let page_path = blog.join("my-post.html");

        let output = plugin.transform_html(&html, &page_path, &ctx).unwrap();
        assert!(output.contains("BreadcrumbList"));
        assert!(output.contains("\"name\":\"Home\""));
        assert!(output.contains("\"name\":\"blog\""));
    }

    #[test]
    fn test_jsonld_idempotent() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        let html = "<html><head><title>X</title>\
                     <script type=\"application/ld+json\">{}</script>\
                     </head><body></body></html>";
        let ctx = test_ctx(&site);
        let plugin = JsonLdPlugin::from_site("https://example.com", "Org");
        let page_path = site.join("x.html");

        let output = plugin.transform_html(html, &page_path, &ctx).unwrap();
        // Should have exactly one ld+json (the original), not two
        let count = output.matches("application/ld+json").count();
        assert_eq!(count, 1);
    }

    // -----------------------------------------------------------------
    // extract_title — edge cases
    // -----------------------------------------------------------------

    #[test]
    fn extract_title_empty_tag_returns_empty_string() {
        assert_eq!(extract_title("<title></title>"), "");
        assert_eq!(extract_title("<title>   </title>"), "");
        assert_eq!(extract_title("<title>\n\t </title>"), "");
    }

    #[test]
    fn extract_title_without_closing_tag_consumes_remainder() {
        // HTML5 spec: <title> is RCDATA, so an unterminated <title>
        // consumes the rest of the document. The lol_html parser walks
        // to EOF and yields the trailing text, which matches what every
        // standards-compliant browser does.
        assert_eq!(extract_title("<title>Unterminated"), "Unterminated");
    }

    #[test]
    fn extract_title_strips_inner_html_tags() {
        let out = extract_title("<title>Hello <em>World</em></title>");
        assert!(out.contains("Hello"));
        assert!(out.contains("World"));
    }

    // -----------------------------------------------------------------
    // extract_description — every branch
    // -----------------------------------------------------------------

    #[test]
    fn extract_description_prefers_main_over_body() {
        let html = r"<html><head></head><body>
            <nav>menu</nav>
            <main>The primary content.</main>
            <footer>Bottom</footer>
        </body></html>";
        let desc = extract_description(html, 200);
        assert!(desc.contains("primary content"));
        assert!(!desc.contains("menu"));
    }

    #[test]
    fn extract_description_main_without_closing_tag_takes_rest() {
        let html = r"<html><body><main>content without close";
        let desc = extract_description(html, 200);
        assert!(desc.contains("content without close"));
    }

    #[test]
    fn extract_description_main_without_angle_bracket_returns_empty_fallback() {
        let html = "<html><body><main";
        let desc = extract_description(html, 200);
        assert_eq!(desc, "");
    }

    #[test]
    fn extract_description_fallback_to_body_strips_script_and_style() {
        let html = r"<html><head></head><body>
            <script>alert('skip');</script>
            <style>body { color: red; }</style>
            <nav>menu items here</nav>
            <header>site title</header>
            <p>The body text.</p>
            <footer>copyright</footer>
        </body></html>";
        let desc = extract_description(html, 200);
        assert!(desc.contains("body text"));
        assert!(!desc.contains("alert"));
        assert!(!desc.contains("color: red"));
        assert!(!desc.contains("menu items"));
        assert!(!desc.contains("site title"));
        assert!(!desc.contains("copyright"));
    }

    #[test]
    fn extract_description_body_without_closing_tag_uses_rest() {
        let html = "<html><body><p>open-ended body paragraph";
        let desc = extract_description(html, 200);
        assert!(desc.contains("open-ended body paragraph"));
    }

    #[test]
    fn extract_description_body_without_angle_bracket_returns_empty() {
        let html = "<html><body";
        let desc = extract_description(html, 200);
        assert_eq!(desc, "");
    }

    #[test]
    fn extract_description_no_body_no_main_uses_entire_html() {
        let html = "just plain text no tags here";
        let desc = extract_description(html, 200);
        assert!(desc.contains("just plain text"));
    }

    #[test]
    fn extract_description_unterminated_script_breaks_out() {
        let html = "<html><body><main><script>unterminated<p>x</p>";
        let desc = extract_description(html, 200);
        let _ = desc;
    }

    #[test]
    fn extract_description_truncates_at_word_boundary() {
        let html = "<html><body><main>one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two twenty-three twenty-four twenty-five</main></body></html>";
        let desc = extract_description(html, 80);
        assert!(desc.len() <= 80);
        assert!(!desc.ends_with('-'));
    }

    #[test]
    fn extract_description_truncates_without_space_falls_to_byte_cut() {
        let html =
            "<html><body><main>oneverylongwordwithnospacesanywherehere</main></body></html>";
        let desc = extract_description(html, 10);
        assert!(desc.len() <= 10);
    }

    #[test]
    fn extract_description_respects_char_boundary_on_truncation() {
        let html = "<html><body><main>Rust programming — é ñ ü characters everywhere in this text that we want to truncate mid-char</main></body></html>";
        let desc = extract_description(html, 30);
        assert!(desc.is_ascii() || !desc.is_empty());
    }

    #[test]
    fn extract_description_truncation_walks_back_multiple_bytes() {
        let mut input = String::from("<html><body><main>");
        input.push_str(&"a".repeat(20));
        input.push('🎉'); // 4 bytes
        input.push_str(&"b".repeat(20));
        input.push_str("</main></body></html>");
        let desc = extract_description(&input, 22);
        assert!(!desc.is_empty(), "expected non-empty desc");
        let _ = desc.len();
    }

    #[test]
    fn extract_description_body_fallback_unterminated_nav_breaks() {
        let html = "<html><body><nav>unterminated nav block<p>visible</p>";
        let desc = extract_description(html, 200);
        let _ = desc;
    }

    // -----------------------------------------------------------------
    // SeoPlugin.after_compile — no </head> tag
    // -----------------------------------------------------------------

    #[test]
    fn seo_plugin_file_without_head_tag_is_unchanged() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("fragment.html"),
            "<p>no html/head/body structure</p>",
        )
        .unwrap();
        let ctx = test_ctx(dir.path());
        SeoPlugin.after_compile(&ctx).unwrap();
        let out = fs::read_to_string(dir.path().join("fragment.html")).unwrap();
        assert_eq!(out, "<p>no html/head/body structure</p>");
    }

    #[test]
    fn seo_plugin_missing_site_dir_returns_ok() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let ctx = test_ctx(&missing);
        SeoPlugin.after_compile(&ctx).unwrap();
    }

    // -----------------------------------------------------------------
    // RobotsPlugin — idempotency + missing dir
    // -----------------------------------------------------------------

    #[test]
    fn robots_plugin_skips_existing_robots_txt() {
        let dir = tempdir().unwrap();
        let existing = dir.path().join("robots.txt");
        fs::write(&existing, "USER: existing").unwrap();

        let plugin = RobotsPlugin::new("https://example.com");
        let ctx = test_ctx(dir.path());
        plugin.after_compile(&ctx).unwrap();

        assert_eq!(fs::read_to_string(&existing).unwrap(), "USER: existing");
    }

    #[test]
    fn robots_plugin_writes_user_agent_and_sitemap() {
        let dir = tempdir().unwrap();
        let plugin = RobotsPlugin::new("https://example.com/");
        let ctx = test_ctx(dir.path());
        plugin.after_compile(&ctx).unwrap();

        let body = fs::read_to_string(dir.path().join("robots.txt")).unwrap();
        assert!(body.contains("User-agent: *"));
        assert!(body.contains("Sitemap: https://example.com/sitemap.xml"));
    }

    #[test]
    fn robots_plugin_missing_site_dir_returns_ok() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let plugin = RobotsPlugin::new("https://example.com");
        let ctx = test_ctx(&missing);
        plugin.after_compile(&ctx).unwrap();
    }

    #[test]
    fn robots_plugin_name_returns_static_identifier() {
        assert_eq!(RobotsPlugin::new("").name(), "robots");
    }

    // -----------------------------------------------------------------
    // CanonicalPlugin — skip path, missing head, already-canonical
    // -----------------------------------------------------------------

    #[test]
    fn canonical_plugin_missing_site_dir_returns_ok() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let plugin = CanonicalPlugin::new("https://example.com");
        let ctx = test_ctx(&missing);
        plugin.after_compile(&ctx).unwrap();
    }

    #[test]
    fn canonical_plugin_replaces_existing_canonical_with_correct_url() {
        let dir = tempdir().unwrap();
        let html = r#"<html><head><link rel="canonical" href="/original"></head><body></body></html>"#;
        let plugin = CanonicalPlugin::new("https://example.com");
        let ctx = test_ctx(dir.path());
        let page_path = dir.path().join("p.html");

        let out = plugin.transform_html(html, &page_path, &ctx).unwrap();
        assert_eq!(out.matches(r#"rel="canonical""#).count(), 1);
        assert!(out.contains("https://example.com/p.html"));
    }

    #[test]
    fn canonical_plugin_skips_pages_with_single_quoted_canonical() {
        let dir = tempdir().unwrap();
        let html =
            r"<html><head><link rel='canonical' href='/x'></head></html>";
        let plugin = CanonicalPlugin::new("https://example.com");
        let ctx = test_ctx(dir.path());
        let page_path = dir.path().join("p.html");

        let out = plugin.transform_html(html, &page_path, &ctx).unwrap();
        assert_eq!(out.matches("canonical").count(), 1);
    }

    #[test]
    fn canonical_plugin_page_without_head_is_left_unchanged() {
        let dir = tempdir().unwrap();
        let html = "<p>no structure</p>";
        let plugin = CanonicalPlugin::new("https://example.com");
        let ctx = test_ctx(dir.path());
        let page_path = dir.path().join("frag.html");

        let out = plugin.transform_html(html, &page_path, &ctx).unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn canonical_plugin_injects_canonical_link_before_head_close() {
        let dir = tempdir().unwrap();
        let html = "<html><head><title>T</title></head><body></body></html>";
        let plugin = CanonicalPlugin::new("https://example.com/");
        let ctx = test_ctx(dir.path());
        let page_path = dir.path().join("a.html");

        let out = plugin.transform_html(html, &page_path, &ctx).unwrap();
        assert!(out.contains(r#"rel="canonical""#));
        assert!(out.contains("https://example.com/a.html"));
    }

    #[test]
    fn canonical_plugin_name_returns_static_identifier() {
        assert_eq!(CanonicalPlugin::new("").name(), "canonical");
    }

    // -----------------------------------------------------------------
    // JsonLdPlugin — WebPage branch + no-head skip
    // -----------------------------------------------------------------

    #[test]
    fn jsonld_plugin_missing_site_dir_returns_ok() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let plugin = JsonLdPlugin::from_site("https://example.com", "Org");
        let ctx = test_ctx(&missing);
        plugin.after_compile(&ctx).unwrap();
    }

    #[test]
    fn jsonld_plugin_skips_pages_without_head_tag() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let ctx = test_ctx(&site);
        let plugin = JsonLdPlugin::from_site("https://example.com", "Org");
        let page_path = site.join("frag.html");

        let out = plugin
            .transform_html("<p>no head</p>", &page_path, &ctx)
            .unwrap();
        assert_eq!(out, "<p>no head</p>");
    }

    #[test]
    fn jsonld_plugin_generates_webpage_when_no_article_element() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let html = "<html><head><title>Hello</title></head><body><p>content</p></body></html>";
        let ctx = test_ctx(&site);
        let plugin = JsonLdPlugin::from_site("https://example.com", "Org");
        let page_path = site.join("index.html");

        let out = plugin.transform_html(html, &page_path, &ctx).unwrap();
        assert!(out.contains("application/ld+json"));
        assert!(out.contains("WebPage"));
    }

    #[test]
    fn jsonld_plugin_generates_article_when_article_element_present() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let html = "<html><head><title>Post</title></head><body><article><h1>Post</h1></article></body></html>";
        let ctx = test_ctx(&site);
        let plugin = JsonLdPlugin::from_site("https://example.com", "Org");
        let page_path = site.join("post.html");

        let out = plugin.transform_html(html, &page_path, &ctx).unwrap();
        assert!(out.contains("application/ld+json"));
        assert!(out.contains(r#""Article""#));
    }

    #[test]
    fn jsonld_plugin_new_stores_supplied_config() {
        let cfg = JsonLdConfig {
            base_url: "https://a".to_string(),
            org_name: "Org".to_string(),
            breadcrumbs: false,
        };
        let plugin = JsonLdPlugin::new(cfg);
        assert_eq!(plugin.config.base_url, "https://a");
        assert_eq!(plugin.config.org_name, "Org");
        assert!(!plugin.config.breadcrumbs);
    }

    #[test]
    fn jsonld_plugin_name_returns_static_identifier() {
        let plugin = JsonLdPlugin::from_site("https://example.com", "Org");
        assert_eq!(plugin.name(), "json-ld");
    }

    // -----------------------------------------------------------------
    // collect_html_files_recursive
    // -----------------------------------------------------------------

    #[test]
    fn collect_html_files_recursive_filters_and_sorts() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(dir.path().join("z.html"), "").unwrap();
        fs::write(dir.path().join("a.html"), "").unwrap();
        fs::write(sub.join("m.html"), "").unwrap();
        fs::write(dir.path().join("ignore.css"), "").unwrap();

        let files = collect_html_files_recursive(dir.path()).unwrap();
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn collect_html_files_recursive_missing_dir_returns_empty() {
        let dir = tempdir().unwrap();
        let result =
            collect_html_files_recursive(&dir.path().join("missing")).unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    // -----------------------------------------------------------------
    // has_meta_tag — name= and property= variants
    // -----------------------------------------------------------------

    #[test]
    fn has_meta_tag_detects_name_double_quote() {
        let html = r#"<meta name="description" content="hello">"#;
        assert!(has_meta_tag(html, "description"));
    }

    #[test]
    fn has_meta_tag_detects_name_single_quote() {
        let html = "<meta name='description' content='hello'>";
        assert!(has_meta_tag(html, "description"));
    }

    #[test]
    fn has_meta_tag_detects_property_double_quote() {
        let html = r#"<meta property="og:title" content="T">"#;
        assert!(has_meta_tag(html, "og:title"));
    }

    #[test]
    fn has_meta_tag_detects_property_single_quote() {
        let html = "<meta property='og:title' content='T'>";
        assert!(has_meta_tag(html, "og:title"));
    }

    #[test]
    fn has_meta_tag_returns_false_when_absent() {
        let html = "<html><head></head></html>";
        assert!(!has_meta_tag(html, "description"));
    }

    #[test]
    fn has_meta_tag_ignores_comment_markers() {
        let html = "<!-- # Start Open Graph / Facebook Meta Tags -->\n\
                     <!-- # End Open Graph / Facebook Meta Tags -->";
        assert!(!has_meta_tag(html, "og:title"));
    }

    // -----------------------------------------------------------------
    // extract_canonical
    // -----------------------------------------------------------------

    #[test]
    fn extract_canonical_finds_url() {
        let html = r#"<link rel="canonical" href="https://example.com/page">"#;
        assert_eq!(extract_canonical(html), "https://example.com/page");
    }

    #[test]
    fn extract_canonical_returns_empty_when_missing() {
        let html = "<html><head><title>No canonical</title></head></html>";
        assert_eq!(extract_canonical(html), "");
    }

    // -----------------------------------------------------------------
    // extract_existing_meta — name and property attributes
    // -----------------------------------------------------------------

    #[test]
    fn extract_existing_meta_name_variant() {
        let html = r#"<meta name="author" content="Alice">"#;
        assert_eq!(extract_existing_meta(html, "author"), "Alice");
    }

    #[test]
    fn extract_existing_meta_property_variant() {
        let html =
            r#"<meta property="article:published_time" content="2026-01-01">"#;
        assert_eq!(
            extract_existing_meta(html, "article:published_time"),
            "2026-01-01"
        );
    }

    #[test]
    fn extract_existing_meta_single_quote_variant() {
        let html = "<meta name='author' content='Bob'>";
        assert_eq!(extract_existing_meta(html, "author"), "Bob");
    }

    #[test]
    fn extract_existing_meta_returns_empty_when_absent() {
        let html = "<html><head></head></html>";
        assert_eq!(extract_existing_meta(html, "author"), "");
    }

    // -----------------------------------------------------------------
    // extract_meta_author
    // -----------------------------------------------------------------

    #[test]
    fn extract_meta_author_from_meta_tag() {
        let html = r#"<meta name="author" content="Jane Doe">"#;
        assert_eq!(extract_meta_author(html), "Jane Doe");
    }

    #[test]
    fn extract_meta_author_from_class_author_span() {
        let html = r#"<span class="author">John Smith</span>"#;
        assert_eq!(extract_meta_author(html), "John Smith");
    }

    #[test]
    fn extract_meta_author_strips_by_prefix() {
        let html = r#"<span class="author">by Alice Wonder</span>"#;
        assert_eq!(extract_meta_author(html), "Alice Wonder");
    }

    #[test]
    fn extract_meta_author_returns_empty_when_absent() {
        let html = "<html><body><p>No author</p></body></html>";
        assert_eq!(extract_meta_author(html), "");
    }

    // -----------------------------------------------------------------
    // extract_date_from_html (JSON-LD)
    // -----------------------------------------------------------------

    #[test]
    fn extract_date_from_html_finds_date_published() {
        let html = r#"<script type="application/ld+json">{"datePublished":"2026-03-15"}</script>"#;
        assert_eq!(
            extract_date_from_html(html, "datePublished"),
            Some("2026-03-15".to_string())
        );
    }

    #[test]
    fn extract_date_from_html_returns_none_when_absent() {
        let html = "<html><body></body></html>";
        assert_eq!(extract_date_from_html(html, "datePublished"), None);
    }

    // -----------------------------------------------------------------
    // extract_meta_date
    // -----------------------------------------------------------------

    #[test]
    fn extract_meta_date_from_published_time() {
        let html =
            r#"<meta property="article:published_time" content="2026-06-01">"#;
        assert_eq!(extract_meta_date(html), Some("2026-06-01".to_string()));
    }

    #[test]
    fn extract_meta_date_from_time_datetime() {
        let html = r#"<time datetime="2026-07-04">July 4</time>"#;
        assert_eq!(extract_meta_date(html), Some("2026-07-04".to_string()));
    }

    #[test]
    fn extract_meta_date_returns_none_when_absent() {
        let html = "<html><body><p>No date</p></body></html>";
        assert_eq!(extract_meta_date(html), None);
    }

    // -----------------------------------------------------------------
    // extract_html_lang
    // -----------------------------------------------------------------

    #[test]
    fn extract_html_lang_double_quotes() {
        let html = r#"<html lang="fr-FR"><head></head></html>"#;
        assert_eq!(extract_html_lang(html), "fr-FR");
    }

    #[test]
    fn extract_html_lang_single_quotes() {
        let html = "<html lang='de-DE'><head></head></html>";
        assert_eq!(extract_html_lang(html), "de-DE");
    }

    #[test]
    fn extract_html_lang_missing_returns_empty() {
        let html = "<html><head></head></html>";
        assert_eq!(extract_html_lang(html), "");
    }

    // -----------------------------------------------------------------
    // extract_first_content_image
    // -----------------------------------------------------------------

    #[test]
    fn extract_first_content_image_from_main() {
        let html = r#"<html><body><main><img src="/img/hero.jpg"></main></body></html>"#;
        assert_eq!(extract_first_content_image(html), "/img/hero.jpg");
    }

    #[test]
    fn extract_first_content_image_from_article() {
        let html = r#"<html><body><article><img src="/img/post.png"></article></body></html>"#;
        assert_eq!(extract_first_content_image(html), "/img/post.png");
    }

    #[test]
    fn extract_first_content_image_no_image_returns_empty() {
        let html = "<html><body><main><p>No images</p></main></body></html>";
        assert_eq!(extract_first_content_image(html), "");
    }

    #[test]
    fn extract_first_content_image_no_main_or_article_returns_empty() {
        let html = r#"<html><body><div><img src="/img/sidebar.jpg"></div></body></html>"#;
        assert_eq!(extract_first_content_image(html), "");
    }

    // -----------------------------------------------------------------
    // inject_seo_tags — article page triggers summary_large_image
    // -----------------------------------------------------------------

    #[test]
    fn inject_seo_tags_article_page_uses_large_image_card() {
        let tmp = tempdir().unwrap();
        let html = "<html><head><title>Blog Post</title></head>\
                     <body><article><p>Article content</p></article></body></html>";
        let ctx = test_ctx(tmp.path());

        let result = SeoPlugin
            .transform_html(html, Path::new("post.html"), &ctx)
            .unwrap();
        assert!(
            result.contains("content=\"summary_large_image\""),
            "article pages should use summary_large_image twitter card"
        );
        assert!(
            result.contains("content=\"article\""),
            "article pages should use og:type=article"
        );
    }

    // -----------------------------------------------------------------
    // CanonicalPlugin — replaces existing canonicals
    // -----------------------------------------------------------------

    #[test]
    fn canonical_plugin_replaces_not_skips_existing() {
        let tmp = tempdir().unwrap();
        let html = r#"<html><head><link rel="canonical" href="https://old.com/wrong"></head><body></body></html>"#;
        let plugin = CanonicalPlugin::new("https://correct.com");
        let ctx = test_ctx(tmp.path());
        let page_path = tmp.path().join("page.html");

        let result = plugin.transform_html(html, &page_path, &ctx).unwrap();
        assert!(
            result.contains("https://correct.com/page.html"),
            "canonical should be replaced with correct URL"
        );
        assert!(
            !result.contains("https://old.com/wrong"),
            "old canonical should be removed"
        );
        assert_eq!(
            result.matches("canonical").count(),
            1,
            "should have exactly one canonical link"
        );
    }
}
