// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Draft filtering plugin.
//!
//! Removes content files with `draft: true` in their frontmatter
//! before compilation, unless the `--drafts` flag is set.

use crate::error::{PathErrorExt, SsgError};
use crate::plugin::{Plugin, PluginContext};
use crate::MAX_DIR_DEPTH;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Plugin that filters draft content before compilation.
///
/// In `before_compile`, scans content files for `draft: true` in
/// frontmatter and renames them to `.md.draft` so staticdatagen
/// skips them. In `after_compile`, restores the originals.
#[derive(Debug, Clone, Copy)]
pub struct DraftPlugin {
    include_drafts: bool,
}

impl DraftPlugin {
    /// Creates a new `DraftPlugin`.
    ///
    /// If `include_drafts` is true, draft files are left in place.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::drafts::DraftPlugin;
    /// use ssg::plugin::Plugin;
    ///
    /// let p = DraftPlugin::new(false);
    /// assert_eq!(p.name(), "drafts");
    /// ```
    #[must_use]
    pub const fn new(include_drafts: bool) -> Self {
        Self { include_drafts }
    }
}

impl Plugin for DraftPlugin {
    fn name(&self) -> &'static str {
        "drafts"
    }

    fn before_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if self.include_drafts || !ctx.content_dir.exists() {
            return Ok(());
        }

        let md_files = collect_md_files(&ctx.content_dir)?;
        let mut hidden = 0usize;

        for path in &md_files {
            if is_draft(path)? {
                let draft_path = path.with_extension("md.draft");
                fs::rename(path, &draft_path).with_path(path)?;
                hidden += 1;
            }
        }

        if hidden > 0 {
            log::info!(
                "[drafts] Hidden {hidden} draft file(s) (use --drafts to include)"
            );
        }
        Ok(())
    }

    fn after_compile(&self, ctx: &PluginContext) -> Result<(), SsgError> {
        if self.include_drafts || !ctx.content_dir.exists() {
            return Ok(());
        }

        // Restore hidden drafts
        let draft_files = collect_draft_files(&ctx.content_dir)?;
        for draft_path in &draft_files {
            let original = draft_path.with_extension("");
            if !original.exists() {
                fs::rename(draft_path, &original).with_path(draft_path)?;
            }
        }
        Ok(())
    }
}

/// Checks if a Markdown file has `draft: true` in its frontmatter.
fn is_draft(path: &Path) -> Result<bool, SsgError> {
    let content = fs::read_to_string(path).with_path(path)?;

    // Quick check: look for draft field in YAML frontmatter
    if !content.starts_with("---") {
        return Ok(false);
    }

    // Find the closing ---
    if let Some(end) = content[3..].find("---") {
        let frontmatter = &content[3..3 + end];
        // Check for draft: true (handles various YAML formats)
        for line in frontmatter.lines() {
            let trimmed = line.trim();
            if trimmed == "draft: true"
                || trimmed == "draft: True"
                || trimmed == "draft: TRUE"
                || trimmed == "draft: yes"
            {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn collect_md_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files_bounded_depth(dir, "md", MAX_DIR_DEPTH)
}

fn collect_draft_files(dir: &Path) -> Result<Vec<PathBuf>, SsgError> {
    crate::walk::walk_files_bounded_depth(dir, "draft", MAX_DIR_DEPTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::init_logger;
    use std::path::PathBuf;
    use tempfile::{tempdir, TempDir};

    // -------------------------------------------------------------------
    // Test fixtures
    // -------------------------------------------------------------------

    /// Builds `<root>/content` and returns the temp dir guard, the
    /// content path, and a `PluginContext` rooted at it.
    fn make_content_layout() -> (TempDir, PathBuf, PluginContext) {
        init_logger();
        let dir = tempdir().expect("create tempdir");
        let content = dir.path().join("content");
        fs::create_dir_all(&content).expect("mkdir content");
        let ctx =
            PluginContext::new(&content, dir.path(), dir.path(), dir.path());
        (dir, content, ctx)
    }

    /// Writes a Markdown file with the given frontmatter draft flag.
    fn write_md(dir: &Path, name: &str, draft_value: Option<&str>) {
        let body = match draft_value {
            Some(v) => format!("---\ntitle: T\ndraft: {v}\n---\nbody"),
            None => "---\ntitle: T\n---\nbody".to_string(),
        };
        fs::write(dir.join(name), body).expect("write md");
    }

    // -------------------------------------------------------------------
    // Constructor + derive surface
    // -------------------------------------------------------------------

    #[test]
    fn new_table_driven_constructs_plugin_with_supplied_flag() {
        let cases = [(true, true), (false, false)];
        for (input, expected) in cases {
            let plugin = DraftPlugin::new(input);
            assert_eq!(
                plugin.include_drafts, expected,
                "include_drafts({input}) should be {expected}"
            );
        }
    }

    #[test]
    fn draft_plugin_is_copy_after_move() {
        // Guards the `Copy` derive added in v0.0.34.
        let plugin = DraftPlugin::new(false);
        let _copy = plugin;
        assert_eq!(plugin.name(), "drafts");
    }

    #[test]
    fn name_returns_static_drafts_identifier() {
        assert_eq!(DraftPlugin::new(false).name(), "drafts");
        assert_eq!(DraftPlugin::new(true).name(), "drafts");
    }

    // -------------------------------------------------------------------
    // is_draft — table-driven over every YAML truthy spelling
    // -------------------------------------------------------------------

    #[test]
    fn is_draft_table_driven_truthy_values_return_true() {
        let cases: &[&str] = &[
            "---\ntitle: T\ndraft: true\n---\n",
            "---\ntitle: T\ndraft: True\n---\n",
            "---\ntitle: T\ndraft: TRUE\n---\n",
            "---\ntitle: T\ndraft: yes\n---\n",
            // leading/trailing whitespace on the line is trimmed
            "---\ntitle: T\n  draft: true  \n---\n",
        ];
        let dir = tempdir().expect("tempdir");
        for (i, body) in cases.iter().enumerate() {
            let path = dir.path().join(format!("d{i}.md"));
            fs::write(&path, body).unwrap();
            assert!(
                is_draft(&path).unwrap(),
                "case {i} {body:?} should be detected as draft"
            );
        }
    }

    #[test]
    fn is_draft_table_driven_falsy_values_return_false() {
        let cases: &[&str] = &[
            // explicit false
            "---\ntitle: T\ndraft: false\n---\n",
            // unrecognised value
            "---\ntitle: T\ndraft: maybe\n---\n",
            // missing field entirely
            "---\ntitle: T\n---\n",
            // YAML 1.2 strict — `True` is not the same as `true`
            // in many parsers; we accept it (covered above) but
            // `tRue` and `Yes` are NOT in the accepted list.
            "---\ntitle: T\ndraft: tRue\n---\n",
            "---\ntitle: T\ndraft: Yes\n---\n",
        ];
        let dir = tempdir().expect("tempdir");
        for (i, body) in cases.iter().enumerate() {
            let path = dir.path().join(format!("p{i}.md"));
            fs::write(&path, body).unwrap();
            assert!(
                !is_draft(&path).unwrap(),
                "case {i} {body:?} should NOT be detected as draft"
            );
        }
    }

    #[test]
    fn is_draft_no_frontmatter_returns_false() {
        // The `!content.starts_with("---")` early return at line 88.
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plain.md");
        fs::write(&path, "# No frontmatter\nJust prose.\n").unwrap();
        assert!(!is_draft(&path).unwrap());
    }

    #[test]
    fn is_draft_unterminated_frontmatter_returns_false() {
        // The `if let Some(end) = ...find("---")` branch at line 93
        // must take the implicit `None` path when the closing `---`
        // is missing — function should return Ok(false), not error.
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("unterminated.md");
        fs::write(&path, "---\ntitle: T\ndraft: true\nno closing fence here\n")
            .unwrap();
        assert!(!is_draft(&path).unwrap());
    }

    #[test]
    fn is_draft_empty_file_returns_false() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.md");
        fs::write(&path, "").unwrap();
        assert!(!is_draft(&path).unwrap());
    }

    #[test]
    fn is_draft_missing_file_returns_err() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist.md");
        let result = is_draft(&missing);
        assert!(result.is_err());
        // Debug formatting carries both the variant name and the path,
        // so this asserts the same facts as a `matches!` + field check
        // without leaving an uncoverable non-Io match arm behind.
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("Io"), "expected Io variant, got: {msg}");
        assert!(
            msg.contains("does-not-exist.md"),
            "error should carry the missing path: {msg}"
        );
    }

    // -------------------------------------------------------------------
    // before_compile — short-circuit paths
    // -------------------------------------------------------------------

    #[test]
    fn before_compile_with_include_drafts_does_not_rename_anything() {
        let (_tmp, content, ctx) = make_content_layout();
        write_md(&content, "draft.md", Some("true"));
        write_md(&content, "published.md", None);

        DraftPlugin::new(true).before_compile(&ctx).unwrap();
        assert!(content.join("draft.md").exists());
        assert!(!content.join("draft.md.draft").exists());
        assert!(content.join("published.md").exists());
    }

    #[test]
    fn before_compile_missing_content_dir_returns_ok() {
        // The `!ctx.content_dir.exists()` short-circuit at line 43.
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("missing-content");
        let ctx =
            PluginContext::new(&missing, dir.path(), dir.path(), dir.path());

        DraftPlugin::new(false)
            .before_compile(&ctx)
            .expect("missing content dir is not an error");
        assert!(!missing.exists());
    }

    #[test]
    fn before_compile_renames_only_drafts_leaves_published_intact() {
        let (_tmp, content, ctx) = make_content_layout();
        write_md(&content, "draft.md", Some("true"));
        write_md(&content, "published.md", None);

        DraftPlugin::new(false).before_compile(&ctx).unwrap();
        assert!(!content.join("draft.md").exists());
        assert!(content.join("draft.md.draft").exists());
        assert!(content.join("published.md").exists());
    }

    #[test]
    fn before_compile_recurses_into_subdirectories() {
        // collect_md_files walks the tree — drafts in nested dirs
        // must also be hidden.
        let (_tmp, content, ctx) = make_content_layout();
        let nested = content.join("blog").join("2026");
        fs::create_dir_all(&nested).unwrap();
        write_md(&nested, "secret.md", Some("true"));
        write_md(&content, "live.md", None);

        DraftPlugin::new(false).before_compile(&ctx).unwrap();
        assert!(nested.join("secret.md.draft").exists());
        assert!(!nested.join("secret.md").exists());
        assert!(content.join("live.md").exists());
    }

    #[test]
    fn before_compile_no_drafts_yields_no_renames() {
        let (_tmp, content, ctx) = make_content_layout();
        write_md(&content, "a.md", None);
        write_md(&content, "b.md", Some("false"));

        DraftPlugin::new(false).before_compile(&ctx).unwrap();
        assert!(content.join("a.md").exists());
        assert!(content.join("b.md").exists());
    }

    // -------------------------------------------------------------------
    // after_compile — restoration paths
    // -------------------------------------------------------------------

    #[test]
    fn after_compile_with_include_drafts_short_circuits() {
        // The `self.include_drafts` short-circuit at line 67 must not
        // attempt to restore anything.
        let (_tmp, content, ctx) = make_content_layout();
        // Pre-place a .draft file to prove it is NOT touched.
        fs::write(content.join("ghost.md.draft"), "---\n---\n").unwrap();

        DraftPlugin::new(true).after_compile(&ctx).unwrap();
        assert!(content.join("ghost.md.draft").exists());
        assert!(!content.join("ghost.md").exists());
    }

    #[test]
    fn after_compile_missing_content_dir_returns_ok() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("missing");
        let ctx =
            PluginContext::new(&missing, dir.path(), dir.path(), dir.path());
        DraftPlugin::new(false).after_compile(&ctx).unwrap();
    }

    #[test]
    fn after_compile_restores_draft_extension_to_md() {
        let (_tmp, content, ctx) = make_content_layout();
        fs::write(content.join("post.md.draft"), "---\n---\n").unwrap();

        DraftPlugin::new(false).after_compile(&ctx).unwrap();
        assert!(content.join("post.md").exists());
        assert!(!content.join("post.md.draft").exists());
    }

    #[test]
    fn after_compile_does_not_overwrite_existing_original() {
        // The `if !original.exists()` guard at line 75 must skip the
        // rename when an original-named file is already present.
        // Otherwise we'd silently clobber user content.
        let (_tmp, content, ctx) = make_content_layout();
        fs::write(content.join("post.md"), "USER WROTE THIS").unwrap();
        fs::write(content.join("post.md.draft"), "STALE DRAFT").unwrap();

        DraftPlugin::new(false).after_compile(&ctx).unwrap();
        let body = fs::read_to_string(content.join("post.md")).unwrap();
        assert_eq!(
            body, "USER WROTE THIS",
            "existing original must not be clobbered"
        );
        assert!(
            content.join("post.md.draft").exists(),
            "stale draft is left in place when original exists"
        );
    }

    #[test]
    fn before_and_after_round_trip_restores_original_content() {
        // End-to-end: hide a draft, then restore it, and prove the
        // file is byte-identical to before.
        let (_tmp, content, ctx) = make_content_layout();
        let payload = "---\ntitle: T\ndraft: true\n---\nDRAFT BODY";
        fs::write(content.join("d.md"), payload).unwrap();

        let plugin = DraftPlugin::new(false);
        plugin.before_compile(&ctx).unwrap();
        plugin.after_compile(&ctx).unwrap();

        let restored = fs::read_to_string(content.join("d.md")).unwrap();
        assert_eq!(restored, payload);
    }

    // -------------------------------------------------------------------
    // collect_md_files / collect_draft_files — recursion + filtering
    // -------------------------------------------------------------------

    #[test]
    fn collect_md_files_returns_empty_for_missing_directory() {
        let dir = tempdir().expect("tempdir");
        let result = collect_md_files(&dir.path().join("missing")).unwrap();
        assert!(result.is_empty(), "{:?}", result);
    }

    #[test]
    fn collect_md_files_filters_non_md_extensions() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::write(dir.path().join("b.txt"), "").unwrap();
        fs::write(dir.path().join("c.html"), "").unwrap();

        let result = collect_md_files(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn collect_md_files_recurses_into_nested_subdirectories() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("top.md"), "").unwrap();
        fs::write(nested.join("deep.md"), "").unwrap();

        let result = collect_md_files(dir.path()).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn collect_draft_files_filters_non_draft_extensions() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("a.md.draft"), "").unwrap();
        fs::write(dir.path().join("b.md"), "").unwrap();

        let result = collect_draft_files(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn collect_draft_files_respects_max_dir_depth_guard() {
        // The `depth > MAX_DIR_DEPTH` continue at line 135 is only
        // reached with a tree deeper than the limit. Closes line 136.
        let dir = tempdir().expect("tempdir");
        let mut current = dir.path().to_path_buf();
        for i in 0..MAX_DIR_DEPTH + 2 {
            current = current.join(format!("d{i}"));
            fs::create_dir_all(&current).unwrap();
            fs::write(current.join("p.md.draft"), "").unwrap();
        }
        let result = collect_draft_files(dir.path()).unwrap();
        // At most MAX_DIR_DEPTH+1 files (depths 0..=MAX) survive.
        assert!(result.len() <= MAX_DIR_DEPTH + 1);
    }

    #[test]
    fn collect_md_files_respects_max_dir_depth_guard() {
        let dir = tempdir().expect("tempdir");
        let mut current = dir.path().to_path_buf();
        for i in 0..MAX_DIR_DEPTH + 2 {
            current = current.join(format!("d{i}"));
            fs::create_dir_all(&current).unwrap();
            fs::write(current.join("p.md"), "").unwrap();
        }
        let result = collect_md_files(dir.path()).unwrap();
        assert!(result.len() <= MAX_DIR_DEPTH + 1);
    }

    #[test]
    fn collect_draft_files_recurses_into_nested_subdirectories() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("a");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("top.md.draft"), "").unwrap();
        fs::write(nested.join("nested.md.draft"), "").unwrap();

        let result = collect_draft_files(dir.path()).unwrap();
        assert_eq!(result.len(), 2);
    }

    // -------------------------------------------------------------------
    // I/O error propagation (unix permission fixtures)
    // -------------------------------------------------------------------

    /// Sets unix permission bits on `path` (test-only helper).
    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .expect("set permissions");
    }

    #[test]
    #[cfg(unix)]
    fn before_compile_propagates_walk_error_from_unreadable_subdir() {
        let (_tmp, content, ctx) = make_content_layout();
        let locked = content.join("locked");
        fs::create_dir_all(&locked).unwrap();
        chmod(&locked, 0o000);

        let result = DraftPlugin::new(false).before_compile(&ctx);
        chmod(&locked, 0o755); // restore so tempdir cleanup succeeds
        assert!(result.is_err(), "unreadable subdir must surface as Err");
    }

    #[test]
    #[cfg(unix)]
    fn before_compile_propagates_is_draft_read_error() {
        let (_tmp, content, ctx) = make_content_layout();
        write_md(&content, "secret.md", Some("true"));
        chmod(&content.join("secret.md"), 0o000);

        let result = DraftPlugin::new(false).before_compile(&ctx);
        chmod(&content.join("secret.md"), 0o644);
        assert!(result.is_err(), "unreadable md file must surface as Err");
    }

    #[test]
    #[cfg(unix)]
    fn before_compile_rename_failure_in_readonly_dir_is_propagated() {
        let (_tmp, content, ctx) = make_content_layout();
        write_md(&content, "draft.md", Some("true"));
        // r-x: walking and reading still work, the rename does not.
        chmod(&content, 0o555);

        let result = DraftPlugin::new(false).before_compile(&ctx);
        chmod(&content, 0o755);
        assert!(result.is_err(), "rename in read-only dir must be an Err");
        assert!(content.join("draft.md").exists());
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_propagates_walk_error_from_unreadable_subdir() {
        let (_tmp, content, ctx) = make_content_layout();
        let locked = content.join("locked");
        fs::create_dir_all(&locked).unwrap();
        chmod(&locked, 0o000);

        let result = DraftPlugin::new(false).after_compile(&ctx);
        chmod(&locked, 0o755);
        assert!(result.is_err(), "unreadable subdir must surface as Err");
    }

    #[test]
    #[cfg(unix)]
    fn after_compile_restore_rename_failure_is_propagated() {
        let (_tmp, content, ctx) = make_content_layout();
        fs::write(content.join("post.md.draft"), "---\n---\n").unwrap();
        chmod(&content, 0o555);

        let result = DraftPlugin::new(false).after_compile(&ctx);
        chmod(&content, 0o755);
        assert!(result.is_err(), "restore rename must surface as Err");
        assert!(content.join("post.md.draft").exists());
    }
}
