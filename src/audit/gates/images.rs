// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Image optimisation + alt-text gate.
//!
//! Per page:
//! - Every `<img>` has an `alt` attribute (error).
//! - Every `<img>` has explicit `width` + `height` (warn — required
//!   to avoid CLS).
//! - Each referenced image file has a sibling `.webp` or `.avif` (warn).
//! - Each referenced image file is under the configured size budget
//!   (warn).

use super::super::{AuditGate, AuditOptions, Finding, Severity, Site};
use super::{find_tag_end, hreflang_attr};

const NAME: &str = "images";

/// Image optimisation + alt-text gate.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditGate;
/// use ssg::audit::gates::images::ImagesGate;
/// assert_eq!(ImagesGate.name(), "images");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ImagesGate;

impl AuditGate for ImagesGate {
    fn name(&self) -> &'static str {
        NAME
    }

    fn explain(&self) -> &'static str {
        "Per <img>: asserts alt text (error), explicit width + height \
         (warn), and that the referenced file has a sibling .webp or \
         .avif source (warn — image-optimization plugin emits both). \
         Files larger than `image_budget` raise a warn so authors \
         catch unoptimised originals."
    }

    fn run(&self, site: &Site, opts: &AuditOptions) -> Vec<Finding> {
        let mut findings = Vec::new();
        for path in &site.html_files {
            let Ok(html) = site.read(path) else { continue };
            let rel = site.rel(path);
            for img in extract_imgs(&html) {
                if img.alt.is_none() {
                    findings.push(
                        Finding::new(
                            NAME,
                            Severity::Error,
                            format!("<img src=\"{}\"> missing alt", img.src),
                        )
                        .with_code("IMG-ALT")
                        .with_path(rel.clone()),
                    );
                }
                if img.width.is_none() || img.height.is_none() {
                    findings.push(
                        Finding::new(
                            NAME,
                            Severity::Warn,
                            format!(
                                "<img src=\"{}\"> missing explicit width/height (CLS risk)",
                                img.src
                            ),
                        )
                        .with_code("IMG-DIMS")
                        .with_path(rel.clone()),
                    );
                }
                if img.src.starts_with("http://")
                    || img.src.starts_with("https://")
                    || img.src.starts_with("//")
                    || img.src.starts_with("data:")
                {
                    continue;
                }
                let candidate = resolve_img_candidate(site, path, &img.src);
                if let Ok(meta) = std::fs::metadata(&candidate) {
                    if meta.len() as usize > opts.image_budget {
                        findings.push(
                            Finding::new(
                                NAME,
                                Severity::Warn,
                                format!(
                                    "{} weighs {} bytes (budget {})",
                                    img.src,
                                    meta.len(),
                                    opts.image_budget
                                ),
                            )
                            .with_code("IMG-OVER-BUDGET")
                            .with_path(rel.clone()),
                        );
                    }
                    // A vector source is already the resolution-independent
                    // format; a raster sibling would be a downgrade, so asking
                    // for one produces a warning nothing can ever clear.
                    let is_vector = candidate
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
                    // Check for sibling .webp / .avif
                    let stem = candidate.with_extension("");
                    let has_webp = stem.with_extension("webp").exists();
                    let has_avif = stem.with_extension("avif").exists();
                    if !is_vector && !has_webp && !has_avif {
                        findings.push(
                            Finding::new(
                                NAME,
                                Severity::Warn,
                                format!(
                                    "{} has no sibling .webp or .avif source",
                                    img.src
                                ),
                            )
                            .with_code("IMG-NO-MODERN")
                            .with_path(rel.clone()),
                        );
                    }
                }
            }
        }
        findings
    }
}

/// Resolves an `<img src>` value to the on-disk file it references:
/// site-absolute paths anchor at the site root, relative paths at the
/// containing page's directory (falling back to the root when the page
/// path has no parent component).
fn resolve_img_candidate(
    site: &Site,
    page: &std::path::Path,
    src: &str,
) -> std::path::PathBuf {
    if let Some(s) = src.strip_prefix('/') {
        site.root.join(s)
    } else if let Some(parent) = page.parent() {
        parent.join(src)
    } else {
        site.root.join(src)
    }
}

struct ImgRef {
    src: String,
    alt: Option<String>,
    width: Option<String>,
    height: Option<String>,
}

fn extract_imgs(html: &str) -> Vec<ImgRef> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<img") {
        let abs = cursor + rel;
        // Quote-aware end detection: SVG data-URIs in `src` carry raw
        // `>` characters that a naive find('>') truncates on.
        let end = find_tag_end(html, abs);
        let tag = &html[abs..end];
        cursor = end;
        let src = hreflang_attr(tag, "src").unwrap_or_default();
        let alt = hreflang_attr(tag, "alt");
        let width = hreflang_attr(tag, "width");
        let height = hreflang_attr(tag, "height");
        out.push(ImgRef {
            src,
            alt,
            width,
            height,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_with(html: &str, image_bytes: usize) -> Site {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let p = root.join("page.html");
        std::fs::write(&p, html).unwrap();
        std::fs::write(root.join("a.jpg"), vec![0u8; image_bytes]).unwrap();
        std::fs::write(root.join("a.webp"), vec![0u8; 10]).unwrap();
        std::mem::forget(tmp);
        Site {
            root,
            html_files: vec![p],
        }
    }

    #[test]
    fn passing_image_is_clean() {
        let html = r#"<html><body><img src="a.jpg" alt="a" width="10" height="10"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(f.is_empty(), "got {f:?}");
    }

    #[test]
    fn missing_alt_flags_error() {
        let html = r#"<html><body><img src="a.jpg" width="10" height="10"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("IMG-ALT")));
    }

    #[test]
    fn over_budget_image_flagged() {
        let html = r#"<html><body><img src="a.jpg" alt="a" width="10" height="10"></body></html>"#;
        let f = ImagesGate.run(
            &site_with(html, 5000),
            &AuditOptions {
                image_budget: 100,
                ..AuditOptions::default()
            },
        );
        assert!(f
            .iter()
            .any(|x| x.code.as_deref() == Some("IMG-OVER-BUDGET")));
    }

    #[test]
    fn missing_width_height_warns_with_dims_code() {
        let html = r#"<html><body><img src="a.jpg" alt="a"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        let dims = f
            .iter()
            .find(|x| x.code.as_deref() == Some("IMG-DIMS"))
            .expect("dims finding");
        assert_eq!(dims.severity, Severity::Warn);
    }

    #[test]
    fn missing_only_height_still_flags_dims() {
        let html =
            r#"<html><body><img src="a.jpg" alt="a" width="10"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("IMG-DIMS")));
    }

    #[test]
    fn no_modern_sibling_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(root.join("plain.png"), vec![0u8; 10]).unwrap();
        let html_path = root.join("page.html");
        std::fs::write(
            &html_path,
            r#"<html><body><img src="plain.png" alt="p" width="1" height="1"></body></html>"#,
        )
        .unwrap();
        std::mem::forget(tmp);
        let s = Site {
            root,
            html_files: vec![html_path],
        };
        let f = ImagesGate.run(&s, &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("IMG-NO-MODERN")));
    }

    #[test]
    fn svg_is_not_asked_for_a_raster_sibling() {
        // An SVG is already resolution-independent; a .webp or .avif sibling
        // would be a downgrade, not a modern format. Asking for one made the
        // theme suite's logo a permanent warning nothing could clear.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(
            root.join("logo.svg"),
            b"<svg xmlns='http://www.w3.org/2000/svg'/>",
        )
        .unwrap();
        let html_path = root.join("page.html");
        std::fs::write(
            &html_path,
            r#"<html><body><img src="logo.svg" alt="l" width="1" height="1"></body></html>"#,
        )
        .unwrap();
        std::mem::forget(tmp);
        let s = Site {
            root,
            html_files: vec![html_path],
        };
        let f = ImagesGate.run(&s, &AuditOptions::default());
        assert!(
            f.iter().all(|x| x.code.as_deref() != Some("IMG-NO-MODERN")),
            "an SVG should not be asked for a raster sibling: {f:?}"
        );
    }

    #[test]
    fn avif_sibling_suppresses_no_modern() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(root.join("hero.png"), vec![0u8; 10]).unwrap();
        std::fs::write(root.join("hero.avif"), vec![0u8; 5]).unwrap();
        let html_path = root.join("page.html");
        // Second <img> lacks height: the IMG-DIMS finding keeps `f`
        // non-empty so the suppression predicate actually evaluates.
        std::fs::write(
            &html_path,
            r#"<html><body><img src="hero.png" alt="h" width="1" height="1"><img src="hero.png" alt="h2" width="1"></body></html>"#,
        )
        .unwrap();
        std::mem::forget(tmp);
        let s = Site {
            root,
            html_files: vec![html_path],
        };
        let f = ImagesGate.run(&s, &AuditOptions::default());
        assert!(f.iter().all(|x| x.code.as_deref() != Some("IMG-NO-MODERN")));
    }

    #[test]
    fn external_image_src_is_skipped() {
        // Height intentionally missing: the IMG-DIMS finding keeps `f`
        // non-empty so the not-probed predicate actually evaluates.
        let html = r#"<html><body><img src="https://cdn.example/a.jpg" alt="x" width="1"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("IMG-DIMS")));
        assert!(
            f.iter()
                .all(|x| x.code.as_deref() != Some("IMG-OVER-BUDGET")
                    && x.code.as_deref() != Some("IMG-NO-MODERN")),
            "external imgs should not be probed; got {f:?}"
        );
    }

    #[test]
    fn data_uri_image_src_is_skipped() {
        // Height intentionally missing: IMG-DIMS keeps `f` non-empty.
        let html = r#"<html><body><img src="data:image/png;base64,iVBOR" alt="x" width="1"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(f
            .iter()
            .all(|x| x.code.as_deref() != Some("IMG-OVER-BUDGET")));
    }

    #[test]
    fn protocol_relative_image_src_is_skipped() {
        // Height intentionally missing: IMG-DIMS keeps `f` non-empty.
        let html = r#"<html><body><img src="//cdn.example/a.jpg" alt="x" width="1"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(f.iter().all(|x| x.code.as_deref() != Some("IMG-NO-MODERN")));
    }

    #[test]
    fn svg_data_uri_with_raw_gt_does_not_truncate_tag() {
        // Regression: naive find('>') cut the tag at the first `>`
        // inside the data-URI, losing alt/width/height that follow.
        // The second <img> yields IMG-NO-MODERN so `f` is non-empty
        // and the exemption predicate actually evaluates.
        let html = "<html><body><img alt=\"Banner\" \
            src=\"data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg'>\
            <rect width='1' height='1'/></svg>\" \
            width=\"1440\" height=\"398\">\
            <img src=\"plain.png\" alt=\"p\" width=\"1\" height=\"1\">\
            </body></html>";
        let s = site_with(html, 10);
        std::fs::write(s.root.join("plain.png"), vec![0u8; 10]).unwrap();
        let f = ImagesGate.run(&s, &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("IMG-NO-MODERN")));
        assert!(
            f.iter().all(|x| x.code.as_deref() != Some("IMG-ALT")
                && x.code.as_deref() != Some("IMG-DIMS")),
            "attributes after a data-URI must be seen: {f:?}"
        );
    }

    #[test]
    fn minified_valueless_alt_counts_as_alt() {
        // Minifiers collapse alt="" to bare `alt` on decorative images.
        // The plain.png <img> yields IMG-NO-MODERN so the exemption
        // predicate below actually evaluates per finding.
        let html = "<html><body>\
            <img alt height=33 role=presentation src=a.jpg width=100>\
            <img src=plain.png alt=p width=1 height=1>\
            </body></html>";
        let s = site_with(html, 10);
        std::fs::write(s.root.join("plain.png"), vec![0u8; 10]).unwrap();
        let f = ImagesGate.run(&s, &AuditOptions::default());
        assert!(f.iter().any(|x| x.code.as_deref() == Some("IMG-NO-MODERN")));
        assert!(
            f.iter().all(|x| x.code.as_deref() != Some("IMG-ALT")
                && x.code.as_deref() != Some("IMG-DIMS")),
            "bare `alt` + unquoted dims must count: {f:?}"
        );
    }

    #[test]
    fn truly_missing_alt_still_flagged_on_minified_tag() {
        // True positive preserved on unquoted minified markup.
        let html = "<html><body>\
            <img height=33 src=a.jpg width=100>\
            </body></html>";
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(
            f.iter().any(|x| x.code.as_deref() == Some("IMG-ALT")),
            "missing alt must still flag: {f:?}"
        );
    }

    #[test]
    fn unreadable_html_skipped_no_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("ghost.html");
        let s = Site {
            root: tmp.path().to_path_buf(),
            html_files: vec![bogus],
        };
        std::mem::forget(tmp);
        let f = ImagesGate.run(&s, &AuditOptions::default());
        assert!(f.is_empty());
    }

    #[test]
    fn site_absolute_src_resolves_from_root() {
        let html = r#"<html><body><img src="/a.jpg" alt="a" width="10" height="10"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(
            f.is_empty(),
            "leading-slash src must anchor at site root: {f:?}"
        );
    }

    #[test]
    fn missing_local_image_file_is_not_probed() {
        // fs::metadata fails, so budget/modern-sibling checks skip.
        let html = r#"<html><body><img src="ghost.png" alt="g" width="1" height="1"></body></html>"#;
        let f = ImagesGate.run(&site_with(html, 10), &AuditOptions::default());
        assert!(f.is_empty(), "nonexistent file must be skipped: {f:?}");
    }

    #[test]
    fn candidate_for_parentless_page_falls_back_to_root() {
        let s = site_with("<html></html>", 10);
        let got =
            resolve_img_candidate(&s, std::path::Path::new(""), "pic.jpg");
        assert_eq!(got, s.root.join("pic.jpg"));
    }

    #[test]
    fn metadata_methods_exposed() {
        let g = ImagesGate;
        assert_eq!(g.name(), "images");
        assert!(g.explain().contains("alt"));
        let _copy: ImagesGate = g;
        let _clone = g;
        assert!(format!("{g:?}").contains("ImagesGate"));
    }
}
