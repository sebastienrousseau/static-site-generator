// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Theme resolution.
//!
//! A theme is a directory holding a layout set and the assets those
//! layouts reference. The nine published SSG themes keep theirs under
//! `_layouts/`; a project-shaped theme keeps it under `templates/`.
//! Both are accepted, because which one a theme uses is not something a
//! consumer should have to know.
//!
//! Before this existed, using a theme meant hand-writing a path into
//! someone else's tree:
//!
//! ```toml
//! template_dir = "../ssg-themes.github.io/themes/quill/_layouts"
//! ```
//!
//! which breaks the moment the theme moves, says nothing about which
//! theme it is, and gives no error worth reading when it is wrong. A
//! name is resolved instead:
//!
//! ```toml
//! theme = "quill"
//! ```

use std::path::{Path, PathBuf};

/// Directory names inside a theme that may hold its layouts, in the
/// order they are tried.
const LAYOUT_DIRS: [&str; 2] = ["_layouts", "templates"];

/// Why a theme name did not resolve.
///
/// Both variants carry what was searched. A theme that cannot be found
/// is nearly always a path problem, and a message that does not say
/// where it looked leaves the reader guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeError {
    /// No directory of that name in any search root.
    NotFound {
        /// The requested theme name.
        name: String,
        /// Every directory that was searched, in order.
        searched: Vec<PathBuf>,
        /// Theme names that do exist in those roots.
        available: Vec<String>,
    },
    /// The directory exists but holds no layouts.
    NoLayouts {
        /// The requested theme name.
        name: String,
        /// The directory that was found.
        dir: PathBuf,
    },
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound {
                name,
                searched,
                available,
            } => {
                write!(f, "no theme named `{name}`. Searched: ")?;
                let paths: Vec<String> =
                    searched.iter().map(|p| p.display().to_string()).collect();
                write!(f, "{}", paths.join(", "))?;
                if available.is_empty() {
                    write!(
                        f,
                        ". No themes found. Put one under `themes/{name}/`, \
                         or set SSG_THEME_PATH to the directory holding your \
                         themes."
                    )
                } else {
                    write!(f, ". Available: {}", available.join(", "))
                }
            }
            Self::NoLayouts { name, dir } => write!(
                f,
                "theme `{name}` at {} has no layouts. Expected one of {} \
                 inside it.",
                dir.display(),
                LAYOUT_DIRS.join(" or ")
            ),
        }
    }
}

impl std::error::Error for ThemeError {}

/// A resolved theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// The theme's own directory.
    pub root: PathBuf,
    /// The directory holding its layouts — what `template_dir` becomes.
    pub template_dir: PathBuf,
}

/// The roots a theme name is searched in, in order.
///
/// `base` first — the directory of the config file naming the theme, so
/// a project's own `themes/` wins over anything installed globally.
/// Then the working directory, then each entry of `SSG_THEME_PATH`.
#[must_use]
pub fn search_roots(base: &Path) -> Vec<PathBuf> {
    let mut roots = vec![base.join("themes")];
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_themes = cwd.join("themes");
        if !roots.contains(&cwd_themes) {
            roots.push(cwd_themes);
        }
    }
    if let Ok(extra) = std::env::var("SSG_THEME_PATH") {
        for part in extra.split(if cfg!(windows) { ';' } else { ':' }) {
            let part = part.trim();
            if !part.is_empty() {
                let p = PathBuf::from(part);
                if !roots.contains(&p) {
                    roots.push(p);
                }
            }
        }
    }
    roots
}

/// Lists the theme names present in `roots`, sorted and deduplicated.
#[must_use]
pub fn available(roots: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = roots
        .iter()
        .filter_map(|r| std::fs::read_dir(r).ok())
        .flat_map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
        })
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Resolves a theme name against `base`.
///
/// # Errors
///
/// [`ThemeError::NotFound`] when no search root holds a directory of
/// that name, and [`ThemeError::NoLayouts`] when one does but contains
/// neither `_layouts/` nor `templates/`.
///
/// # Examples
///
/// ```
/// # use std::fs;
/// # let tmp = tempfile::tempdir().unwrap();
/// # fs::create_dir_all(tmp.path().join("themes/quill/_layouts")).unwrap();
/// let theme = ssg::theme::resolve("quill", tmp.path()).unwrap();
/// assert!(theme.template_dir.ends_with("_layouts"));
/// ```
pub fn resolve(name: &str, base: &Path) -> Result<Theme, ThemeError> {
    let roots = search_roots(base);
    for root in &roots {
        let dir = root.join(name);
        if !dir.is_dir() {
            continue;
        }
        for layout in LAYOUT_DIRS {
            let candidate = dir.join(layout);
            if candidate.is_dir() {
                return Ok(Theme {
                    root: dir,
                    template_dir: candidate,
                });
            }
        }
        return Err(ThemeError::NoLayouts {
            name: name.to_string(),
            dir,
        });
    }
    Err(ThemeError::NotFound {
        name: name.to_string(),
        available: available(&roots),
        searched: roots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn theme_at(root: &Path, name: &str, layout_dir: &str) {
        fs::create_dir_all(root.join("themes").join(name).join(layout_dir))
            .expect("create theme");
    }

    #[test]
    fn resolves_a_layouts_theme() {
        let tmp = TempDir::new().expect("tempdir");
        theme_at(tmp.path(), "quill", "_layouts");
        let theme = resolve("quill", tmp.path()).expect("resolve");
        assert!(theme.template_dir.ends_with("_layouts"));
        assert!(theme.root.ends_with("quill"));
    }

    /// A project-shaped theme keeps its layouts in `templates/`. Which
    /// convention a theme uses is not something a consumer should have
    /// to know, so both resolve.
    #[test]
    fn resolves_a_templates_theme() {
        let tmp = TempDir::new().expect("tempdir");
        theme_at(tmp.path(), "housestyle", "templates");
        let theme = resolve("housestyle", tmp.path()).expect("resolve");
        assert!(theme.template_dir.ends_with("templates"));
    }

    /// `_layouts` is tried first so a theme carrying both is read the
    /// way its author publishes it.
    #[test]
    fn layouts_wins_when_a_theme_carries_both() {
        let tmp = TempDir::new().expect("tempdir");
        theme_at(tmp.path(), "both", "_layouts");
        theme_at(tmp.path(), "both", "templates");
        let theme = resolve("both", tmp.path()).expect("resolve");
        assert!(theme.template_dir.ends_with("_layouts"));
    }

    /// The error has to say where it looked. A theme that does not
    /// resolve is nearly always a path problem, and a bare "not found"
    /// leaves the reader with nothing to check.
    #[test]
    fn a_missing_theme_reports_the_paths_searched_and_what_exists() {
        let tmp = TempDir::new().expect("tempdir");
        theme_at(tmp.path(), "quill", "_layouts");
        theme_at(tmp.path(), "stablo", "_layouts");
        let err = resolve("nosuch", tmp.path()).expect_err("must not resolve");
        let msg = err.to_string();
        assert!(msg.contains("nosuch"), "{msg}");
        assert!(msg.contains("themes"), "should name a search root: {msg}");
        assert!(msg.contains("quill"), "should list what exists: {msg}");
        assert!(msg.contains("stablo"), "should list what exists: {msg}");
    }

    /// A directory of the right name but with no layouts is a different
    /// mistake from a missing one, and gets a different message.
    #[test]
    fn a_theme_without_layouts_is_reported_as_such() {
        let tmp = TempDir::new().expect("tempdir");
        fs::create_dir_all(tmp.path().join("themes/hollow"))
            .expect("create dir");
        let err = resolve("hollow", tmp.path()).expect_err("must not resolve");
        let msg = err.to_string();
        assert!(msg.contains("has no layouts"), "{msg}");
        assert!(msg.contains("_layouts"), "should say what it wanted: {msg}");
    }

    #[test]
    fn available_lists_theme_names_sorted_without_dotfiles() {
        let tmp = TempDir::new().expect("tempdir");
        theme_at(tmp.path(), "voxt", "_layouts");
        theme_at(tmp.path(), "apex", "_layouts");
        fs::create_dir_all(tmp.path().join("themes/.hidden"))
            .expect("hidden dir");
        let names = available(&[tmp.path().join("themes")]);
        assert_eq!(names, vec!["apex".to_string(), "voxt".to_string()]);
    }

    /// The config file's own directory is searched before anything
    /// else, so a build does not depend on where it was invoked from.
    #[test]
    fn the_base_directory_is_the_first_search_root() {
        let tmp = TempDir::new().expect("tempdir");
        let roots = search_roots(tmp.path());
        assert_eq!(roots.first(), Some(&tmp.path().join("themes")));
    }
}
