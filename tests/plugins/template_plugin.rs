// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::plugins::TemplatePlugin` (feature-gated `templates`).

#![allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(feature = "templates")]
mod gated {
    use ssg::plugin::Plugin;
    use ssg::template_engine::TemplateConfig;
    use ssg::template_plugin::TemplatePlugin;
    use tempfile::tempdir;

    #[test]
    fn template_plugin_constructs_with_default_config() {
        let dir = tempdir().unwrap();
        let cfg = TemplateConfig {
            template_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let p = TemplatePlugin::new(cfg);
        assert!(!p.name().is_empty(), "should not be empty");
    }
}

#[test]
fn module_compiles() {
    let _ = std::any::type_name::<()>();
}
