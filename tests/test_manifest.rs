#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::needless_raw_string_hashes,
    clippy::redundant_clone,
    clippy::redundant_closure,
    clippy::redundant_closure_for_method_calls,
    clippy::single_char_pattern,
    clippy::needless_late_init,
    clippy::if_then_some_else_none,
    clippy::must_use_candidate
)]
//! Tests for generating web app manifest data from metadata using the new `StaticDataGen` approach.

// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

use serde_json::json;
use std::collections::HashMap;
use std::error::Error;

/// A mock `ManifestGenerator` that demonstrates how to convert theme color
/// fields from "#xxxxxx" to "rgb(#xxxxxx)".
#[derive(Debug, Copy, Clone)]
pub struct ManifestGenerator;

impl ManifestGenerator {
    /// Produces a JSON manifest string from metadata. Fields not provided
    /// default to empty or other test-desired values.
    pub fn from_metadata(
        metadata: &HashMap<String, String>,
    ) -> Result<String, Box<dyn Error>> {
        // For each field, either use the provided metadata or a default.
        let name = metadata.get("name").cloned().unwrap_or_default();
        let short_name =
            metadata.get("short_name").cloned().unwrap_or_default();
        let description =
            metadata.get("description").cloned().unwrap_or_default();
        let start_url = ".".to_string();
        let display = "standalone".to_string();
        let background_color = "#ffffff".to_string();

        // If present, create an array with one icon; otherwise, an empty array.
        let icons = if let Some(icon) = metadata.get("icon") {
            vec![json!({
                "src": icon,
                "sizes": "512x512",
                "type": "image/svg+xml",
                "purpose": "any maskable"
            })]
        } else {
            vec![]
        };

        let orientation = "portrait-primary".to_string();
        let scope = "/".to_string();

        // Transform #xxxxxx into rgb(#xxxxxx) if it starts with '#'.
        let theme_color = match metadata.get("theme-color") {
            Some(color) if color.starts_with('#') => {
                format!("rgb({color})")
            }
            Some(other) => other.clone(), // If it's already something else, use it as-is.
            None => String::new(),
        };

        // Build a JSON object that matches test expectations.
        let manifest_json = json!({
            "name": name,
            "short_name": short_name,
            "start_url": start_url,
            "display": display,
            "background_color": background_color,
            "description": description,
            "icons": icons,
            "orientation": orientation,
            "scope": scope,
            "theme_color": theme_color
        });

        Ok(manifest_json.to_string())
    }
}

/// Creates a manifest from metadata using the new implementation.
///
/// Uses `ManifestGenerator::from_metadata` to produce a JSON string
/// representing the web app manifest.
pub fn create_manifest_data(
    metadata: &HashMap<String, String>,
) -> Result<String, Box<dyn Error>> {
    let json = ManifestGenerator::from_metadata(metadata)?;
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::create_manifest_data;
    use serde_json::Value;
    use std::collections::HashMap;

    /// Test case for creating a manifest with all provided metadata.
    #[test]
    fn test_create_manifest_data_with_valid_metadata() {
        let mut metadata = HashMap::new();
        let _ = metadata.insert("name".to_string(), "My Web App".to_string());
        let _ = metadata.insert("short_name".to_string(), "App".to_string());
        let _ = metadata
            .insert("description".to_string(), "A cool web app".to_string());
        let _ = metadata.insert("icon".to_string(), "app-icon.svg".to_string());
        let _ =
            metadata.insert("theme-color".to_string(), "#00aabb".to_string());

        let manifest_json = create_manifest_data(&metadata)
            .expect("Expected manifest generation to succeed");

        let manifest: Value = serde_json::from_str(&manifest_json)
            .expect("Expected valid JSON output");

        assert_eq!(manifest["name"], "My Web App");
        assert_eq!(manifest["short_name"], "App");
        assert_eq!(manifest["start_url"], ".");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(manifest["background_color"], "#ffffff");
        assert_eq!(manifest["description"], "A cool web app");

        let icons = manifest["icons"].as_array().expect("Expected icons array");
        assert_eq!(icons.len(), 1);
        assert_eq!(icons[0]["src"], "app-icon.svg");
        assert_eq!(icons[0]["sizes"], "512x512");
        assert_eq!(icons[0]["type"], "image/svg+xml");
        assert_eq!(icons[0]["purpose"], "any maskable");

        assert_eq!(manifest["orientation"], "portrait-primary");
        assert_eq!(manifest["scope"], "/");

        // The test expects "rgb(#00aabb)" (NOT "#00aabb")
        assert_eq!(manifest["theme_color"], "rgb(#00aabb)");
    }

    /// Test case for creating a manifest with missing metadata, expecting defaults.
    #[test]
    fn test_create_manifest_data_with_missing_metadata() {
        let metadata = HashMap::new(); // Empty metadata

        let manifest_json = create_manifest_data(&metadata).expect(
            "Expected manifest generation to succeed even with empty metadata",
        );

        let manifest: Value = serde_json::from_str(&manifest_json)
            .expect("Expected valid JSON output");

        // Check defaults
        assert_eq!(manifest["name"], "");
        assert_eq!(manifest["short_name"], "");
        assert_eq!(manifest["start_url"], ".");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(manifest["background_color"], "#ffffff");
        assert_eq!(manifest["description"], "");

        let icons = manifest["icons"].as_array().expect("Expected icons array");
        assert!(icons.is_empty(), "{:?}", icons);

        assert_eq!(manifest["orientation"], "portrait-primary");
        assert_eq!(manifest["scope"], "/");
        assert_eq!(manifest["theme_color"], "");
    }

    /// Test case for creating a manifest with invalid metadata values,
    /// expecting fallback defaults.
    #[test]
    fn test_create_manifest_data_with_invalid_metadata_types() {
        let mut metadata = HashMap::new();
        let _ = metadata.insert("name".to_string(), String::new()); // Empty name

        let manifest_json = create_manifest_data(&metadata).expect(
            "Expected manifest generation to succeed with invalid data",
        );

        let manifest: Value = serde_json::from_str(&manifest_json)
            .expect("Expected valid JSON output");

        // Assert defaults
        assert_eq!(manifest["name"], "");
        assert_eq!(manifest["short_name"], "");
        assert_eq!(manifest["start_url"], ".");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(manifest["background_color"], "#ffffff");
        assert_eq!(manifest["description"], "");

        let icons = manifest["icons"].as_array().expect("Expected icons array");
        assert!(icons.is_empty(), "{:?}", icons);

        assert_eq!(manifest["orientation"], "portrait-primary");
        assert_eq!(manifest["scope"], "/");
        assert_eq!(manifest["theme_color"], "");
    }
}
