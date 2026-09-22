// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Build-time SBOM generation (issue #457).
//!
//! Emits a `CycloneDX` 1.5 JSON Software Bill of Materials at the
//! root of the generated site (`sbom.cdx.json`) and links to it
//! from every HTML page via `<link rel="sbom" type="application/vnd.cyclonedx+json">`.
//!
//! # Why ship an SBOM with the static site?
//!
//! Procurement teams in regulated industries (finance, healthcare,
//! government) increasingly require SBOMs for any deployed software
//! — including the build pipeline that produced static assets. The
//! `scheduled.yml` workflow already generates a `CycloneDX` SBOM via
//! `cargo cyclonedx` and attaches a Sigstore provenance attestation,
//! but those artifacts live in CI; they're not discoverable from
//! the deployed site. This plugin fixes that gap by **embedding**
//! the SBOM into every site, making the supply chain machine-
//! introspectable from the consumer's browser.
//!
//! # Format
//!
//! Minimal `CycloneDX` 1.5 (the JSON Schema is documented at
//! <https://cyclonedx.org/docs/1.5/json/>). The component list
//! covers the SSG package itself; transitive Cargo dependencies
//! are out of scope here (they're in the CI-generated SBOM
//! published as a release artifact). The rendered SBOM declares:
//!
//! - `bomFormat`: "`CycloneDX`"
//! - `specVersion`: "1.5"
//! - `serialNumber`: `urn:uuid:…`, derived from the build (see below)
//! - `version`: 1
//! - `metadata.timestamp`: build time (ISO 8601, UTC)
//! - `metadata.tools[]`: SSG name + version
//! - `metadata.component`: the site itself (type: "application")
//! - `components[]`: SSG generator
//!
//! # Discoverability
//!
//! Every HTML page emitted by the build receives a
//! `<link rel="sbom" type="application/vnd.cyclonedx+json"
//!  href="<base-url-path>/sbom.cdx.json">` element in `<head>`.
//! This is the
//! IANA-registered link relation for SBOM discovery (registered
//! 2023; see <https://www.iana.org/assignments/link-relations/>).
//!
//! # Serial number
//!
//! `serialNumber` is optional in the `CycloneDX` schema, but consumers
//! use it to tell the format apart from SPDX. GitHub's attestation
//! action, for one, recognises `CycloneDX` only when `bomFormat`,
//! `serialNumber` and `specVersion` are all present, and otherwise
//! fails a release with "Unsupported SBOM format". An SBOM without it
//! is therefore valid but not attestable.
//!
//! The value is derived from the build rather than drawn at random, so
//! `SOURCE_DATE_EPOCH` pins it along with the timestamp and the
//! determinism gate still holds. Two builds that differ only in wall
//! clock still get different serials, which is what `CycloneDX` asks for.
//!
//! # Idempotency
//!
//! The HTML transform is idempotent — pages that already contain
//! `rel="sbom"` are left unchanged. The JSON file is rewritten on
//! every build (so timestamps stay current).

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::util::head_dom::inject_before_head_close;
use std::fs;
use std::path::Path;

/// Plugin that emits a `CycloneDX` SBOM and links to it from every
/// HTML page.
#[derive(Debug, Clone, Copy, Default)]
pub struct SbomPlugin;

impl SbomPlugin {
    /// Returns the relative path of the SBOM file under `site_dir`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::sbom::SbomPlugin;
    ///
    /// assert_eq!(SbomPlugin::sbom_path(), "sbom.cdx.json");
    /// ```
    pub const fn sbom_path() -> &'static str {
        "sbom.cdx.json"
    }
}

impl Plugin for SbomPlugin {
    fn name(&self) -> &'static str {
        "sbom"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if !ctx.site_dir.exists() {
            return Ok(());
        }
        let sbom = build_sbom();
        let path = ctx.site_dir.join(Self::sbom_path());
        let json = serialize_sbom(&sbom).map_err(|e| SsgError::Io {
            path: path.clone(),
            source: std::io::Error::other(e),
        })?;
        fs::write(&path, json).with_path(&path)?;
        log::info!("[sbom] Wrote CycloneDX SBOM to {}", path.display());
        Ok(())
    }

    fn has_transform(&self) -> bool {
        true
    }

    fn transform_html(
        &self,
        html: &str,
        _path: &Path,
        ctx: &PluginContext,
    ) -> Result<String, SsgError> {
        // Idempotent: skip if an SBOM link is already present.
        if html.contains("rel=\"sbom\"") || html.contains("rel='sbom'") {
            return Ok(html.to_string());
        }
        // The SBOM sits at the *site* root, which is only the server root
        // when `base_url` has no path. On a project site served from a
        // sub-path, a bare `/sbom.cdx.json` points at the domain apex and
        // 404s, so carry the prefix across.
        let link = format!(
            "<link rel=\"sbom\" type=\"application/vnd.cyclonedx+json\" \
             href=\"{}/{}\">\n",
            ctx.config.as_ref().map_or_else(String::new, |c| {
                crate::plugins_group::csp::base_url_path_prefix(&c.base_url)
            }),
            Self::sbom_path()
        );
        Ok(inject_before_head_close(html, &link))
    }
}

/// Derives the SBOM's `serialNumber` from the inputs that identify the
/// build.
///
/// Hashed rather than random so that `SOURCE_DATE_EPOCH` pins the serial
/// exactly as it pins the timestamp; a random UUID would reintroduce the
/// nondeterminism that gate exists to prevent. The digest is truncated to
/// 128 bits and stamped with the RFC 9562 version-8 (custom) and variant
/// bits, which is what that version is for. Using the existing `sha2`
/// dependency keeps this free of a new crate, per `AGENTS.md`.
fn sbom_serial_number(timestamp: &str, ssg_version: &str) -> String {
    use sha2::{Digest as _, Sha256};

    // Domain-separated and length-prefixed, matching `llm_cache`, so no two
    // different field splits can hash to the same digest.
    let mut hasher = Sha256::new();
    hasher.update(b"ssg-sbom-serial-v1\x00");
    hasher.update((timestamp.len() as u64).to_le_bytes());
    hasher.update(timestamp.as_bytes());
    hasher.update((ssg_version.len() as u64).to_le_bytes());
    hasher.update(ssg_version.as_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // version 8
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 9562 variant

    let mut hex = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Builds the minimal `CycloneDX` 1.5 SBOM document for this site.
fn build_sbom() -> serde_json::Value {
    let now = current_iso_timestamp();
    let ssg_version = env!("CARGO_PKG_VERSION");
    serde_json::json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": sbom_serial_number(&now, ssg_version),
        "version": 1,
        "metadata": {
            "timestamp": now,
            "tools": [{
                "vendor": "SSG Contributors",
                "name": "ssg",
                "version": ssg_version,
            }],
            "component": {
                "type": "application",
                "bom-ref": "site",
                "name": "static-site",
                "description": "Site generated by SSG",
            }
        },
        "components": [{
            "type": "application",
            "bom-ref": format!("ssg@{ssg_version}"),
            "name": "ssg",
            "version": ssg_version,
            "description": "Static site generator",
            "purl": format!("pkg:cargo/ssg@{ssg_version}"),
            "licenses": [
                {"license": {"id": "MIT"}},
                {"license": {"id": "Apache-2.0"}}
            ],
            "externalReferences": [
                {"type": "vcs", "url": "https://github.com/sebastienrousseau/static-site-generator"},
                {"type": "documentation", "url": "https://docs.rs/ssg"}
            ]
        }]
    })
}

/// Serialize the SBOM with a fault-injection hook so tests can drive
/// the error branch (pretty-printing a `Value` built from hardcoded
/// strings and numbers cannot fail in practice).
fn serialize_sbom(sbom: &serde_json::Value) -> serde_json::Result<String> {
    fail_point!("sbom::serialize", |_| Err(
        <serde_json::Error as serde::ser::Error>::custom(
            "injected: sbom::serialize"
        )
    ));
    serde_json::to_string_pretty(sbom)
}

/// Cheap ISO 8601 timestamp without pulling in a date crate.
/// Uses `std::time::SystemTime` and converts `UNIX_EPOCH` seconds to
/// `YYYY-MM-DDTHH:MM:SSZ` via the proleptic Gregorian calendar.
///
/// Reproducible builds (SECURITY.md convention, determinism.yml CI
/// gate): a wall-clock timestamp makes `sbom.cdx.json` differ across
/// otherwise-identical builds, so `SOURCE_DATE_EPOCH` wins when set.
/// It pins `serialNumber` too, since that is derived from this value.
fn current_iso_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(secs) = epoch.trim().parse::<u64>() {
            return epoch_to_iso(secs);
        }
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    epoch_to_iso(secs)
}

/// Converts seconds since UNIX epoch to ISO 8601 `YYYY-MM-DDTHH:MM:SSZ`.
fn epoch_to_iso(secs: u64) -> String {
    // Days since 1970-01-01 + seconds within day.
    let days = secs / 86_400;
    let sec_in_day = secs % 86_400;
    let hour = (sec_in_day / 3600) as u32;
    let minute = ((sec_in_day % 3600) / 60) as u32;
    let second = (sec_in_day % 60) as u32;

    // Convert `days` to YYYY-MM-DD via proleptic Gregorian rules.
    // Algorithm from Howard Hinnant's date library (public domain).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::SsgConfig;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn epoch_to_iso_handles_unix_epoch() {
        assert_eq!(epoch_to_iso(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn epoch_to_iso_handles_known_timestamps() {
        // 1700000000 = 2023-11-14 22:13:20 UTC
        assert_eq!(epoch_to_iso(1_700_000_000), "2023-11-14T22:13:20Z");
        // 1577836800 = 2020-01-01 00:00:00 UTC
        assert_eq!(epoch_to_iso(1_577_836_800), "2020-01-01T00:00:00Z");
    }

    #[test]
    #[serial_test::serial(source_date_epoch)]
    fn current_iso_timestamp_honours_source_date_epoch() {
        // determinism.yml gate: SOURCE_DATE_EPOCH must pin this SBOM's
        // timestamp, which is the one that reaches sbom.cdx.json and, via
        // sbom_serial_number, fixes the serial with it.
        let prev = std::env::var("SOURCE_DATE_EPOCH").ok();
        std::env::set_var("SOURCE_DATE_EPOCH", "1700000000");
        let pinned = current_iso_timestamp();
        std::env::set_var("SOURCE_DATE_EPOCH", "not-a-number");
        let fallback = current_iso_timestamp();
        match prev {
            Some(v) => std::env::set_var("SOURCE_DATE_EPOCH", v),
            None => std::env::remove_var("SOURCE_DATE_EPOCH"),
        }
        assert_eq!(pinned, "2023-11-14T22:13:20Z");
        // Unparseable epoch falls back to wall clock — assert only the
        // shape so the test never depends on today's date.
        assert!(fallback.ends_with('Z') && fallback.len() == 20);
    }

    #[test]
    fn build_sbom_includes_required_cyclonedx_fields() {
        let sbom = build_sbom();
        assert_eq!(sbom["bomFormat"], "CycloneDX");
        assert_eq!(sbom["specVersion"], "1.5");
        assert_eq!(sbom["version"], 1);
        assert!(sbom["metadata"]["timestamp"].as_str().is_some());
        assert!(sbom["metadata"]["tools"].as_array().is_some());
        let components = sbom["components"].as_array().unwrap();
        assert!(!components.is_empty());
        // Every component must have a name and a purl.
        for c in components {
            assert!(c["name"].as_str().is_some());
            assert!(c["purl"].as_str().is_some());
        }
    }

    #[test]
    fn build_sbom_is_attestable_as_cyclonedx() {
        // GitHub's attestation action sniffs the format with
        // `bomFormat && serialNumber && specVersion` and rejects anything
        // else as "Unsupported SBOM format", which fails a release at the
        // attestation step. Assert that exact triple, not just validity.
        let sbom = build_sbom();
        for field in ["bomFormat", "serialNumber", "specVersion"] {
            let value = sbom[field].as_str();
            assert!(
                value.is_some_and(|v| !v.is_empty()),
                "{field} must be a non-empty string for the SBOM to be attestable"
            );
        }
    }

    #[test]
    fn sbom_serial_number_has_uuid_urn_shape() {
        let serial = sbom_serial_number("2023-11-14T22:13:20Z", "0.0.63");
        let uuid = serial
            .strip_prefix("urn:uuid:")
            .expect("serialNumber must be a UUID URN");

        // CycloneDX constrains serialNumber to 8-4-4-4-12 lowercase hex.
        let groups: Vec<&str> = uuid.split('-').collect();
        assert_eq!(groups.len(), 5, "expected 8-4-4-4-12, got {uuid}");
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(
            groups.iter().all(|g| g
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))),
            "UUID groups must be lowercase hex: {uuid}"
        );

        // RFC 9562: version nibble 8, variant bits 10xx.
        assert_eq!(
            groups[2].as_bytes()[0],
            b'8',
            "expected version 8 in {uuid}"
        );
        assert!(
            matches!(groups[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
            "expected RFC 9562 variant in {uuid}"
        );
    }

    #[test]
    fn sbom_serial_number_is_derived_not_random() {
        // Derived, so SOURCE_DATE_EPOCH pins it along with the timestamp.
        // A random UUID would pass the shape test above while silently
        // breaking the determinism gate, so pin the value itself.
        let a = sbom_serial_number("2023-11-14T22:13:20Z", "0.0.63");
        let b = sbom_serial_number("2023-11-14T22:13:20Z", "0.0.63");
        assert_eq!(a, b, "same inputs must produce the same serial");

        // Different builds still get different serials, as CycloneDX asks.
        let later = sbom_serial_number("2023-11-14T22:13:21Z", "0.0.63");
        assert_ne!(a, later, "a different timestamp must change the serial");
        let other_version =
            sbom_serial_number("2023-11-14T22:13:20Z", "0.0.64");
        assert_ne!(
            a, other_version,
            "a different ssg version must change the serial"
        );
    }

    #[test]
    #[serial_test::serial(source_date_epoch)]
    fn build_sbom_serial_is_pinned_by_source_date_epoch() {
        // determinism.yml gate: with the epoch pinned, two builds must
        // agree on the serial as well as the timestamp.
        let prev = std::env::var("SOURCE_DATE_EPOCH").ok();
        std::env::set_var("SOURCE_DATE_EPOCH", "1700000000");
        let first = build_sbom();
        let second = build_sbom();
        match prev {
            Some(v) => std::env::set_var("SOURCE_DATE_EPOCH", v),
            None => std::env::remove_var("SOURCE_DATE_EPOCH"),
        }
        // Assert presence first: two absent fields compare equal, so without
        // this the test would still pass if the serial were dropped entirely.
        assert!(first["serialNumber"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert_eq!(first["serialNumber"], second["serialNumber"]);
        assert_eq!(
            first["metadata"]["timestamp"],
            second["metadata"]["timestamp"]
        );
    }

    #[test]
    #[serial_test::parallel]
    fn sbom_plugin_writes_file_after_compile() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        SbomPlugin.after_compile(&ctx).unwrap();
        let sbom_file = site.join(SbomPlugin::sbom_path());
        assert!(sbom_file.exists());
        let body = fs::read_to_string(&sbom_file).unwrap();
        assert!(body.contains("\"CycloneDX\""));
        assert!(body.contains("\"specVersion\": \"1.5\""));
    }

    /// The SBOM sits at the site root, which is only the server root when
    /// `base_url` has no path. On a project site a bare `/sbom.cdx.json`
    /// points at the domain apex and 404s on every page that carries it.
    #[test]
    fn sbom_link_carries_the_base_url_path_prefix() {
        let dir = tempdir().unwrap();
        let config = SsgConfig::builder()
            .base_url("https://example.com/ssg-themes.github.io/apex".into())
            .build()
            .unwrap();
        let ctx = PluginContext::with_config(
            dir.path(),
            dir.path(),
            dir.path(),
            dir.path(),
            config,
        );
        let out = SbomPlugin
            .transform_html(
                "<html><head></head><body></body></html>",
                Path::new("x.html"),
                &ctx,
            )
            .unwrap();
        assert!(
            out.contains(r#"href="/ssg-themes.github.io/apex/sbom.cdx.json""#),
            "{out}"
        );
    }

    #[test]
    fn sbom_plugin_injects_link_into_head() {
        let dir = tempdir().unwrap();
        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let html = "<html><head><title>x</title></head><body></body></html>";
        let out = SbomPlugin
            .transform_html(html, Path::new("x.html"), &ctx)
            .unwrap();
        assert!(out.contains("rel=\"sbom\""));
        assert!(out.contains("application/vnd.cyclonedx+json"));
        assert!(out.contains("href=\"/sbom.cdx.json\""));
    }

    #[test]
    fn sbom_plugin_is_idempotent() {
        let dir = tempdir().unwrap();
        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let html = r#"<html><head><link rel="sbom" type="application/vnd.cyclonedx+json" href="/sbom.cdx.json"></head><body></body></html>"#;
        let out = SbomPlugin
            .transform_html(html, Path::new("x.html"), &ctx)
            .unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn sbom_plugin_is_idempotent_with_single_quoted_attribute() {
        // Covers the `rel='sbom'` disjunct of the idempotency check —
        // every other test only exercises the double-quoted form.
        let dir = tempdir().unwrap();
        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let html = r"<html><head><link rel='sbom' type='application/vnd.cyclonedx+json' href='/sbom.cdx.json'></head><body></body></html>";
        let out = SbomPlugin
            .transform_html(html, Path::new("x.html"), &ctx)
            .unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn sbom_plugin_skips_pages_without_head_tag() {
        let dir = tempdir().unwrap();
        let ctx =
            PluginContext::new(dir.path(), dir.path(), dir.path(), dir.path());
        let html = "<p>orphan content with no head</p>";
        let out = SbomPlugin
            .transform_html(html, Path::new("x.html"), &ctx)
            .unwrap();
        assert_eq!(out, html);
    }

    #[test]
    #[serial_test::parallel]
    fn sbom_plugin_after_compile_noop_when_site_missing() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let ctx =
            PluginContext::new(dir.path(), dir.path(), &missing, dir.path());
        SbomPlugin.after_compile(&ctx).unwrap();
        assert!(!missing.exists());
    }

    #[test]
    fn sbom_path_constant() {
        assert_eq!(SbomPlugin::sbom_path(), "sbom.cdx.json");
    }

    #[test]
    #[serial_test::parallel]
    fn after_compile_write_failure_returns_io_error() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();

        // Create a directory where the SBOM is expected to be written, causing fs::write to fail.
        let sbom_dir = site.join(SbomPlugin::sbom_path());
        fs::create_dir(&sbom_dir).unwrap();

        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());
        let res = SbomPlugin.after_compile(&ctx);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(
            matches!(err, SsgError::Io { ref path, .. } if path == &sbom_dir)
        );
    }
}

#[cfg(all(test, feature = "test-fault-injection"))]
mod fault_tests {
    use super::*;
    use crate::plugin::PluginContext;
    use serial_test::serial;
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
        // `serde_json::to_string_pretty` on the hardcoded `build_sbom()`
        // literal cannot fail in practice, so the only way to exercise
        // `after_compile`'s serialize-error branch is fault injection.
        let _guard = FailGuard("sbom::serialize");
        fail::cfg("sbom::serialize", "return").expect("activate failpoint");

        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir_all(&site).unwrap();
        let ctx = PluginContext::new(dir.path(), dir.path(), &site, dir.path());

        let err = SbomPlugin
            .after_compile(&ctx)
            .expect_err("injected serialize failure must propagate");
        let msg = format!("{err}");
        assert!(msg.contains("sbom.cdx.json"), "got: {msg}");
        assert!(msg.contains("injected: sbom::serialize"), "got: {msg}");
    }
}
