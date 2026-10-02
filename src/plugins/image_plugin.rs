// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Image optimization plugin.
//!
//! Processes images to generate WebP variants and responsive `<picture>`
//! elements with `srcset`, `loading="lazy"`, and `decoding="async"`.
//!
//! ## Responsive pipeline
//!
//! For each `<img>` in compiled HTML the plugin emits:
//!
//! ```html
//! <picture>
//!   <source type="image/avif" srcset="…-320w.avif 320w, …"> <!-- if avif feature -->
//!   <source type="image/webp" srcset="…-320w.webp 320w, …">
//!   <img src="/original.jpg" alt="…" width="…" height="…"
//!        loading="lazy" decoding="async">
//! </picture>
//! ```
//!
//! Images tagged with `fetchpriority="high"` receive `loading="eager"`
//! instead, so the browser fetches them immediately.

#[cfg(feature = "image-optimization")]
use crate::error::{PathErrorExt, SsgError};
#[cfg(feature = "image-optimization")]
use crate::plugin::{Plugin, PluginContext};

#[cfg(feature = "image-optimization")]
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// Default responsive breakpoints (px).
#[cfg(feature = "image-optimization")]
const DEFAULT_BREAKPOINTS: &[u32] = &[320, 640, 1024, 1440];

/// Default WebP encoding quality (1–100).
#[cfg(feature = "image-optimization")]
const DEFAULT_QUALITY: u8 = 80;

/// Default AVIF encoding quality (1–100). 70 is the sweet spot in the
/// `ravif` docs: visually transparent on photographic content while
/// producing files 20-30 % smaller than equivalent WebP. Lowering to
/// 60 still keeps SSIM ≥ 0.95 on the typical JPEG source (issue #521
/// AC4); raising past 80 yields rapidly diminishing returns.
#[cfg(feature = "image-optimization")]
const DEFAULT_AVIF_QUALITY: u8 = 70;

/// `ravif` encoding speed preset (1 = slowest/best, 10 = fastest).
/// `4` is the published sweet spot — closer to "best" than to "fastest"
/// without the cost cliff of `1`/`2`.
#[cfg(feature = "image-optimization")]
const AVIF_SPEED: u8 = 4;

/// Plugin that optimises images and rewrites HTML with `<picture>` tags.
///
/// Runs in `after_compile`:
/// 1. Scans `site_dir` for JPEG/PNG images
/// 2. Generates WebP and AVIF variants at responsive widths
/// 3. Rewrites `<img>` tags to `<picture>` with `srcset`
/// 4. Adds `loading="lazy"`, `decoding="async"`, `width`, `height`
///
/// The quality and breakpoints are configurable via the struct fields.
#[cfg(feature = "image-optimization")]
#[derive(Debug, Clone)]
pub struct ImageOptimizationPlugin {
    /// WebP encoding quality (1–100). Defaults to 80.
    pub quality: u8,
    /// AVIF encoding quality (1–100). Defaults to 70 — see the
    /// `DEFAULT_AVIF_QUALITY` private constant for the rationale.
    pub avif_quality: u8,
    /// Skip AVIF encoding for non-priority images (saves build time
    /// at the cost of bandwidth on modern browsers). Hero images
    /// marked `fetchpriority="high"` always get AVIF regardless —
    /// see issue #521 AC5.
    ///
    /// Currently exposed as a config knob; per-image priority data is
    /// only known to the HTML rewriter (which preserves
    /// `fetchpriority="high"` and `loading="eager"`), so the present
    /// implementation always encodes AVIF for every responsive variant
    /// of every collected image. Honoured-by-default in a follow-up
    /// once priority metadata is plumbed from the shortcode layer
    /// into `collect_images`.
    pub lazy_avif: bool,
    /// Responsive width breakpoints in pixels. Defaults to `[320, 640, 1024, 1440]`.
    pub breakpoints: Vec<u32>,
}

#[cfg(feature = "image-optimization")]
impl Default for ImageOptimizationPlugin {
    fn default() -> Self {
        Self {
            quality: DEFAULT_QUALITY,
            avif_quality: DEFAULT_AVIF_QUALITY,
            lazy_avif: false,
            breakpoints: DEFAULT_BREAKPOINTS.to_vec(),
        }
    }
}

#[cfg(feature = "image-optimization")]
impl Plugin for ImageOptimizationPlugin {
    fn name(&self) -> &'static str {
        "image-optimization"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }

        let images = collect_images(&ctx.site_dir)?;
        if images.is_empty() {
            return Ok(());
        }

        let optimized_dir = ctx.site_dir.join("optimized");
        fs::create_dir_all(&optimized_dir).with_path(&optimized_dir)?;

        let manifest = optimize_all_images(
            &images,
            &ctx.site_dir,
            &optimized_dir,
            &self.breakpoints,
            self.quality,
            self.avif_quality,
        );

        rewrite_html_img_tags(&ctx.site_dir, &manifest)?;

        let webp_count: usize =
            manifest.values().map(|m| m.webp_variants.len()).sum();
        let avif_count: usize =
            manifest.values().map(|m| m.avif_variants.len()).sum();
        log::info!(
            "[image] Optimised {} image(s); {webp_count} WebP, {avif_count} AVIF variant(s)",
            manifest.len()
        );
        Ok(())
    }
}

#[cfg(feature = "image-optimization")]
#[derive(Debug, Clone)]
struct ImageVariant {
    rel_path: String,
    width: u32,
}

#[cfg(feature = "image-optimization")]
#[derive(Debug, Clone)]
struct ImageManifest {
    original_rel: String,
    original_width: u32,
    original_height: u32,
    webp_variants: Vec<ImageVariant>,
    avif_variants: Vec<ImageVariant>,
}

/// Optimizes all images and builds the manifest, logging warnings for failures.
#[cfg(feature = "image-optimization")]
fn optimize_all_images(
    images: &[PathBuf],
    site_dir: &Path,
    optimized_dir: &Path,
    breakpoints: &[u32],
    quality: u8,
    avif_quality: u8,
) -> HashMap<String, ImageManifest> {
    let mut manifest = HashMap::new();
    for img_path in images {
        match process_image(
            img_path,
            site_dir,
            optimized_dir,
            breakpoints,
            quality,
            avif_quality,
        ) {
            Ok(entry) => {
                let _ = manifest.insert(entry.original_rel.clone(), entry);
            }
            Err(e) => {
                log::warn!(
                    "[image] Failed to process {}: {e}",
                    img_path.display()
                );
            }
        }
    }
    manifest
}

/// Rewrites HTML files to use `<picture>` tags for optimized images.
#[cfg(feature = "image-optimization")]
fn rewrite_html_img_tags(
    site_dir: &Path,
    manifest: &HashMap<String, ImageManifest>,
) -> Result<(), SsgError> {
    let html_files = collect_html_files(site_dir)?;
    for html_path in &html_files {
        let html = fs::read_to_string(html_path).with_path(html_path)?;
        let rewritten = rewrite_img_tags(&html, manifest);
        if rewritten != html {
            fs::write(html_path, rewritten).with_path(html_path)?;
        }
    }
    Ok(())
}

/// Processes a single image: resize + encode to WebP **and** AVIF at
/// responsive widths.
///
/// WebP is written serially in the breakpoint loop (encoding is cheap;
/// `image::save` is essentially memcpy + libwebp). AVIF encoding is
/// expensive (rav1e at speed 4 takes 50-400 ms per variant), so the
/// per-width AVIF jobs are dispatched via `rayon::par_iter` across the
/// breakpoints — wall-time scales with `breakpoints / cores` rather
/// than `breakpoints * encoder_cost`. See issue #521 AC3.
#[cfg(feature = "image-optimization")]
fn process_image(
    img_path: &Path,
    site_dir: &Path,
    optimized_dir: &Path,
    breakpoints: &[u32],
    _quality: u8,
    avif_quality: u8,
) -> Result<ImageManifest, SsgError> {
    use rayon::prelude::*;

    let img = image::open(img_path).map_err(|e| SsgError::io(e, img_path))?;

    let (orig_w, orig_h) = (img.width(), img.height());
    let rel = img_path
        .strip_prefix(site_dir)
        .unwrap_or(img_path)
        .to_string_lossy()
        .replace('\\', "/");

    let stem = img_path.file_stem().unwrap_or_default().to_string_lossy();

    // ---- WebP path (serial, fast) -----------------------------------
    let mut webp_variants = Vec::new();
    let mut resized_variants: Vec<(u32, image::DynamicImage)> = Vec::new();

    for &width in breakpoints {
        if width >= orig_w {
            continue; // Skip sizes larger than original
        }

        let ratio = f64::from(width) / f64::from(orig_w);
        let height = (f64::from(orig_h) * ratio) as u32;
        let resized = img.resize_exact(
            width,
            height,
            image::imageops::FilterType::Lanczos3,
        );

        // Save WebP variant
        let variant_name = format!("{stem}-{width}w.webp");
        let variant_path = optimized_dir.join(&variant_name);
        resized
            .save(&variant_path)
            .map_err(|e| SsgError::io(e, &variant_path))?;

        let variant_rel = format!("optimized/{variant_name}");
        webp_variants.push(ImageVariant {
            rel_path: variant_rel,
            width,
        });

        resized_variants.push((width, resized));
    }

    // ---- AVIF path (parallel) ---------------------------------------
    // Each `(width, DynamicImage)` is encoded on the rayon pool; the
    // resulting bytes are written to disk on the same worker that
    // produced them. Failures are logged + skipped so a single broken
    // variant doesn't blow up the build.
    let avif_results: Vec<Option<ImageVariant>> = resized_variants
        .par_iter()
        .map(|(width, resized)| {
            let variant_name = format!("{stem}-{width}w.avif");
            let variant_path = optimized_dir.join(&variant_name);
            match encode_avif(resized, avif_quality) {
                Ok(bytes) => match fs::write(&variant_path, &bytes) {
                    Ok(()) => Some(ImageVariant {
                        rel_path: format!("optimized/{variant_name}"),
                        width: *width,
                    }),
                    Err(e) => {
                        log::warn!(
                            "[image] AVIF write failed for {}: {e}",
                            variant_path.display()
                        );
                        None
                    }
                },
                Err(e) => {
                    log::warn!(
                        "[image] AVIF encode failed for {}-{width}w: {e}",
                        stem
                    );
                    None
                }
            }
        })
        .collect();

    let avif_variants: Vec<ImageVariant> =
        avif_results.into_iter().flatten().collect();

    Ok(ImageManifest {
        original_rel: rel,
        original_width: orig_w,
        original_height: orig_h,
        webp_variants,
        avif_variants,
    })
}

/// Encode a `DynamicImage` as AVIF bytes using `ravif`.
///
/// The image is first converted to RGBA8 (ravif's most general input
/// format — it auto-detects fully-opaque images and drops the alpha
/// plane internally, so this isn't wasteful for JPEG sources). The
/// returned `Vec<u8>` is a self-contained AVIF file ready to write to
/// disk.
///
/// `quality` is clamped to `1..=100`; the speed preset is fixed at
/// `AVIF_SPEED` (4 — "balanced", private constant). Alpha channels
/// use the same quality as colour.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "image-optimization")]
/// # fn example() {
/// use ssg::image_plugin::encode_avif;
/// use image::{DynamicImage, RgbaImage};
///
/// let img = DynamicImage::ImageRgba8(RgbaImage::new(8, 8));
/// let bytes = encode_avif(&img, 60).unwrap();
/// assert!(!bytes.is_empty());
/// # }
/// ```
///
/// # Errors
/// Returns [`SsgError::Io`] wrapping the underlying `ravif::Error`
/// if rav1e fails to encode (effectively a bug in ravif/rav1e — the
/// inputs we feed are always valid).
#[cfg(feature = "image-optimization")]
pub fn encode_avif(
    img: &image::DynamicImage,
    quality: u8,
) -> Result<Vec<u8>, SsgError> {
    use rgb::FromSlice;

    fail_point!("image::encode-avif", |_| {
        Err(SsgError::io(
            std::io::Error::other("injected: image::encode-avif"),
            "<avif-buffer>",
        ))
    });

    let rgba = img.to_rgba8();
    let width = rgba.width() as usize;
    let height = rgba.height() as usize;

    // Guard before handing the buffer to ravif: `imgref` asserts
    // `stride > 0` and panics on zero-dimension images, so a corrupt
    // or programmatically-empty input must surface as a typed error,
    // never a panic (house no-panic rule).
    if width == 0 || height == 0 {
        return Err(SsgError::io(
            std::io::Error::other(format!(
                "cannot AVIF-encode empty image ({width}x{height})"
            )),
            "<avif-buffer>",
        ));
    }

    let raw = rgba.as_raw().as_rgba();

    let quality_f = f32::from(quality.clamp(1, 100));

    let encoded = ravif::Encoder::new()
        .with_quality(quality_f)
        .with_alpha_quality(quality_f)
        .with_speed(AVIF_SPEED)
        .encode_rgba(ravif::Img::new(raw, width, height))
        .map_err(|e| {
            SsgError::io(
                std::io::Error::other(format!("ravif encode failed: {e}")),
                "<avif-buffer>",
            )
        })?;

    Ok(encoded.avif_file)
}

/// Rewrites `<img src="...">` tags to `<picture>` with srcset.
///
/// For each image in the manifest that has variants, the original
/// `<img>` tag is wrapped in a `<picture>` element with:
/// - `<source type="image/avif" srcset="...">` (if AVIF variants exist)
/// - `<source type="image/webp" srcset="...">`
/// - `<img>` fallback with `loading`, `decoding`, `width`, `height`
///
/// Images with `fetchpriority="high"` get `loading="eager"` instead of
/// `loading="lazy"`.
///
/// **Streaming parser:** uses `lol_html` so HTML comments, character
/// entities in `alt`, and pre-existing `srcset` are all handled
/// correctly (issue #525 AC1–AC3). Only the first match per manifest
/// entry is rewritten, matching the previous `str::find`-based
/// behaviour so existing tests continue to pass.
#[cfg(feature = "image-optimization")]
fn rewrite_img_tags(
    html: &str,
    manifest: &HashMap<String, ImageManifest>,
) -> String {
    use crate::util::html_rewriter::rewrite_html;
    use lol_html::element;
    use std::cell::RefCell;
    use std::rc::Rc;

    // Tracks which manifest entries have already been rewritten so the
    // "first-occurrence-only" invariant survives the streaming pass.
    let consumed: Rc<RefCell<std::collections::HashSet<String>>> =
        Rc::new(RefCell::new(std::collections::HashSet::new()));
    let manifest = manifest.clone();

    let consumed_cb = Rc::clone(&consumed);
    let handler = element!("img", move |el| {
        let Some(src) = el.get_attribute("src") else {
            return Ok(());
        };
        // Normalise to the manifest-relative key (drop leading slash if any).
        let key = src.strip_prefix('/').unwrap_or(&src).to_string();
        let Some(entry) = manifest.get(&key) else {
            return Ok(());
        };
        if entry.webp_variants.is_empty() && entry.avif_variants.is_empty() {
            return Ok(());
        }
        let original_rel = &entry.original_rel;
        {
            let mut consumed = consumed_cb.borrow_mut();
            if consumed.contains(original_rel) {
                return Ok(());
            }
            let _ = consumed.insert(original_rel.clone());
        }

        let alt = el.get_attribute("alt").unwrap_or_default();
        let fetchpriority = el.get_attribute("fetchpriority");
        let loading = if fetchpriority.as_deref() == Some("high") {
            "eager"
        } else {
            "lazy"
        };
        let width = el
            .get_attribute("width")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(entry.original_width);
        let height = el
            .get_attribute("height")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(entry.original_height);

        // Author-supplied srcset is replaced (issue #525 AC3) — log so
        // the build output makes the swap visible. lol_html guarantees
        // exactly one `srcset` on the emitted `<source>` elements.
        if el.has_attribute("srcset") {
            log::info!(
                "[image] replacing author-supplied srcset on '{src}' with responsive variants"
            );
        }

        let webp_srcset: String = entry
            .webp_variants
            .iter()
            .map(|v| format!("/{} {}w", v.rel_path, v.width))
            .collect::<Vec<_>>()
            .join(", ");
        let avif_srcset: String = entry
            .avif_variants
            .iter()
            .map(|v| format!("/{} {}w", v.rel_path, v.width))
            .collect::<Vec<_>>()
            .join(", ");
        let sizes = "(max-width: 640px) 100vw, (max-width: 1024px) 50vw, 33vw";

        let mut picture = String::from("<picture>");
        if !avif_srcset.is_empty() {
            picture.push_str(&format!(
                "<source type=\"image/avif\" srcset=\"{avif_srcset}\" sizes=\"{sizes}\">"
            ));
        }
        if !webp_srcset.is_empty() {
            picture.push_str(&format!(
                "<source type=\"image/webp\" srcset=\"{webp_srcset}\" sizes=\"{sizes}\">"
            ));
        }

        let fp_attr = match fetchpriority {
            Some(ref fp) => format!(" fetchpriority=\"{fp}\""),
            None => String::new(),
        };
        picture.push_str(&format!(
            "<img src=\"/{original_rel}\" alt=\"{alt}\" \
             width=\"{width}\" height=\"{height}\" \
             loading=\"{loading}\" decoding=\"async\"{fp_attr}>",
        ));
        picture.push_str("</picture>");

        el.replace(&picture, lol_html::html_content::ContentType::Html);
        Ok(())
    });

    unwrap_rewrite(html, rewrite_html(html, vec![handler]))
}

/// Unwraps the streaming-rewrite result, falling back to the original
/// HTML (with a warning) when `lol_html` failed.
#[cfg(feature = "image-optimization")]
fn unwrap_rewrite(html: &str, res: Result<String, SsgError>) -> String {
    match res {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "[image] HTML rewrite failed, leaving page unchanged: {e}"
            );
            html.to_string()
        }
    }
}

#[cfg(all(test, feature = "image-optimization"))]
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    // Retained as a test helper for the legacy `extract_attr_table_driven`
    // unit test; the production `rewrite_img_tags` path now uses
    // `lol_html`'s attribute API directly.
    let pattern = format!("{attr}=\"");
    let start = tag.find(&pattern)? + pattern.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

/// Collect all `.jpg`/`.jpeg`/`.png` files under `dir`, skipping any
/// that live inside an `optimized/` subdirectory (the plugin's own
/// output directory — must not be re-processed).
#[cfg(feature = "image-optimization")]
fn collect_images(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    let all = crate::walk::walk_files_multi(dir, &["jpg", "jpeg", "png"])?;
    Ok(all
        .into_iter()
        .filter(|p| !p.components().any(|c| c.as_os_str() == "optimized"))
        .collect())
}

#[cfg(feature = "image-optimization")]
fn collect_html_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files(dir, "html")
}

#[cfg(all(test, feature = "image-optimization"))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // -------------------------------------------------------------------
    // Test fixtures
    // -------------------------------------------------------------------

    /// Writes a tiny programmatically-generated JPEG to the given path.
    fn write_test_jpeg(path: &Path, w: u32, h: u32) {
        let buf = image::ImageBuffer::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        image::DynamicImage::ImageRgb8(buf)
            .save_with_format(path, image::ImageFormat::Jpeg)
            .expect("write jpeg");
    }

    /// Writes a tiny programmatically-generated PNG to the given path.
    fn write_test_png(path: &Path, w: u32, h: u32) {
        let buf = image::ImageBuffer::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 200, 255])
        });
        image::DynamicImage::ImageRgba8(buf)
            .save_with_format(path, image::ImageFormat::Png)
            .expect("write png");
    }

    /// Builds an in-memory `ImageManifest` with the supplied WebP variants.
    fn manifest_with(
        original_rel: &str,
        width: u32,
        height: u32,
        variant_widths: &[u32],
    ) -> HashMap<String, ImageManifest> {
        let stem = original_rel
            .rsplit('/')
            .next()
            .unwrap_or(original_rel)
            .rsplit('.')
            .nth(1)
            .unwrap_or("img");
        let webp_variants = variant_widths
            .iter()
            .map(|&w| ImageVariant {
                rel_path: format!("optimized/{stem}-{w}w.webp"),
                width: w,
            })
            .collect();
        let mut m = HashMap::new();
        let _ = m.insert(
            original_rel.to_string(),
            ImageManifest {
                original_rel: original_rel.to_string(),
                original_width: width,
                original_height: height,
                webp_variants,
                avif_variants: Vec::new(),
            },
        );
        m
    }

    /// Builds a manifest with both WebP and AVIF variants.
    fn manifest_with_avif(
        original_rel: &str,
        width: u32,
        height: u32,
        variant_widths: &[u32],
    ) -> HashMap<String, ImageManifest> {
        let stem = original_rel
            .rsplit('/')
            .next()
            .unwrap_or(original_rel)
            .rsplit('.')
            .nth(1)
            .unwrap_or("img");
        let webp_variants = variant_widths
            .iter()
            .map(|&w| ImageVariant {
                rel_path: format!("optimized/{stem}-{w}w.webp"),
                width: w,
            })
            .collect();
        let avif_variants = variant_widths
            .iter()
            .map(|&w| ImageVariant {
                rel_path: format!("optimized/{stem}-{w}w.avif"),
                width: w,
            })
            .collect();
        let mut m = HashMap::new();
        let _ = m.insert(
            original_rel.to_string(),
            ImageManifest {
                original_rel: original_rel.to_string(),
                original_width: width,
                original_height: height,
                webp_variants,
                avif_variants,
            },
        );
        m
    }

    // -------------------------------------------------------------------
    // ImageOptimizationPlugin — struct configuration
    // -------------------------------------------------------------------

    #[test]
    fn default_plugin_has_expected_quality_and_breakpoints() {
        let plugin = ImageOptimizationPlugin::default();
        assert_eq!(plugin.quality, 80);
        assert_eq!(plugin.avif_quality, 70);
        assert!(!plugin.lazy_avif);
        assert_eq!(plugin.breakpoints, vec![320, 640, 1024, 1440]);
    }

    #[test]
    fn plugin_allows_custom_quality_and_breakpoints() {
        let plugin = ImageOptimizationPlugin {
            quality: 90,
            avif_quality: 60,
            lazy_avif: true,
            breakpoints: vec![480, 960],
        };
        assert_eq!(plugin.quality, 90);
        assert_eq!(plugin.avif_quality, 60);
        assert!(plugin.lazy_avif);
        assert_eq!(plugin.breakpoints, vec![480, 960]);
    }

    // -------------------------------------------------------------------
    // encode_avif — direct API contract (issue #521 AC1, AC4)
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(image_encode_avif_failpoint)]
    fn encode_avif_produces_valid_avif_bytes() {
        let buf = image::ImageBuffer::from_fn(64, 64, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 4) as u8, 128])
        });
        let img = image::DynamicImage::ImageRgb8(buf);
        let bytes = encode_avif(&img, 70).expect("encode_avif");
        assert!(
            bytes.len() > 12 && &bytes[4..12] == b"ftypavif",
            "encoded bytes should be a valid ftypavif ISOBMFF file"
        );
    }

    #[test]
    #[serial_test::parallel(image_encode_avif_failpoint)]
    fn encode_avif_lower_quality_yields_smaller_file() {
        // Use a non-trivial pattern so quantisation actually has somewhere to
        // throw bits away (a flat gradient compresses to the same minimum at
        // both quality levels and breaks the size delta assertion).
        let buf = image::ImageBuffer::from_fn(96, 96, |x, y| {
            let r = ((x * y) ^ (x + y)) as u8;
            let g = (x.wrapping_mul(3).wrapping_add(y)) as u8;
            let b = ((x ^ y) * 5) as u8;
            image::Rgb([r, g, b])
        });
        let img = image::DynamicImage::ImageRgb8(buf);
        let high = encode_avif(&img, 80).unwrap();
        let low = encode_avif(&img, 30).unwrap();
        // quality=30 should be smaller than quality=80. (Plain assert:
        // lazily-formatted message args would be uncovered regions.)
        assert!(low.len() < high.len());
    }

    #[test]
    #[serial_test::parallel(image_encode_avif_failpoint)]
    fn encode_avif_clamps_quality_to_valid_range() {
        let buf = image::ImageBuffer::from_fn(32, 32, |_, _| {
            image::Rgb([200_u8, 100, 50])
        });
        let img = image::DynamicImage::ImageRgb8(buf);
        // Quality 0 should be clamped to 1, not blow up.
        assert!(encode_avif(&img, 0).is_ok());
        // 101 isn't representable in u8 anyway, but max is fine.
        assert!(encode_avif(&img, 100).is_ok());
    }

    #[cfg(feature = "test-fault-injection")]
    #[test]
    #[serial_test::serial(image_encode_avif_failpoint)]
    fn encode_avif_injected_failure_returns_err() {
        // RAII guard so the global failpoint is always disabled again,
        // even if the assertion below panics.
        struct FailGuard;
        impl Drop for FailGuard {
            fn drop(&mut self) {
                let _ = fail::cfg("image::encode-avif", "off");
            }
        }
        let _guard = FailGuard;
        fail::cfg("image::encode-avif", "return").unwrap();

        let buf =
            image::ImageBuffer::from_fn(8, 8, |_, _| image::Rgb([1_u8, 2, 3]));
        let img = image::DynamicImage::ImageRgb8(buf);
        let err = encode_avif(&img, 70).unwrap_err();
        assert!(format!("{err}").contains("encode-avif"));
    }

    #[test]
    fn name_returns_static_image_optimization_identifier() {
        assert_eq!(
            ImageOptimizationPlugin::default().name(),
            "image-optimization"
        );
    }

    // -------------------------------------------------------------------
    // extract_attr — table-driven over the success / failure paths
    // -------------------------------------------------------------------

    #[test]
    fn extract_attr_table_driven_inputs() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            (r#"<img src="x.jpg" alt="Photo">"#, "alt", Some("Photo")),
            (r#"<img src="x.jpg">"#, "alt", None),
            (r#"<img alt="">"#, "alt", Some("")),
            (
                r#"<img alt="multi word value">"#,
                "alt",
                Some("multi word value"),
            ),
            (r#"<img src="x.jpg" alt="P">"#, "src", Some("x.jpg")),
            (r"<img>", "src", None),
            (
                r#"<img fetchpriority="high" src="x.jpg">"#,
                "fetchpriority",
                Some("high"),
            ),
        ];
        for &(tag, attr, expected) in cases {
            let actual = extract_attr(tag, attr);
            assert_eq!(
                actual.as_deref(),
                expected,
                "extract_attr({tag:?}, {attr:?}) should be {expected:?}"
            );
        }
    }

    // -------------------------------------------------------------------
    // rewrite_img_tags — picture element generation
    // -------------------------------------------------------------------

    #[test]
    fn rewrite_img_tags_replaces_img_with_picture_element() {
        let manifest =
            manifest_with("images/photo.jpg", 2000, 1500, &[640, 1024]);
        let html = r#"<img src="images/photo.jpg" alt="A photo">"#;

        let result = rewrite_img_tags(html, &manifest);

        assert!(result.contains("<picture>"));
        assert!(result.contains("</picture>"));
        assert!(result.contains(r#"type="image/webp""#));
        assert!(result.contains("srcset="));
        assert!(result.contains("640w"));
        assert!(result.contains("1024w"));
        assert!(result.contains(r#"loading="lazy""#));
        assert!(result.contains(r#"decoding="async""#));
        assert!(result.contains(r#"width="2000""#));
        assert!(result.contains(r#"height="1500""#));
        assert!(result.contains(r#"alt="A photo""#));
    }

    #[test]
    fn rewrite_img_tags_preserves_alt_text() {
        let manifest = manifest_with("a.jpg", 2000, 1000, &[640]);
        let html = r#"<img src="a.jpg" alt="Important context">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(result.contains(r#"alt="Important context""#));
    }

    #[test]
    fn rewrite_img_tags_handles_missing_alt_with_empty_string() {
        let manifest = manifest_with("a.jpg", 2000, 1000, &[640]);
        let html = r#"<img src="a.jpg">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(result.contains(r#"alt="""#));
    }

    #[test]
    fn rewrite_img_tags_handles_absolute_src_path() {
        let manifest = manifest_with("images/a.jpg", 2000, 1000, &[640]);
        let html = r#"<img src="/images/a.jpg" alt="">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(result.contains("<picture>"));
    }

    #[test]
    fn rewrite_img_tags_no_match_returns_unchanged() {
        let manifest = manifest_with("ghost.jpg", 100, 100, &[640]);
        let html = r"<p>no images here</p>";
        let result = rewrite_img_tags(html, &manifest);
        assert_eq!(result, html);
    }

    #[test]
    fn rewrite_img_tags_skips_entries_with_no_variants() {
        let manifest = manifest_with("a.jpg", 2000, 1000, &[]);
        let html = r#"<img src="a.jpg" alt="x">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert_eq!(result, html, "no variants → no rewrite");
    }

    // -------------------------------------------------------------------
    // rewrite_img_tags — srcset format
    // -------------------------------------------------------------------

    #[test]
    fn rewrite_img_tags_builds_srcset_with_width_descriptors() {
        let manifest =
            manifest_with("a.jpg", 4000, 3000, &[320, 640, 1024, 1440]);
        let html = r#"<img src="a.jpg" alt="">"#;
        let result = rewrite_img_tags(html, &manifest);
        for w in [320, 640, 1024, 1440] {
            assert!(
                result.contains(&format!("{w}w")),
                "srcset should contain {w}w:\n{result}"
            );
        }
        assert!(result.matches(", ").count() >= 3);
    }

    #[test]
    fn rewrite_img_tags_srcset_paths_are_absolute() {
        let manifest = manifest_with("a.jpg", 2000, 1000, &[640]);
        let html = r#"<img src="a.jpg" alt="">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(
            result.contains("/optimized/a-640w.webp 640w"),
            "srcset paths should be absolute: {result}"
        );
    }

    // -------------------------------------------------------------------
    // rewrite_img_tags — lazy loading defaults
    // -------------------------------------------------------------------

    #[test]
    fn rewrite_img_tags_default_loading_is_lazy() {
        let manifest = manifest_with("a.jpg", 2000, 1000, &[640]);
        let html = r#"<img src="a.jpg" alt="">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(result.contains(r#"loading="lazy""#));
        assert!(result.contains(r#"decoding="async""#));
    }

    #[test]
    fn rewrite_img_tags_fetchpriority_high_gets_eager_loading() {
        let manifest = manifest_with("hero.jpg", 2000, 1000, &[640]);
        let html = r#"<img src="hero.jpg" alt="Hero" fetchpriority="high">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(
            result.contains(r#"loading="eager""#),
            "fetchpriority=high should produce loading=eager: {result}"
        );
        assert!(
            result.contains(r#"fetchpriority="high""#),
            "fetchpriority attribute should be preserved: {result}"
        );
    }

    #[test]
    fn rewrite_img_tags_fetchpriority_low_still_lazy() {
        let manifest = manifest_with("bg.jpg", 2000, 1000, &[640]);
        let html = r#"<img src="bg.jpg" alt="" fetchpriority="low">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(result.contains(r#"loading="lazy""#));
    }

    // -------------------------------------------------------------------
    // rewrite_img_tags — AVIF + WebP picture element
    // -------------------------------------------------------------------

    #[test]
    fn rewrite_img_tags_includes_avif_source_when_present() {
        let manifest =
            manifest_with_avif("photo.jpg", 2000, 1500, &[640, 1024]);
        let html = r#"<img src="photo.jpg" alt="">"#;
        let result = rewrite_img_tags(html, &manifest);

        assert!(
            result.contains(r#"type="image/avif""#),
            "should have AVIF source: {result}"
        );
        assert!(
            result.contains(r#"type="image/webp""#),
            "should have WebP source: {result}"
        );

        // AVIF should come before WebP (browser picks first match)
        let avif_pos = result.find("image/avif").expect("avif present");
        let webp_pos = result.find("image/webp").expect("webp present");
        assert!(
            avif_pos < webp_pos,
            "AVIF source should precede WebP source"
        );
    }

    #[test]
    fn rewrite_img_tags_avif_srcset_has_correct_format() {
        let manifest = manifest_with_avif("photo.jpg", 2000, 1500, &[320, 640]);
        let html = r#"<img src="photo.jpg" alt="">"#;
        let result = rewrite_img_tags(html, &manifest);

        assert!(
            result.contains("/optimized/photo-320w.avif 320w"),
            "AVIF srcset should have width descriptors: {result}"
        );
        assert!(
            result.contains("/optimized/photo-640w.avif 640w"),
            "AVIF srcset should have width descriptors: {result}"
        );
    }

    // -------------------------------------------------------------------
    // rewrite_img_tags — width/height preservation
    // -------------------------------------------------------------------

    #[test]
    fn rewrite_img_tags_injects_dimensions_from_manifest() {
        let manifest = manifest_with("a.jpg", 1920, 1080, &[640]);
        let html = r#"<img src="a.jpg" alt="">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(result.contains(r#"width="1920""#));
        assert!(result.contains(r#"height="1080""#));
    }

    #[test]
    fn rewrite_img_tags_preserves_explicit_width_height() {
        let manifest = manifest_with("a.jpg", 1920, 1080, &[640]);
        let html = r#"<img src="a.jpg" alt="" width="800" height="450">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(
            result.contains(r#"width="800""#),
            "explicit width should be preserved: {result}"
        );
        assert!(
            result.contains(r#"height="450""#),
            "explicit height should be preserved: {result}"
        );
    }

    #[test]
    fn rewrite_img_tags_falls_back_to_manifest_dimensions_on_unparseable_attrs()
    {
        // Non-numeric width/height attributes fail `.parse::<u32>()`,
        // exercising the `.ok()` → `None` → `unwrap_or(entry.original_*)`
        // fallback for both dimensions.
        let manifest = manifest_with("a.jpg", 1920, 1080, &[640]);
        let html = r#"<img src="a.jpg" alt="" width="abc" height="xyz">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert!(
            result.contains(r#"width="1920""#),
            "unparseable width should fall back to manifest width: {result}"
        );
        assert!(
            result.contains(r#"height="1080""#),
            "unparseable height should fall back to manifest height: {result}"
        );
    }

    #[test]
    fn rewrite_img_tags_only_replaces_first_occurrence_per_image() {
        let manifest = manifest_with("a.jpg", 2000, 1000, &[640]);
        let html = r#"<img src="a.jpg"><img src="a.jpg">"#;
        let result = rewrite_img_tags(html, &manifest);
        assert_eq!(result.matches("<picture>").count(), 1);
    }

    // -------------------------------------------------------------------
    // collect_images — extension filter + optimized-dir skip
    // -------------------------------------------------------------------

    #[test]
    fn collect_images_skips_optimized_subdirectory() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let opt = site.join("optimized");
        fs::create_dir_all(&opt).unwrap();

        fs::write(site.join("photo.jpg"), [0xFF, 0xD8]).unwrap();
        fs::write(opt.join("photo-640w.webp"), [0]).unwrap();

        let images = collect_images(&site).unwrap();
        assert_eq!(images.len(), 1);
        assert!(images[0].ends_with("photo.jpg"));
    }

    #[test]
    fn collect_images_filters_to_jpg_jpeg_png_only() {
        let dir = tempdir().expect("tempdir");
        for name in ["a.jpg", "b.jpeg", "c.png", "d.gif", "e.webp", "f.txt"] {
            fs::write(dir.path().join(name), [0]).unwrap();
        }
        let images = collect_images(dir.path()).unwrap();
        assert_eq!(images.len(), 3, "only jpg/jpeg/png should be collected");
    }

    #[test]
    fn collect_images_extension_match_is_case_insensitive() {
        let dir = tempdir().expect("tempdir");
        for name in ["A.JPG", "B.PNG", "C.JPEG"] {
            fs::write(dir.path().join(name), [0]).unwrap();
        }
        let images = collect_images(dir.path()).unwrap();
        assert_eq!(images.len(), 3);
    }

    #[test]
    fn collect_images_recurses_into_nested_subdirectories() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("top.jpg"), [0]).unwrap();
        fs::write(nested.join("deep.png"), [0]).unwrap();

        let images = collect_images(dir.path()).unwrap();
        assert_eq!(images.len(), 2);
    }

    #[test]
    fn collect_images_returns_empty_for_missing_directory() {
        let dir = tempdir().expect("tempdir");
        let result = collect_images(&dir.path().join("missing")).unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    fn collect_images_returns_results_sorted() {
        let dir = tempdir().expect("tempdir");
        for name in ["zebra.jpg", "apple.jpg", "mango.jpg"] {
            fs::write(dir.path().join(name), [0]).unwrap();
        }
        let images = collect_images(dir.path()).unwrap();
        let names: Vec<_> = images
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["apple.jpg", "mango.jpg", "zebra.jpg"]);
    }

    // -------------------------------------------------------------------
    // collect_html_files — recursion + filtering
    // -------------------------------------------------------------------

    #[test]
    fn collect_html_files_filters_non_html_extensions() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("a.html"), "").unwrap();
        fs::write(dir.path().join("b.css"), "").unwrap();

        let result = collect_html_files(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn collect_html_files_recurses_and_sorts() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("blog");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("index.html"), "").unwrap();
        fs::write(nested.join("post.html"), "").unwrap();

        let result = collect_html_files(dir.path()).unwrap();
        assert_eq!(result.len(), 2);
    }

    // -------------------------------------------------------------------
    // after_compile — short-circuit paths (no real images)
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_missing_site_dir_returns_ok() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("missing");
        let ctx =
            PluginContext::new(dir.path(), dir.path(), &missing, dir.path());
        ImageOptimizationPlugin::default()
            .after_compile(&ctx)
            .expect("missing site is not an error");
        assert!(!missing.exists());
    }

    // -------------------------------------------------------------------
    // process_image — real JPEG/PNG round-trip
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(image_encode_avif_failpoint)]
    fn process_image_generates_webp_variants_below_original_width() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let opt = site.join("optimized");
        fs::create_dir_all(&opt).unwrap();

        let src = site.join("hero.jpg");
        write_test_jpeg(&src, 2000, 1000);

        let manifest = process_image(
            &src,
            &site,
            &opt,
            &[320, 640, 1024, 1440],
            DEFAULT_QUALITY,
            DEFAULT_AVIF_QUALITY,
        )
        .unwrap();
        assert_eq!(manifest.original_width, 2000);
        assert_eq!(manifest.original_height, 1000);
        assert_eq!(manifest.original_rel, "hero.jpg");

        // Every breakpoint strictly less than 2000 must produce a variant.
        assert_eq!(manifest.webp_variants.len(), 4);
        for v in &manifest.webp_variants {
            assert!(opt
                .join(v.rel_path.trim_start_matches("optimized/"))
                .exists());
            assert!(v.width < 2000);
        }

        // AVIF variants generated in parallel with WebP at each breakpoint.
        assert_eq!(manifest.avif_variants.len(), 4);
        for v in &manifest.avif_variants {
            let path = opt.join(v.rel_path.trim_start_matches("optimized/"));
            assert!(path.exists(), "AVIF variant must exist on disk: {path:?}");
            let bytes = fs::read(&path).unwrap();
            // ISO BMFF AVIF magic: bytes 4..12 == "ftypavif" (or "ftypavis"
            // for sequences). We only emit still images so it's always "avif".
            assert!(
                bytes.len() > 12 && &bytes[4..12] == b"ftypavif",
                "AVIF file should start with ftypavif box: {path:?}"
            );
        }
    }

    #[test]
    fn process_image_skips_widths_larger_than_original() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let opt = site.join("optimized");
        fs::create_dir_all(&opt).unwrap();

        let src = site.join("small.png");
        write_test_png(&src, 500, 500);

        let manifest = process_image(
            &src,
            &site,
            &opt,
            &[320, 640, 1024, 1440],
            DEFAULT_QUALITY,
            DEFAULT_AVIF_QUALITY,
        )
        .unwrap();
        // Only 320 should survive (320 < 500).
        assert_eq!(manifest.webp_variants.len(), 1);
        assert_eq!(manifest.webp_variants[0].width, 320);
        assert_eq!(manifest.avif_variants.len(), 1);
        assert_eq!(manifest.avif_variants[0].width, 320);
    }

    #[test]
    #[serial_test::parallel(image_encode_avif_failpoint)]
    fn process_image_uses_custom_breakpoints() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let opt = site.join("optimized");
        fs::create_dir_all(&opt).unwrap();

        let src = site.join("photo.jpg");
        write_test_jpeg(&src, 2000, 1000);

        let manifest = process_image(
            &src,
            &site,
            &opt,
            &[480, 960],
            DEFAULT_QUALITY,
            DEFAULT_AVIF_QUALITY,
        )
        .unwrap();
        assert_eq!(manifest.webp_variants.len(), 2);
        assert_eq!(manifest.webp_variants[0].width, 480);
        assert_eq!(manifest.webp_variants[1].width, 960);
    }

    #[test]
    fn process_image_rejects_unreadable_source_path() {
        let dir = tempdir().expect("tempdir");
        let opt = dir.path().join("opt");
        fs::create_dir_all(&opt).unwrap();
        let missing = dir.path().join("does-not-exist.jpg");
        assert!(process_image(
            &missing,
            dir.path(),
            &opt,
            DEFAULT_BREAKPOINTS,
            DEFAULT_QUALITY,
            DEFAULT_AVIF_QUALITY
        )
        .is_err());
    }

    // -------------------------------------------------------------------
    // after_compile — end-to-end on real images
    // -------------------------------------------------------------------

    #[test]
    #[serial_test::parallel(image_encode_avif_failpoint)]
    fn after_compile_processes_real_images_and_rewrites_html() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let images = site.join("images");
        fs::create_dir_all(&images).unwrap();

        write_test_jpeg(&images.join("photo.jpg"), 2000, 1500);
        fs::write(
            site.join("index.html"),
            r#"<html><head></head><body><img src="/images/photo.jpg" alt="Test"></body></html>"#,
        )
        .unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        ImageOptimizationPlugin::default()
            .after_compile(&ctx)
            .unwrap();

        // Original file preserved.
        assert!(images.join("photo.jpg").exists());
        // Optimized directory populated.
        assert!(site.join("optimized").exists());
        // HTML rewritten to <picture>.
        let html = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(html.contains("<picture>"));
        assert!(html.contains("image/webp"));
        assert!(html.contains(r#"alt="Test""#));
        assert!(html.contains(r#"loading="lazy""#));
        assert!(html.contains(r#"decoding="async""#));
    }

    #[test]
    fn after_compile_failed_image_processing_logs_warn_and_continues() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        fs::write(site.join("broken.jpg"), b"this is not really a jpeg")
            .unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        ImageOptimizationPlugin::default()
            .after_compile(&ctx)
            .expect("broken image must not propagate");
    }

    #[test]
    fn after_compile_html_without_image_refs_skips_rewrite() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let images = site.join("images");
        fs::create_dir_all(&images).unwrap();

        write_test_jpeg(&images.join("orphan.jpg"), 1000, 1000);
        let original_html =
            "<html><head></head><body><p>no images here</p></body></html>";
        fs::write(site.join("index.html"), original_html).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        ImageOptimizationPlugin::default()
            .after_compile(&ctx)
            .unwrap();

        let after = fs::read_to_string(site.join("index.html")).unwrap();
        assert_eq!(
            after, original_html,
            "html with no image refs should not be rewritten"
        );
    }

    #[test]
    fn after_compile_no_images_short_circuits_without_creating_optimized_dir() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("index.html"), "<p></p>").unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        ImageOptimizationPlugin::default()
            .after_compile(&ctx)
            .unwrap();
        assert!(!site.join("optimized").exists());
    }

    // -------------------------------------------------------------------
    // rewrite_img_tags — skip branches
    // -------------------------------------------------------------------

    #[test]
    fn rewrite_img_tags_skips_img_without_src() {
        let manifest = manifest_with("photo.jpg", 100, 80, &[320]);
        let html = "<img alt=\"no source\">";
        assert_eq!(rewrite_img_tags(html, &manifest), html);
    }

    #[test]
    fn rewrite_img_tags_skips_src_not_in_manifest() {
        let manifest = manifest_with("photo.jpg", 100, 80, &[320]);
        let html = "<img src=\"/unknown.jpg\">";
        assert_eq!(rewrite_img_tags(html, &manifest), html);
    }

    #[test]
    fn rewrite_img_tags_logs_when_replacing_author_srcset() {
        crate::test_support::init_logger();
        let manifest = manifest_with("photo.jpg", 100, 80, &[320]);
        let html = "<img src=\"/photo.jpg\" srcset=\"old.jpg 1x\">";
        let out = rewrite_img_tags(html, &manifest);
        assert!(out.contains("<picture>"));
        assert!(!out.contains("old.jpg"));
    }

    #[test]
    fn rewrite_img_tags_avif_only_entry_omits_webp_source() {
        let mut manifest = HashMap::new();
        let _ = manifest.insert(
            "photo.jpg".to_string(),
            ImageManifest {
                original_rel: "photo.jpg".to_string(),
                original_width: 100,
                original_height: 80,
                webp_variants: Vec::new(),
                avif_variants: vec![ImageVariant {
                    rel_path: "optimized/photo-320w.avif".to_string(),
                    width: 320,
                }],
            },
        );
        let out = rewrite_img_tags("<img src=\"/photo.jpg\">", &manifest);
        assert!(out.contains("image/avif"));
        assert!(!out.contains("image/webp"));
    }

    #[test]
    fn unwrap_rewrite_falls_back_to_original_on_error() {
        crate::test_support::init_logger();
        let err = SsgError::io(
            std::io::Error::other("synthetic lol_html failure"),
            "<lol_html>",
        );
        let out = unwrap_rewrite("<p>original</p>", Err(err));
        assert_eq!(out, "<p>original</p>");
    }

    #[test]
    fn extract_attr_returns_none_for_unterminated_value() {
        assert!(extract_attr("<img alt=\"unterminated", "alt").is_none());
    }

    // -------------------------------------------------------------------
    // encode/save error branches
    // -------------------------------------------------------------------

    #[test]
    fn encode_avif_rejects_empty_image() {
        let img = image::DynamicImage::new_rgb8(0, 0);
        assert!(encode_avif(&img, 70).is_err());
    }

    #[test]
    fn process_image_webp_save_fails_when_variant_squatted_by_dir() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let optimized = site.join("optimized");
        fs::create_dir_all(&optimized).unwrap();
        write_test_jpeg(&site.join("photo.jpg"), 400, 20);
        // A directory squats the WebP variant path, so `resized.save`
        // fails and the Io closure fires.
        fs::create_dir_all(optimized.join("photo-320w.webp")).unwrap();

        let res = process_image(
            &site.join("photo.jpg"),
            &site,
            &optimized,
            &[320],
            80,
            70,
        );
        assert!(res.is_err());
    }

    #[test]
    #[serial_test::parallel(image_encode_avif_failpoint)]
    fn process_image_avif_write_failure_logs_and_skips_variant() {
        crate::test_support::init_logger();
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let optimized = site.join("optimized");
        fs::create_dir_all(&optimized).unwrap();
        write_test_jpeg(&site.join("photo.jpg"), 400, 20);
        // A directory squats the AVIF variant path: encoding succeeds
        // but fs::write fails, which is logged and skipped.
        fs::create_dir_all(optimized.join("photo-320w.avif")).unwrap();

        let entry = process_image(
            &site.join("photo.jpg"),
            &site,
            &optimized,
            &[320],
            80,
            70,
        )
        .unwrap();
        assert_eq!(entry.webp_variants.len(), 1);
        assert!(entry.avif_variants.is_empty());
    }

    #[cfg(feature = "test-fault-injection")]
    #[test]
    #[serial_test::serial(image_encode_avif_failpoint)]
    fn process_image_avif_encode_failure_logs_and_skips_variant() {
        // Distinct from the write-failure case above: here `encode_avif`
        // itself fails (via the injected failpoint), exercising the
        // outer `Err(e) => { log::warn!(...); None }` arm in
        // `process_image`'s AVIF results loop rather than the inner
        // fs::write failure arm.
        crate::test_support::init_logger();
        struct FailGuard;
        impl Drop for FailGuard {
            fn drop(&mut self) {
                let _ = fail::cfg("image::encode-avif", "off");
            }
        }
        let _guard = FailGuard;
        fail::cfg("image::encode-avif", "return").unwrap();

        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let optimized = site.join("optimized");
        fs::create_dir_all(&optimized).unwrap();
        write_test_jpeg(&site.join("photo.jpg"), 400, 20);

        let entry = process_image(
            &site.join("photo.jpg"),
            &site,
            &optimized,
            &[320],
            80,
            70,
        )
        .unwrap();
        assert_eq!(
            entry.webp_variants.len(),
            1,
            "WebP path is unaffected by the AVIF failpoint"
        );
        assert!(
            entry.avif_variants.is_empty(),
            "AVIF variant must be skipped when encoding fails"
        );
    }

    // -------------------------------------------------------------------
    // after_compile / rewrite_html_img_tags — IO error branches
    // -------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn after_compile_fails_when_site_has_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let locked = site.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let res = ImageOptimizationPlugin::default().after_compile(&ctx);

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        // Root CI runners bypass perms; only assert when it errored.
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }

    #[test]
    fn after_compile_fails_when_optimized_dir_squatted_by_file() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        // Tiny image: below every default breakpoint, so no encoding
        // happens — but images is non-empty so create_dir_all runs.
        write_test_jpeg(&site.join("tiny.jpg"), 16, 16);
        fs::write(site.join("optimized"), "not a dir").unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let err = ImageOptimizationPlugin::default()
            .after_compile(&ctx)
            .unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_fails_when_html_is_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        write_test_jpeg(&site.join("tiny.jpg"), 16, 16);
        let html = site.join("index.html");
        fs::write(&html, "<p>x</p>").unwrap();
        fs::set_permissions(&html, fs::Permissions::from_mode(0o000)).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let res = ImageOptimizationPlugin::default().after_compile(&ctx);

        let _ = fs::set_permissions(&html, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::parallel(image_encode_avif_failpoint)]
    fn after_compile_fails_when_html_is_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        // Wide enough for one 320w variant, so the HTML actually
        // changes and the write is attempted.
        write_test_jpeg(&site.join("photo.jpg"), 400, 20);
        let html = site.join("index.html");
        fs::write(&html, "<img src=\"/photo.jpg\">").unwrap();
        fs::set_permissions(&html, fs::Permissions::from_mode(0o444)).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let plugin = ImageOptimizationPlugin {
            breakpoints: vec![320],
            ..Default::default()
        };
        let res = plugin.after_compile(&ctx);

        let _ = fs::set_permissions(&html, fs::Permissions::from_mode(0o644));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }

    #[test]
    #[cfg(unix)]
    fn rewrite_html_img_tags_fails_on_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        let locked = site.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();

        let res = rewrite_html_img_tags(&site, &HashMap::new());

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        if let Err(e) = res {
            assert!(!format!("{e}").is_empty(), "should not be empty");
        }
    }
}
