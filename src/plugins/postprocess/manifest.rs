// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Manifest fix plugin.

use super::helpers::{read_meta_sidecars, truncate_at_word};
use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use std::fs;

/// Fixes manifest.json description truncation by using full text or
/// word-boundary-safe truncation at 200 characters.
#[derive(Debug, Clone, Copy)]
pub struct ManifestFixPlugin;

impl Plugin for ManifestFixPlugin {
    fn name(&self) -> &'static str {
        "manifest-fix"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        let manifest_path = ctx.site_dir.join("manifest.json");
        if !manifest_path.exists() {
            return Ok(());
        }

        let content =
            fs::read_to_string(&manifest_path).with_path(&manifest_path)?;

        if content.trim().is_empty() {
            let meta_entries =
                read_meta_sidecars(&ctx.site_dir).unwrap_or_default();
            let mut manifest = serde_json::json!({
                "name": "Static Site",
                "short_name": "Site",
                "start_url": "/",
                "display": "standalone"
            });
            if let Some(desc) = find_full_description(&meta_entries) {
                let truncated = truncate_at_word(&desc, 200);
                manifest["description"] = serde_json::Value::String(truncated);
            }
            let output = serialize_manifest(&manifest)
                .map_err(|e| SsgError::io(e, &manifest_path))?;
            fs::write(&manifest_path, output).with_path(&manifest_path)?;
            log::info!("[manifest-fix] Generated manifest.json from metadata");
            return Ok(());
        }

        let mut manifest: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| SsgError::io(e, &manifest_path))?;

        let meta_entries =
            read_meta_sidecars(&ctx.site_dir).unwrap_or_default();

        let full_description = find_full_description(&meta_entries);

        if let Some(desc) = full_description {
            let truncated = truncate_at_word(&desc, 200);
            manifest["description"] = serde_json::Value::String(truncated);
        } else if let Some(current) =
            manifest.get("description").and_then(|v| v.as_str())
        {
            if let Some(fixed) = fix_truncated_description(current) {
                manifest["description"] = serde_json::Value::String(fixed);
            }
        }

        // Drop icon entries whose `src` is empty; Chrome logs
        // "Error while trying to use the following icon from the Manifest"
        // when it tries to fetch them.
        drop_empty_icons(&mut manifest);
        set_icon_types(&mut manifest);

        let output = serialize_manifest(&manifest)
            .map_err(|e| SsgError::io(e, &manifest_path))?;
        fs::write(&manifest_path, output).with_path(&manifest_path)?;

        log::info!("[manifest-fix] Fixed manifest.json description");
        Ok(())
    }
}

/// Finds the full description from meta sidecars, preferring the root page.
fn find_full_description(
    meta_entries: &[(String, std::collections::HashMap<String, String>)],
) -> Option<String> {
    meta_entries
        .iter()
        .find(|(rel, _)| rel.is_empty() || rel == ".")
        .and_then(|(_, meta)| meta.get("description"))
        .or_else(|| {
            meta_entries
                .iter()
                .find_map(|(_, meta)| meta.get("description"))
        })
        .cloned()
}

/// Removes any entry from the manifest's `icons` array whose `src` is
/// missing or empty. Chrome logs a manifest icon download error for each
/// such entry, even though the manifest itself is otherwise valid.
fn drop_empty_icons(manifest: &mut serde_json::Value) {
    let Some(icons) = manifest.get_mut("icons").and_then(|v| v.as_array_mut())
    else {
        return;
    };
    icons.retain(|icon| {
        icon.get("src")
            .and_then(|s| s.as_str())
            .is_some_and(|s| !s.is_empty())
    });
    if icons.is_empty() {
        // An empty array is preferable to `[{src:""}]` — but if there are
        // truly no usable icons, drop the key entirely so the manifest
        // doesn't advertise an empty icon set. (`get_mut("icons")`
        // succeeding above guarantees this is an object.)
        let _ = manifest.as_object_mut().and_then(|map| map.remove("icons"));
    }
}

/// Sets each icon's `type` from its file extension.
///
/// staticdatagen's generator declares every icon as `image/svg+xml`, so an
/// `icon` front-matter value naming a PNG produced a manifest that told the
/// browser the PNG was an SVG. The extension of `src` (query and fragment
/// stripped, case ignored) decides the type when it names an image format;
/// otherwise the declared type is the only information and is kept.
fn set_icon_types(manifest: &mut serde_json::Value) {
    let Some(icons) = manifest.get_mut("icons").and_then(|v| v.as_array_mut())
    else {
        return;
    };
    for icon in icons {
        let Some(mime) = icon
            .get("src")
            .and_then(|s| s.as_str())
            .and_then(image_type_for)
        else {
            continue;
        };
        if let Some(entry) = icon.as_object_mut() {
            let _ = entry.insert("type".into(), mime.into());
        }
    }
}

/// The image MIME type a URL's file extension names, if any.
fn image_type_for(src: &str) -> Option<&'static str> {
    let path = src.split(['?', '#']).next().unwrap_or(src);
    let file = path.rsplit('/').next().unwrap_or(path);
    let (_, ext) = file.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "svg" => Some("image/svg+xml"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}

/// Serialize the manifest with a fault-injection hook so tests can
/// drive the error branch (pretty-printing a `Value` cannot fail in
/// practice).
fn serialize_manifest(
    manifest: &serde_json::Value,
) -> serde_json::Result<String> {
    fail_point!("postprocess::manifest-serialize", |_| Err(
        <serde_json::Error as serde::ser::Error>::custom(
            "injected: postprocess::manifest-serialize"
        )
    ));
    serde_json::to_string_pretty(manifest)
}

/// Fixes a truncated description by ensuring it ends at a word boundary.
/// Returns `None` if the description already ends with proper punctuation.
fn fix_truncated_description(current: &str) -> Option<String> {
    if current.ends_with('.')
        || current.ends_with('!')
        || current.ends_with('?')
        || current.ends_with("...")
    {
        return None;
    }
    Some(if let Some(last_space) = current.rfind(' ') {
        format!("{}...", &current[..last_space])
    } else {
        format!("{current}...")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginContext;
    use anyhow::Result;
    use std::path::Path;
    use tempfile::tempdir;

    fn test_ctx(site_dir: &Path) -> PluginContext {
        crate::test_support::init_logger();
        PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            site_dir,
            Path::new("templates"),
        )
    }

    #[test]
    fn test_drop_empty_icons_removes_empty_src() {
        let mut m: serde_json::Value = serde_json::from_str(
            r#"{"icons":[{"src":"","sizes":"512x512"},{"src":"/icon.svg","sizes":"512x512"}]}"#,
        )
        .unwrap();
        drop_empty_icons(&mut m);
        let icons = m["icons"].as_array().unwrap();
        assert_eq!(icons.len(), 1);
        assert_eq!(icons[0]["src"], "/icon.svg");
    }

    #[test]
    fn test_drop_empty_icons_removes_key_when_all_empty() {
        let mut m: serde_json::Value =
            serde_json::from_str(r#"{"name":"x","icons":[{"src":""}]}"#)
                .unwrap();
        drop_empty_icons(&mut m);
        assert!(m.get("icons").is_none(), "icons key should be dropped");
    }

    #[test]
    fn test_set_icon_types_follows_the_file_extension() {
        let mut m: serde_json::Value = serde_json::from_str(
            r#"{"icons":[
                {"src":"https://cdn.example/icons/512x512.png","type":"image/svg+xml"},
                {"src":"/icon.svg","type":"image/svg+xml"},
                {"src":"/icon.WEBP?v=2#x"},
                {"src":"/favicon.ico","type":"image/x-icon"},
                {"src":"/photo.jpeg","type":"image/png"},
                {"src":"/icon","type":"image/svg+xml"},
                {"src":"/icon.bin","type":"application/x-custom"}
            ]}"#,
        )
        .unwrap();
        set_icon_types(&mut m);
        let types: Vec<Option<&str>> = m["icons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.get("type").and_then(|t| t.as_str()))
            .collect();
        assert_eq!(
            types,
            [
                Some("image/png"),
                Some("image/svg+xml"),
                Some("image/webp"),
                Some("image/x-icon"),
                Some("image/jpeg"),
                // No extension, or one that is not an image format: the
                // declared type is the only information, so it stays.
                Some("image/svg+xml"),
                Some("application/x-custom"),
            ]
        );
    }

    #[test]
    fn test_set_icon_types_without_icons_is_a_no_op() {
        let mut m: serde_json::Value =
            serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        set_icon_types(&mut m);
        assert_eq!(m, serde_json::json!({"name":"x"}));
    }

    #[test]
    #[serial_test::parallel]
    fn test_manifest_fix_corrects_a_png_icon_declared_as_svg() -> Result<()> {
        // staticdatagen's manifest generator declares every icon as
        // image/svg+xml, whatever the `icon` front matter points at.
        let tmp = tempdir().unwrap();
        let manifest_path = tmp.path().join("manifest.json");
        fs::write(
            &manifest_path,
            r#"{"name":"Test","icons":[{"purpose":"any maskable","sizes":"512x512","src":"https://cdn.example/icons/512x512.png","type":"image/svg+xml"}]}"#,
        )
        .unwrap();
        let ctx = test_ctx(tmp.path());
        ManifestFixPlugin.after_compile(&ctx).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap())
                .unwrap();
        assert_eq!(manifest["icons"][0]["type"], "image/png");
        assert_eq!(manifest["icons"][0]["sizes"], "512x512");
        Ok(())
    }

    #[test]
    fn name_is_stable() {
        assert_eq!(ManifestFixPlugin.name(), "manifest-fix");
    }

    #[test]
    #[serial_test::parallel]
    fn after_compile_no_op_when_manifest_missing() -> Result<()> {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        ManifestFixPlugin.after_compile(&ctx).unwrap();
        assert!(!tmp.path().join("manifest.json").exists());
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn after_compile_returns_error_on_invalid_json() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("manifest.json"), "not valid json").unwrap();
        let ctx = test_ctx(tmp.path());
        let err = ManifestFixPlugin.after_compile(&ctx).unwrap_err();
        assert!(
            err.to_string().contains("invalid JSON")
                || err.to_string().contains("manifest"),
            "expected JSON parse error, got: {err}"
        );
    }

    #[test]
    fn drop_empty_icons_keeps_array_with_real_entries() {
        let mut m: serde_json::Value = serde_json::from_str(
            r#"{"icons":[{"src":"/a.svg"},{"src":"/b.svg"}]}"#,
        )
        .unwrap();
        drop_empty_icons(&mut m);
        let icons = m["icons"].as_array().unwrap();
        assert_eq!(icons.len(), 2);
    }

    #[test]
    fn drop_empty_icons_no_op_when_no_icons_key() {
        let mut m: serde_json::Value =
            serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        drop_empty_icons(&mut m);
        assert!(m.get("icons").is_none());
        assert_eq!(m["name"], "x");
    }

    #[test]
    fn drop_empty_icons_no_op_when_icons_not_array() {
        // Defensive: malformed manifest with non-array icons.
        let mut m: serde_json::Value =
            serde_json::from_str(r#"{"icons":"not an array"}"#).unwrap();
        drop_empty_icons(&mut m);
        assert_eq!(m["icons"], "not an array");
    }

    #[test]
    fn fix_truncated_description_returns_none_when_already_terminated() {
        assert!(fix_truncated_description("ends with period.").is_none());
        assert!(fix_truncated_description("ends with bang!").is_none());
        assert!(fix_truncated_description("ends with question?").is_none());
        assert!(fix_truncated_description("ends with ellipsis...").is_none());
    }

    #[test]
    fn fix_truncated_description_truncates_at_word_boundary() {
        let out =
            fix_truncated_description("a long description without ending");
        assert_eq!(out.as_deref(), Some("a long description without..."));
    }

    #[test]
    fn fix_truncated_description_no_space_appends_ellipsis() {
        // Edge case: a single very long word without spaces.
        let out = fix_truncated_description("supercalifragilistic");
        assert_eq!(out.as_deref(), Some("supercalifragilistic..."));
    }

    #[test]
    #[serial_test::parallel]
    fn after_compile_drops_empty_icons_in_manifest() -> Result<()> {
        let tmp = tempdir().unwrap();
        let manifest_path = tmp.path().join("manifest.json");
        fs::write(
            &manifest_path,
            r#"{"name":"X","description":"Already terminated.","icons":[{"src":""}]}"#,
        ).unwrap();
        let ctx = test_ctx(tmp.path());
        ManifestFixPlugin.after_compile(&ctx).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap())
                .unwrap();
        assert!(after.get("icons").is_none(), "empty icon should be dropped");
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn test_manifest_fix_repairs_truncated_description() -> Result<()> {
        let tmp = tempdir().unwrap();
        let manifest_path = tmp.path().join("manifest.json");
        fs::write(
            &manifest_path,
            r#"{"name":"Test","description":"A new paper suggests Shor's algorithm could run on as few as 10,000 qubits. The threshold for cryptographically relevant"}"#,
        ).unwrap();

        let ctx = test_ctx(tmp.path());
        ManifestFixPlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&manifest_path).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&result).unwrap();
        let desc = manifest["description"].as_str().unwrap();
        // Non-short-circuiting `|` so every operand is evaluated.
        let clean =
            desc.ends_with("...") | desc.ends_with('.') | desc.ends_with('!');
        assert!(clean, "Description should end cleanly, got: {desc}");
        Ok(())
    }

    #[test]
    #[serial_test::parallel]
    fn test_manifest_fix_uses_sidecar_description() -> Result<()> {
        let tmp = tempdir().unwrap();
        let manifest_path = tmp.path().join("manifest.json");
        fs::write(
            &manifest_path,
            r#"{"name":"Test","description":"Short description"}"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("index.meta.json"),
            r#"{"description":"This is a very long description that we are using to test manifest metadata sidecar description truncation logic in the manifest fix plugin. We need to make sure that the total length of this text exceeds two hundred characters so that the truncation is triggered."}"#,
        ).unwrap();

        let ctx = test_ctx(tmp.path());
        ManifestFixPlugin.after_compile(&ctx).unwrap();

        let result = fs::read_to_string(&manifest_path).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&result).unwrap();
        let desc = manifest["description"].as_str().unwrap();
        assert!(desc.starts_with("This is a very long"));
        assert!(desc.ends_with("..."));
        Ok(())
    }

    #[test]
    fn test_find_full_description_fallback() {
        let mut entries = Vec::new();
        let mut meta1 = std::collections::HashMap::new();
        let _ = meta1.insert("title".to_string(), "No description".to_string());
        entries.push(("root".to_string(), meta1));

        let mut meta2 = std::collections::HashMap::new();
        let _ = meta2
            .insert("description".to_string(), "Fallback desc".to_string());
        entries.push(("subpage".to_string(), meta2));

        let desc = find_full_description(&entries);
        assert_eq!(desc.as_deref(), Some("Fallback desc"));
    }

    #[test]
    fn test_find_full_description_none() {
        let mut entries = Vec::new();
        let mut meta1 = std::collections::HashMap::new();
        let _ = meta1.insert("title".to_string(), "No description".to_string());
        entries.push(("root".to_string(), meta1));

        let desc = find_full_description(&entries);
        assert!(desc.is_none());
    }

    // -----------------------------------------------------------------
    // after_compile: manifest without a description key
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel]
    fn after_compile_handles_manifest_without_description() {
        let tmp = tempdir().unwrap();
        let manifest_path = tmp.path().join("manifest.json");
        fs::write(&manifest_path, r#"{"name":"X"}"#).unwrap();
        let ctx = test_ctx(tmp.path());
        ManifestFixPlugin.after_compile(&ctx).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap())
                .unwrap();
        assert!(
            after.get("description").is_none(),
            "no description key must be invented"
        );
    }

    // -----------------------------------------------------------------
    // Error paths
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::parallel]
    fn after_compile_errors_on_invalid_utf8_manifest() {
        let tmp = tempdir().unwrap();
        let manifest_path = tmp.path().join("manifest.json");
        fs::write(&manifest_path, [0xFF, 0xFE, 0xFD]).unwrap();
        let ctx = test_ctx(tmp.path());
        let err = ManifestFixPlugin.after_compile(&ctx).unwrap_err();
        assert!(format!("{err}").contains("manifest.json"));
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_write_failure_on_readonly_manifest() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let manifest_path = tmp.path().join("manifest.json");
        fs::write(&manifest_path, r#"{"name":"X","description":"Done."}"#)
            .unwrap();
        fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o444))
            .unwrap();

        let ctx = test_ctx(tmp.path());
        let result = ManifestFixPlugin.after_compile(&ctx);
        let _ = fs::set_permissions(
            &manifest_path,
            fs::Permissions::from_mode(0o644),
        );
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("manifest.json"));
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::plugin::PluginContext;
    use serial_test::serial;
    use std::path::Path;
    use tempfile::tempdir;

    /// RAII guard that disables a failpoint on drop.
    struct FailGuard(&'static str);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            let _ = fail::cfg(self.0, "off");
        }
    }

    #[test]
    #[serial]
    fn after_compile_maps_serialize_failure_to_io_error() {
        let _guard = FailGuard("postprocess::manifest-serialize");
        fail::cfg("postprocess::manifest-serialize", "return")
            .expect("activate failpoint");

        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("manifest.json"),
            r#"{"name":"X","description":"Already terminated."}"#,
        )
        .unwrap();
        crate::test_support::init_logger();
        let ctx = PluginContext::new(
            Path::new("content"),
            Path::new("build"),
            tmp.path(),
            Path::new("templates"),
        );
        let err = ManifestFixPlugin
            .after_compile(&ctx)
            .expect_err("injected serialize failure must propagate");
        let msg = format!("{err}");
        assert!(msg.contains("manifest.json"), "got: {msg}");
        assert!(
            msg.contains("injected: postprocess::manifest-serialize"),
            "got: {msg}"
        );
    }
}
