// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::image_plugin` (feature-gated `image-optimization`).

#[cfg(feature = "image-optimization")]
mod gated {
    use ssg::image_plugin::ImageOptimizationPlugin;
    use ssg::plugin::{Plugin, PluginContext};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn image_plugin_default_constructs() {
        let p = ImageOptimizationPlugin::default();
        assert!(!p.name().is_empty(), "should not be empty");
        assert_eq!(p.avif_quality, 70);
        assert!(!p.lazy_avif);
    }

    /// End-to-end AVIF emission test — issue #521 AC1, AC2, AC6.
    ///
    /// Drops a real JPEG into a fake `site_dir`, runs `after_compile`,
    /// then asserts:
    /// 1. AVIF files exist on disk for every breakpoint < original width
    ///    (AC1)
    /// 2. The rewritten `<picture>` element emits `<source type="image/avif">`
    ///    BEFORE `<source type="image/webp">` (AC2)
    /// 3. The WebP `<source>` and srcset widths are still present
    ///    (AC6 — no regression)
    #[test]
    #[allow(clippy::unwrap_used)]
    fn after_compile_emits_avif_source_before_webp() {
        let dir = tempdir().expect("tempdir");
        let site = dir.path().join("site");
        let imgs = site.join("images");
        fs::create_dir_all(&imgs).unwrap();

        // 2000x1500 hero → all four breakpoints (320, 640, 1024, 1440)
        // are < original width, so we expect four AVIF + four WebP files.
        let buf = image::ImageBuffer::from_fn(2000, 1500, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        image::DynamicImage::ImageRgb8(buf)
            .save_with_format(imgs.join("hero.jpg"), image::ImageFormat::Jpeg)
            .expect("write hero.jpg");

        fs::write(
            site.join("index.html"),
            r#"<html><head></head><body><img src="/images/hero.jpg" alt="Hero"></body></html>"#,
        )
        .unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        ImageOptimizationPlugin::default()
            .after_compile(&ctx)
            .expect("image plugin after_compile");

        // ---- AC1: AVIF files materialised on disk -----------------
        let opt = site.join("optimized");
        for w in [320, 640, 1024, 1440] {
            let p = opt.join(format!("hero-{w}w.avif"));
            assert!(p.exists(), "missing AVIF variant {}", p.display());
            let bytes = fs::read(&p).unwrap();
            assert!(
                bytes.len() > 12 && &bytes[4..12] == b"ftypavif",
                "AVIF {} should start with ftypavif box",
                p.display()
            );
        }

        // ---- AC2 + AC6: HTML rewritten with AVIF before WebP -------
        let html = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(html.contains("<picture>"), "expected <picture> wrap");
        let avif_pos = html
            .find("type=\"image/avif\"")
            .expect("AVIF source must be present");
        let webp_pos = html
            .find("type=\"image/webp\"")
            .expect("WebP source must still be present (AC6)");
        assert!(
            avif_pos < webp_pos,
            "AVIF <source> must precede WebP <source>; got avif@{avif_pos} webp@{webp_pos}"
        );

        // WebP srcset widths preserved (AC6 — no regression in responsive variants).
        for w in [320, 640, 1024, 1440] {
            assert!(
                html.contains(&format!("hero-{w}w.webp")),
                "WebP variant {w}w missing from srcset"
            );
            assert!(
                html.contains(&format!("hero-{w}w.avif")),
                "AVIF variant {w}w missing from srcset"
            );
        }
    }
}

#[test]
fn module_compiles() {
    let _ = std::any::type_name::<()>();
}
