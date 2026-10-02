// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Edge RPC schema emitter (issue #548 AC1 + AC4).
//!
//! Walks the `ssg_rpc` dispatch inventory at build time and writes a
//! TypeScript declaration file at `dist/.ssg/rpc.d.ts`. The matching
//! JS client (`web/rpc.js`) consumes that file via a TS users
//! import.
//!
//! ## Wiring
//!
//! The plugin is **registered unconditionally** in `Plugins::build`,
//! but it is a no-op when zero `#[ssg_rpc]`-annotated functions are
//! reachable from the binary (which is the case for `ssg` itself —
//! users add the macro in their own crates that pull `ssg-rpc`).
//!
//! ## Why a plugin and not a `build.rs`?
//!
//! Because the inventory only contains functions that are reachable
//! from the **final binary**. `build.rs` runs at compile time when
//! the user's RPCs are not yet linked. The plugin runs at the same
//! lifecycle stage as the ISR manifest emitter, which means the
//! `dist/.ssg/` directory it writes into is already created.

use std::fs;
use std::path::Path;

use crate::error::SsgError;
use crate::plugin::{Plugin, PluginContext};

/// Relative path inside `dist/` where the schema is written.
pub const RPC_DTS_RELATIVE_PATH: &str = ".ssg/rpc.d.ts";

/// `after_compile` plugin that emits `dist/.ssg/rpc.d.ts`.
///
/// Skips silently when the dispatch inventory is empty so the
/// behaviour is byte-identical to v0.0.43 for users who don't opt
/// into Edge RPC.
///
/// # Examples
///
/// ```
/// use ssg::plugin::Plugin;
/// use ssg::rpc_schema::RpcSchemaPlugin;
/// assert_eq!(RpcSchemaPlugin::new().name(), "rpc-schema");
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct RpcSchemaPlugin;

impl RpcSchemaPlugin {
    /// Constructs a new instance.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::rpc_schema::RpcSchemaPlugin;
    /// let _plugin = RpcSchemaPlugin::new();
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Plugin for RpcSchemaPlugin {
    fn name(&self) -> &'static str {
        "rpc-schema"
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if ctx.dry_run {
            return Ok(());
        }

        // No RPCs registered — nothing to emit. Stay silent so
        // `ssg build` output looks identical for non-RPC sites.
        if ssg_rpc::dispatch::iter_descriptors().next().is_none() {
            return Ok(());
        }

        let opts = ssg_rpc::EmitOptions::default();
        let ts = ssg_rpc::emit_typescript(&opts);

        let out_path = ctx.site_dir.join(RPC_DTS_RELATIVE_PATH);
        ensure_parent(&out_path)?;
        fs::write(&out_path, ts).map_err(|e| SsgError::Io {
            path: out_path.clone(),
            source: e,
        })?;

        Ok(())
    }
}

fn ensure_parent(path: &Path) -> Result<(), SsgError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| SsgError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginContext;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use ssg_rpc::{ssg_rpc, RpcError};
    use tempfile::tempdir;

    /// Tiny in-test RPC so the dispatch inventory has at least one
    /// entry during `cargo test --lib`, exercising the full emit
    /// path inside `RpcSchemaPlugin::after_compile`. Without this,
    /// the inventory is empty and the early-return at line ~78
    /// hides the rest of the function from coverage instrumentation.
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    struct CovInput {
        v: u32,
    }

    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    struct CovOutput {
        out: u32,
    }

    #[ssg_rpc]
    #[doc = "Coverage probe: only exists so the inventory is non-empty."]
    fn _ssg_rpc_schema_coverage_probe(
        input: CovInput,
    ) -> Result<CovOutput, RpcError> {
        Ok(CovOutput { out: input.v + 1 })
    }

    fn ctx_for(site_dir: &Path) -> PluginContext {
        PluginContext {
            content_dir: site_dir.to_path_buf(),
            build_dir: site_dir.to_path_buf(),
            site_dir: site_dir.to_path_buf(),
            template_dir: site_dir.to_path_buf(),
            config: None,
            cache: None,
            memory_budget: None,
            html_files: None,
            dep_graph: None,
            dry_run: false,
        }
    }

    #[test]
    fn plugin_name_is_stable() {
        assert_eq!(RpcSchemaPlugin::new().name(), "rpc-schema");
    }

    #[test]
    fn dry_run_short_circuits() {
        let dir = tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.dry_run = true;
        RpcSchemaPlugin::new().after_compile(&ctx).unwrap();
        // No file written.
        assert!(!dir.path().join(RPC_DTS_RELATIVE_PATH).exists());
    }

    #[test]
    fn coverage_probe_dispatches_and_increments() {
        // Drives the `_ssg_rpc_schema_coverage_probe` body via the
        // dispatcher so its 5 lines (signature + return expression)
        // are covered. Without this, the probe is registered into
        // the inventory but never executed.
        let out = ssg_rpc::dispatch::dispatch(
            "_ssg_rpc_schema_coverage_probe",
            r#"{"v":41}"#,
        )
        .expect("dispatch");
        assert!(out.contains("\"out\":42"));
    }

    #[test]
    fn writes_typescript_when_inventory_nonempty() {
        // The `_ssg_rpc_schema_coverage_probe` above registers a
        // single descriptor, so iter_descriptors().next() returns
        // Some(_) and the emit path executes end-to-end.
        let dir = tempdir().unwrap();
        let ctx = ctx_for(dir.path());
        RpcSchemaPlugin::new().after_compile(&ctx).unwrap();
        let path = dir.path().join(RPC_DTS_RELATIVE_PATH);
        assert!(path.exists(), "rpc.d.ts must be written");
        let txt = fs::read_to_string(&path).unwrap();
        assert!(
            txt.contains("AUTO-GENERATED"),
            "emitted file should carry the header: {txt}"
        );
    }

    #[test]
    fn ensure_parent_creates_missing_directory() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a/b/c/file.d.ts");
        ensure_parent(&nested).unwrap();
        assert!(nested.parent().unwrap().is_dir());
    }

    #[test]
    fn ensure_parent_path_without_parent_is_ok() {
        // `Path::new("")` has no parent — must be a no-op (Ok).
        ensure_parent(Path::new("")).unwrap();
    }

    #[test]
    fn after_compile_fails_when_ssg_dir_squatted_by_file() {
        // `.ssg` exists as a file, so ensure_parent's create_dir_all
        // fails and the Io closure fires.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".ssg"), "not a dir").unwrap();
        let ctx = ctx_for(dir.path());
        let err = RpcSchemaPlugin::new().after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }

    #[test]
    fn after_compile_fails_when_dts_path_squatted_by_dir() {
        // A directory squats `.ssg/rpc.d.ts`, so fs::write fails.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(RPC_DTS_RELATIVE_PATH)).unwrap();
        let ctx = ctx_for(dir.path());
        let err = RpcSchemaPlugin::new().after_compile(&ctx).unwrap_err();
        assert!(!format!("{err}").is_empty(), "should not be empty");
    }
}
