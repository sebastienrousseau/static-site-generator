// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! URL and path validation utilities.

use super::error::CliError;
use super::RESERVED_NAMES;
use std::fs;
use std::path::Path;

/// Returns `true` if `s` looks like a valid HTTP(S) URL.
///
/// # Examples
///
/// ```rust
/// use ssg::cmd::is_valid_url;
///
/// assert!(is_valid_url("https://example.com"));
/// assert!(is_valid_url("http://example.com:8080/path"));
/// assert!(!is_valid_url("ftp://example.com"));
/// assert!(!is_valid_url("http://localhost"));
/// ```
pub fn is_valid_url(s: &str) -> bool {
    let rest = if let Some(r) = s.strip_prefix("https://") {
        r
    } else if let Some(r) = s.strip_prefix("http://") {
        r
    } else {
        return false;
    };

    // Must have a dot in the host portion
    if !rest.contains('.') {
        return false;
    }

    // Validate port if present: split host from path, then check for ':'
    let authority = rest.split('/').next().unwrap_or(rest);
    if let Some(colon_pos) = authority.rfind(':') {
        let port_str = &authority[colon_pos + 1..];
        if !port_str.is_empty() {
            match port_str.parse::<u16>() {
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }

    true
}

/// Validates a URL for security and format.
///
/// # Examples
/// ```
/// use ssg::cmd::validate_url;
/// assert!(validate_url("http://example.com").is_ok());
/// assert!(validate_url("javascript:alert(1)").is_err());
/// ```
pub fn validate_url(url: &str) -> Result<(), CliError> {
    let xss_patterns = ["javascript:", "data:", "vbscript:"];
    if xss_patterns.iter().any(|p| url.contains(p)) {
        return Err(CliError::InvalidUrl(
            "URL contains unsafe protocol".into(),
        ));
    }

    if url.contains('<') || url.contains('>') || url.contains('"') {
        return Err(CliError::InvalidUrl(
            "URL contains invalid characters".into(),
        ));
    }

    if !is_valid_url(url) {
        return Err(CliError::InvalidUrl(url.to_string()));
    }
    Ok(())
}

pub(super) fn validate_path_safety(
    path: &Path,
    field: &str,
) -> Result<(), CliError> {
    // Check for invalid characters and mixed separators
    let path_str = path.to_string_lossy();

    // Basic invalid characters
    let invalid_chars = ["<", ">", "|", "\"", "?", "*"];
    if invalid_chars.iter().any(|&c| path_str.contains(c)) {
        return Err(CliError::InvalidPath {
            field: field.to_string(),
            details: "Path contains invalid characters".to_string(),
        });
    }

    // Check for mixed/invalid path separators (only on non-Windows)
    #[cfg(not(target_os = "windows"))]
    if path_str.contains('\\') {
        return Err(CliError::InvalidPath {
            field: field.to_string(),
            details: "Path contains backslashes".to_string(),
        });
    }

    // Parent directory traversal check. A relative path is resolved
    // lexically against where it starts and refused only if it climbs
    // above that point, so `a..b.md` and `a/../b` are fine and
    // `../x` or `a/../../x` are not. Absolute paths are left alone:
    // the user typed them on the command line or in their own config,
    // and an absolute path can name any location with or without `..`.
    // "Absolute" here means anchored: it starts at a root or a drive
    // prefix. On Windows `\foo` and `C:foo` are anchored but not
    // `is_absolute()`, and must be accepted as they were before.
    let anchored = matches!(
        path.components().next(),
        Some(std::path::Component::Prefix(_) | std::path::Component::RootDir)
    );
    if !anchored && commons::path::safe_join(Path::new(""), path).is_err() {
        return Err(CliError::InvalidPath {
            field: field.to_string(),
            details: "Path contains parent directory traversal".to_string(),
        });
    }

    // Check for Windows reserved names
    if let Some(stem) = path.file_stem() {
        let stem_lower = stem.to_string_lossy().to_lowercase();
        if RESERVED_NAMES.contains(&stem_lower.as_str()) {
            return Err(CliError::InvalidPath {
                field: field.to_string(),
                details: format!("Path uses reserved name '{stem_lower}'"),
            });
        }
    }

    // If path exists, check if it's a symlink
    if path.exists() {
        fail_point!("cmd::symlink-metadata", |_| {
            #[cfg(all(test, feature = "test-fault-injection"))]
            if !fault::armed("cmd::symlink-metadata") {
                // A concurrent lib test activated the process-global
                // failpoint; this thread did not arm it, so behave as
                // if the failpoint were inactive.
                return check_symlink(path, field);
            }
            Err(CliError::IoError(std::io::Error::other(
                "injected: cmd::symlink-metadata",
            )))
        });
        return check_symlink(path, field);
    }

    Ok(())
}

/// Symlink tail of [`validate_path_safety`]: fetch metadata and reject
/// symlinks. Extracted so the `cmd::symlink-metadata` failpoint can
/// fall through to identical behaviour for unarmed threads.
fn check_symlink(path: &Path, field: &str) -> Result<(), CliError> {
    let metadata = symlink_metadata_checked(path).map_err(|_| {
        CliError::IoError(std::io::Error::other("Failed to get path metadata"))
    })?;

    if metadata.file_type().is_symlink() {
        return Err(CliError::InvalidPath {
            field: field.to_string(),
            details: "Path is a symlink".to_string(),
        });
    }
    Ok(())
}

/// `fs::symlink_metadata` behind the `cmd::symlink-metadata-io`
/// thread-local fault. The real call only fails on a TOCTOU race (the
/// path vanishing after `exists()`), so tests inject the failure to
/// drive the `map_err` branch in [`check_symlink`].
fn symlink_metadata_checked(path: &Path) -> std::io::Result<fs::Metadata> {
    #[cfg(all(test, feature = "test-fault-injection"))]
    if fault::armed("cmd::symlink-metadata-io") {
        return Err(std::io::Error::other(
            "injected: cmd::symlink-metadata-io",
        ));
    }
    fs::symlink_metadata(path)
}

/// Thread-local fault injection scoped to the current thread, so
/// arming a fault in one test cannot leak into concurrently running
/// tests that validate existing paths.
#[cfg(all(test, feature = "test-fault-injection"))]
mod fault {
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<Option<&'static str>> = const { Cell::new(None) };
    }

    /// Arms `name` for the current thread; disarmed when the returned
    /// guard drops (panic-safe).
    pub(super) fn arm(name: &'static str) -> ArmGuard {
        ARMED.with(|a| a.set(Some(name)));
        ArmGuard
    }

    /// Returns whether `name` is armed on the current thread.
    pub(super) fn armed(name: &str) -> bool {
        ARMED.with(|a| a.get() == Some(name))
    }

    /// RAII guard that disarms the thread-local fault on drop.
    #[derive(Debug)]
    pub(super) struct ArmGuard;

    impl Drop for ArmGuard {
        fn drop(&mut self) {
            ARMED.with(|a| a.set(None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_os = "windows"))]
    use clap::Command;
    use tempfile::tempdir;

    #[test]
    fn test_url_validation() {
        let cmd = crate::cmd::Cli::build();
        let _matches = cmd.get_matches_from(vec![
            "ssg",
            "--new",
            "dummy_site",
            "--content",
            "dummy_content",
            "--output",
            "dummy_output",
            "--template",
            "dummy_template",
        ]);

        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("https://example.com<script>").is_err());
    }

    #[test]
    fn test_path_safety() {
        let valid = Path::new("valid");
        let absolute_valid = std::env::current_dir().unwrap().join(valid);
        assert!(validate_path_safety(&absolute_valid, "test").is_ok());
    }

    #[test]
    fn test_absolute_path_validation() {
        let path = std::env::current_dir().unwrap().join("valid_path");
        assert!(validate_path_safety(&path, "test").is_ok());
    }

    #[cfg(not(target_os = "windows"))] // Unix-specific: path behaviour / error messages differ on Windows
    #[test]
    fn test_path_with_separators() {
        let cmd = Command::new("test_no_required_args");
        let _matches = cmd.get_matches_from(vec!["test_no_required_args"]);

        let path = Path::new("path/to\\file");
        let result = validate_path_safety(path, "test");
        assert!(result.is_err(), "Expected error for backslashes");
    }

    #[test]
    fn test_symlink_path_validation() {
        let temp_dir = tempdir().unwrap();
        let target = temp_dir.path().join("target");
        let symlink = temp_dir.path().join("symlink");

        fs::write(&target, "content").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &symlink).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &symlink).unwrap();

        let resolved_path = fs::canonicalize(&symlink).unwrap();
        let normalized_target = fs::canonicalize(&target).unwrap();
        println!("Resolved symlink path: {resolved_path:?}");
        println!("Normalized target path: {normalized_target:?}");

        let result = validate_path_safety(&symlink, "symlink");
        assert!(result.is_err(), "Expected error for symlink path");
        assert!(matches!(
            result,
            Err(CliError::InvalidPath { field: _, details }) if details.contains("symlink")
        ));
    }

    #[test]
    fn test_url_edge_cases() {
        assert!(validate_url("http://").is_err());
        assert!(validate_url("https://").is_err());
        assert!(validate_url("http://example.com:65536").is_err());
    }

    #[test]
    fn test_validate_url_ftp_scheme() {
        assert!(validate_url("ftp://example.com").is_err());
    }

    /// Region-free variant of `assert!(matches!(err, InvalidPath))` —
    /// `matches!` would leave its never-taken false arm uncovered.
    fn assert_invalid_path(result: Result<(), CliError>) {
        let err = result.expect_err("expected InvalidPath error");
        assert!(format!("{err:?}").starts_with("InvalidPath"));
    }

    #[test]
    fn test_validate_path_with_invalid_chars() {
        let result =
            validate_path_safety(Path::new("path<with>invalid"), "test");
        assert_invalid_path(result);
    }

    #[test]
    fn test_validate_path_with_traversal() {
        let result = validate_path_safety(Path::new("../etc/passwd"), "test");
        assert_invalid_path(result);
    }

    #[test]
    fn dotted_names_are_not_traversal() {
        // A substring check for ".." used to reject these.
        for p in ["a..b.md", "content/v1..v2", "..hidden"] {
            assert!(validate_path_safety(Path::new(p), "test").is_ok(), "{p}");
        }
    }

    #[test]
    fn relative_paths_must_not_climb_out() {
        for p in ["..", "../x", "a/../../x", "a/./../../x"] {
            assert_invalid_path(validate_path_safety(Path::new(p), "test"));
        }
    }

    #[test]
    fn backtracking_inside_a_relative_path_is_allowed() {
        assert!(validate_path_safety(Path::new("a/../b"), "test").is_ok());
    }

    #[test]
    fn test_validate_path_with_reserved_name() {
        assert_invalid_path(validate_path_safety(Path::new("con"), "test"));
        assert_invalid_path(validate_path_safety(Path::new("aux"), "test"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_validate_path_with_backslash() {
        let result =
            validate_path_safety(Path::new("path\\with\\backslash"), "test");
        assert_invalid_path(result);
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_path_existing_symlink() {
        let temp_dir = tempdir().unwrap();
        let target = temp_dir.path().join("real");
        let link = temp_dir.path().join("link");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let result = validate_path_safety(&link, "test");
        assert_invalid_path(result);
    }

    #[test]
    fn is_valid_url_empty_port_after_colon_is_accepted() {
        // A trailing colon with no digits skips the port parse
        // entirely — covers the empty-port branch.
        assert!(is_valid_url("http://example.com:"));
        assert!(is_valid_url("https://example.com:/path"));
    }

    #[test]
    fn validate_path_root_has_no_file_stem() {
        // `/` has no file stem, covering the `if let Some(stem)` miss
        // branch; it exists and is not a symlink, so the result is Ok.
        assert!(validate_path_safety(Path::new("/"), "test").is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn validate_path_accepts_windows_anchored_paths() {
        // Anchored at a root or a drive but not `is_absolute()` on
        // Windows; 0.0.73 accepted them and so must the safe_join check.
        for p in [r"\site", r"C:site", r"\..\x"] {
            assert!(
                validate_path_safety(Path::new(p), "test").is_ok(),
                "{p} should be accepted"
            );
        }
    }

    #[cfg(feature = "test-fault-injection")]
    mod fault_injection {
        use super::*;

        /// RAII guard that disables a `fail` failpoint on drop.
        struct FailGuard<'a>(&'a str);

        impl Drop for FailGuard<'_> {
            fn drop(&mut self) {
                let _ = fail::cfg(self.0, "off");
            }
        }

        #[test]
        fn symlink_metadata_failpoint_short_circuits_armed_thread_only() {
            let temp_dir = tempdir().unwrap();
            let existing = temp_dir.path().join("real.txt");
            fs::write(&existing, "x").unwrap();

            let _fail_guard = FailGuard("cmd::symlink-metadata");
            fail::cfg("cmd::symlink-metadata", "return")
                .expect("activate failpoint");

            // Unarmed thread: the active global failpoint must fall
            // through to the real symlink check.
            assert!(
                validate_path_safety(&existing, "test").is_ok(),
                "unarmed thread must pass through the failpoint"
            );

            // Armed thread: the injected error short-circuits.
            let _arm = fault::arm("cmd::symlink-metadata");
            let err = validate_path_safety(&existing, "test")
                .expect_err("armed failpoint must short-circuit");
            assert!(
                format!("{err}").contains("injected: cmd::symlink-metadata")
            );
        }

        #[test]
        fn symlink_metadata_io_fault_drives_map_err() {
            let temp_dir = tempdir().unwrap();
            let existing = temp_dir.path().join("real.txt");
            fs::write(&existing, "x").unwrap();

            let _arm = fault::arm("cmd::symlink-metadata-io");
            let err = validate_path_safety(&existing, "test")
                .expect_err("injected metadata error must propagate");
            assert!(
                format!("{err}").contains("Failed to get path metadata"),
                "map_err must wrap the injected error: {err}"
            );
        }
    }

    // -----------------------------------------------------------------
    // is_valid_url -- edge cases
    // -----------------------------------------------------------------

    #[test]
    fn is_valid_url_empty_string() {
        assert!(!is_valid_url(""));
    }

    #[test]
    fn is_valid_url_no_dot_in_host() {
        assert!(!is_valid_url("http://localhost"));
    }

    #[test]
    fn is_valid_url_just_scheme() {
        assert!(!is_valid_url("http://"));
        assert!(!is_valid_url("https://"));
    }

    #[test]
    fn is_valid_url_with_port() {
        assert!(is_valid_url("http://example.com:8080"));
        assert!(is_valid_url("https://example.com:443"));
    }

    #[test]
    fn is_valid_url_with_path() {
        assert!(is_valid_url("http://example.com/path/to/page"));
        assert!(is_valid_url("https://example.com/"));
    }

    #[test]
    fn is_valid_url_invalid_port() {
        assert!(!is_valid_url("http://example.com:99999"));
        assert!(!is_valid_url("http://example.com:notaport"));
    }

    #[test]
    fn is_valid_url_no_scheme() {
        assert!(!is_valid_url("example.com"));
        assert!(!is_valid_url("ftp://example.com"));
    }

    // -----------------------------------------------------------------
    // validate_url -- invalid schemes, missing host
    // -----------------------------------------------------------------

    #[test]
    fn validate_url_data_scheme_rejected() {
        assert!(validate_url("data:text/html,<h1>hi</h1>").is_err());
    }

    #[test]
    fn validate_url_vbscript_scheme_rejected() {
        assert!(validate_url("vbscript:MsgBox").is_err());
    }

    #[test]
    fn validate_url_missing_host_after_scheme() {
        assert!(validate_url("http://").is_err());
    }

    #[test]
    fn validate_url_angle_brackets_rejected() {
        assert!(validate_url("http://example.com/<script>").is_err());
    }

    #[test]
    fn validate_url_quote_rejected() {
        assert!(validate_url("http://example.com/\"test").is_err());
    }
}
