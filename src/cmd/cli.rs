// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! CLI argument parsing and banner display.
//!
//! Two parsers coexist here:
//!
//! 1. The legacy flag-style parser exposed via [`Cli::build`] — kept for
//!    backwards compatibility with `ssg -s public -w -t templates`
//!    invocations. Slated for removal in 1.0.
//! 2. The subcommand-style parser exposed via [`Cli::subcommand_app`] —
//!    the new `ssg dev / build / check / deploy` surface introduced
//!    by issue #527.
//!
//! [`parse_and_dispatch`] sniffs `argv` and routes to whichever parser
//! matches, emitting a deprecation warning on the legacy path.

use clap::{Arg, ArgAction, Command};
use std::path::PathBuf;

/// Subcommand names recognised by the unified CLI. Used to discriminate
/// between subcommand-style invocations (`ssg dev`, `ssg build …`) and
/// the legacy flag-only form (`ssg -s public`).
pub const SUBCOMMANDS: &[&str] = &[
    "build", "dev", "check", "deploy", "audit", "plugins", "help",
];

/// Deployment targets accepted by `ssg deploy --target …`.
///
/// `none` is the explicit "build only, no upload" target — equivalent to
/// `ssg build` but routed through the deploy plumbing so dry-runs of CI
/// configs are easy to validate.
pub const DEPLOY_TARGETS: &[&str] = &[
    "netlify",
    "vercel",
    "cloudflare-pages",
    "github-pages",
    "s3",
    "none",
];

/// Deprecation message printed to stderr when the legacy flag-only form
/// is used. Kept as a const so tests can assert on the exact text.
pub const LEGACY_DEPRECATION_WARNING: &str =
    "warning: legacy CLI form deprecated; use 'ssg dev' (will be removed in 1.0)";

#[derive(Clone, Copy, Debug, Default)]
/// A simple CLI struct for building the SSG command.
pub struct Cli;

/// Outcome of [`Cli::parse_and_dispatch`] — tells `main`/`run` which
/// subcommand was selected, or that the legacy form was used.
#[derive(Debug, Clone)]
pub enum CliInvocation {
    /// `ssg build [--…]` — produce a static site under the configured
    /// output directory.
    Build,
    /// `ssg dev [--…]` — produce the site and start the dev server.
    Dev,
    /// `ssg check [--…]` — run validators with `dry_run: true` and exit.
    Check,
    /// `ssg deploy --target <target>` — build then invoke the deploy
    /// adapter for the chosen target.
    Deploy {
        /// The selected deploy target (`netlify`, `vercel`, …).
        target: String,
    },
    /// `ssg plugins list [--json]` — report the plugin pipeline without
    /// building anything.
    Plugins {
        /// Emit JSON rather than a table.
        json: bool,
        /// Deploy target whose plugin should be included, mirroring
        /// `ssg deploy --target`. Absent lists the plain build pipeline.
        target: Option<String>,
    },
    /// `ssg audit [--gate <name>] [--json|--junit] [--fail-on <sev>]` —
    /// run the 15 native CI gates against the built site (issue #549).
    Audit,
    /// Legacy flag-only invocation (`ssg -s public -w`). Behaves like
    /// `Dev` if `--serve` is present, otherwise like `Build`. Emits a
    /// deprecation warning on stderr before dispatch.
    Legacy,
}

/// Parses a boolean-ish environment value for a `SetTrue` flag.
///
/// clap's default bool parser accepts only `true`/`false`, but an env var is
/// conventionally set to `1`, `yes` or `on`. Anything unrecognised is an
/// error rather than a silent false: a typo'd `SSG_NO_TAG_PAGES=ture` should
/// say so, not quietly generate the pages the operator asked to skip.
fn parse_env_bool(s: &str) -> Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "" => Ok(false),
        other => Err(format!(
            "expected a boolean (1/true/yes/on or 0/false/no/off), got {other:?}"
        )),
    }
}

impl Cli {
    /// Builds the legacy flag-style `clap::Command`.
    ///
    /// Preserved so the deprecation shim, existing examples, and the
    /// already-shipped CI invocations keep working through the 0.0.x
    /// line. Removed in 1.0 per issue #527 AC7.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::cmd::Cli;
    ///
    /// let cmd = Cli::build();
    /// assert!(cmd.get_name().contains("ssg") || !cmd.get_name().is_empty());
    /// ```
    #[must_use]
    pub fn build() -> Command {
        Command::new(env!("CARGO_PKG_NAME"))
            .author(env!("CARGO_PKG_AUTHORS"))
            .about(env!("CARGO_PKG_DESCRIPTION"))
            .version(env!("CARGO_PKG_VERSION"))
            .arg(
                Arg::new("config")
                    .help("Configuration file path")
                    .long("config")
                    .short('f')
                    .value_name("FILE")
                    .value_parser(clap::value_parser!(PathBuf)),
            )
            .arg(
                Arg::new("new")
                    .help("Create new project")
                    .long("new")
                    .short('n')
                    .value_name("NAME")
                    .value_parser(clap::value_parser!(String)), // Change from PathBuf to String
            )
            .arg(
                Arg::new("content")
                    .help("Content directory")
                    .long("content")
                    .short('c')
                    .value_name("DIR")
                    .value_parser(clap::value_parser!(PathBuf)),
            )
            .arg(
                Arg::new("output")
                    .help("Output directory")
                    .long("output")
                    .short('o')
                    .value_name("DIR")
                    .value_parser(clap::value_parser!(PathBuf)),
            )
            .arg(
                Arg::new("template")
                    .help("Template directory")
                    .long("template")
                    .short('t')
                    .value_name("DIR")
                    .value_parser(clap::value_parser!(PathBuf)),
            )
            .arg(
                Arg::new("theme")
                    .help("Theme name; resolves the template directory")
                    .long("theme")
                    .value_name("NAME"),
            )
            .arg(
                Arg::new("serve")
                    .help("Development server directory")
                    .long("serve")
                    .short('s')
                    .value_name("DIR")
                    .value_parser(clap::value_parser!(PathBuf)),
            )
            .arg(
                Arg::new("watch")
                    .help("Watch for changes")
                    .long("watch")
                    .short('w')
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("drafts")
                    .help("Include draft pages in the build")
                    .long("drafts")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("deploy")
                    .help("Generate deployment config (netlify, vercel, cloudflare, github)")
                    .long("deploy")
                    .value_name("TARGET")
                    .value_parser(clap::value_parser!(String)),
            )
            .arg(
                Arg::new("no_tag_pages")
                    .help(
                        "Skip taxonomy (tag/category/topic) page generation",
                    )
                    .long("no-tag-pages")
                    .env("SSG_NO_TAG_PAGES")
                    // `SetTrue` parses the env var as a *value*, and its
                    // default parser accepts only "true"/"false" — so the
                    // conventional `SSG_NO_TAG_PAGES=1` was rejected with
                    // `invalid value '1'`, taking the whole build down. The
                    // flag form was unaffected, which is how this shipped
                    // with the release notes advertising `=1`.
                    .value_parser(parse_env_bool)
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("validate")
                    .help("Validate content schemas without building")
                    .long("validate")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("quiet")
                    .help("Suppress non-error output")
                    .long("quiet")
                    .short('q')
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("verbose")
                    .help("Show detailed build information")
                    .long("verbose")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                // Resolves #422. The flag is always parsed so scripts
                // are stable across feature-on/feature-off builds; if
                // the binary was compiled without the `otel` feature
                // we accept the flag and emit a warning when it's
                // present but the runtime support isn't compiled in.
                Arg::new("trace")
                    .help("Enable OpenTelemetry build tracing (requires `otel` feature)")
                    .long("trace")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("jobs")
                    .help("Number of parallel threads (default: num CPUs)")
                    .long("jobs")
                    .short('j')
                    .value_name("N")
                    .value_parser(clap::value_parser!(usize)),
            )
            .arg(
                Arg::new("max-memory")
                    .help("Peak memory budget in MB for streaming compilation (default: 512)")
                    .long("max-memory")
                    .value_name("MB")
                    .value_parser(clap::value_parser!(usize)),
            )
            .arg(
                Arg::new("ai-fix")
                    .help("Run agentic AI pipeline to audit and fix content readability")
                    .long("ai-fix")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("ai-fix-dry-run")
                    .help("Preview AI fixes without writing changes")
                    .long("ai-fix-dry-run")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("incremental")
                    .help("Rebuild only the pages affected by source changes (issue #524)")
                    .long("incremental")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("no-llm-cache")
                    .help("Disable the deterministic LLM inference cache")
                    .long("no-llm-cache")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("isr")
                    .help("Emit ISR manifest + raw KV payloads under dist/.ssg/ (opt-in, issue #546)")
                    .long("isr")
                    .action(ArgAction::SetTrue),
            )
    }

    /// Builds the subcommand-style `clap::Command` (issue #527).
    ///
    /// Surface:
    ///
    /// ```text
    /// ssg <SUBCOMMAND> [OPTIONS]
    ///
    /// Development:
    ///   dev      Start the dev server with file watching
    ///
    /// Build:
    ///   build    Produce a static site under the configured output dir
    ///
    /// Validate:
    ///   check    Run validators (read-only) and exit
    ///
    /// Deploy:
    ///   deploy   Build and ship to a pluggable target
    /// ```
    ///
    /// Each subcommand carries the same `--config / --output / …`
    /// option pile so existing scripts can be ported one-to-one.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::cmd::Cli;
    ///
    /// let app = Cli::subcommand_app();
    /// let names: Vec<_> = app.get_subcommands().map(|c| c.get_name()).collect();
    /// assert!(names.contains(&"build"));
    /// assert!(names.contains(&"dev"));
    /// ```
    #[must_use]
    pub fn subcommand_app() -> Command {
        let shared = || -> Vec<Arg> {
            vec![
                Arg::new("config")
                    .help("Configuration file path")
                    .long("config")
                    .short('f')
                    .value_name("FILE")
                    .value_parser(clap::value_parser!(PathBuf)),
                Arg::new("content")
                    .help("Content directory")
                    .long("content")
                    .short('c')
                    .value_name("DIR")
                    .value_parser(clap::value_parser!(PathBuf)),
                Arg::new("output")
                    .help("Output directory")
                    .long("output")
                    .short('o')
                    .value_name("DIR")
                    .value_parser(clap::value_parser!(PathBuf)),
                Arg::new("template")
                    .help("Template directory")
                    .long("template")
                    .short('t')
                    .value_name("DIR")
                    .value_parser(clap::value_parser!(PathBuf)),
                Arg::new("theme")
                    .help("Theme name; resolves the template directory")
                    .long("theme")
                    .value_name("NAME"),
                Arg::new("quiet")
                    .help("Suppress non-error output")
                    .long("quiet")
                    .short('q')
                    .action(ArgAction::SetTrue),
                Arg::new("verbose")
                    .help("Show detailed build information")
                    .long("verbose")
                    .action(ArgAction::SetTrue),
                Arg::new("jobs")
                    .help("Number of parallel threads (default: num CPUs)")
                    .long("jobs")
                    .short('j')
                    .value_name("N")
                    .value_parser(clap::value_parser!(usize)),
                Arg::new("no-llm-cache")
                    .help("Disable the deterministic LLM inference cache")
                    .long("no-llm-cache")
                    .action(ArgAction::SetTrue),
            ]
        };

        Command::new(env!("CARGO_PKG_NAME"))
            .author(env!("CARGO_PKG_AUTHORS"))
            .about(env!("CARGO_PKG_DESCRIPTION"))
            .version(env!("CARGO_PKG_VERSION"))
            .subcommand_required(false)
            .arg_required_else_help(false)
            .after_help(
                "Development:\n  dev      Start the dev server with watch + HMR\n\n\
                 Build:\n  build    Produce a static site under public/\n\n\
                 Validate:\n  check    Run validators (no output written)\n\n\
                 Deploy:\n  deploy   Build then ship to a pluggable target\n\n\
                 Run `ssg <SUBCOMMAND> --help` for subcommand-specific options."
            )
            .subcommand(
                Command::new("build")
                    .about("Produce a static site under the configured output directory")
                    .long_about(
                        "Run the full build pipeline and exit. Equivalent to the legacy \
                         `ssg -s <dir>` invocation without `--watch`."
                    )
                    .args(shared())
                    .arg(
                        Arg::new("drafts")
                            .help("Include draft pages in the build")
                            .long("drafts")
                            .action(ArgAction::SetTrue),
                    )
                    .arg(
                        Arg::new("max-memory")
                            .help("Peak memory budget in MB for streaming compilation")
                            .long("max-memory")
                            .value_name("MB")
                            .value_parser(clap::value_parser!(usize)),
                    )
                    .arg(
                        Arg::new("incremental")
                            .help("Rebuild only the pages affected by source changes (issue #524)")
                            .long("incremental")
                            .action(ArgAction::SetTrue),
                    )
                    .arg(
                        Arg::new("isr")
                            .help("Emit ISR manifest + raw KV payloads under dist/.ssg/ (opt-in, issue #546)")
                            .long("isr")
                            .action(ArgAction::SetTrue),
                    ),
            )
            .subcommand(
                Command::new("dev")
                    .about("Start the dev server with file watching and HMR")
                    .long_about(
                        "Build the site, then serve it on http://127.0.0.1:8000 (or \
                         $SSG_HOST:$SSG_PORT). File changes trigger a rebuild and an \
                         HMR push to the browser."
                    )
                    .args(shared())
                    .arg(
                        Arg::new("serve")
                            .help("Override the directory served (defaults to output dir)")
                            .long("serve")
                            .short('s')
                            .value_name("DIR")
                            .value_parser(clap::value_parser!(PathBuf)),
                    )
                    .arg(
                        Arg::new("drafts")
                            .help("Include draft pages in the dev build")
                            .long("drafts")
                            .action(ArgAction::SetTrue),
                    ),
            )
            .subcommand(
                Command::new("check")
                    .about("Run all build-time validators without writing output")
                    .long_about(
                        "Executes the content validation, accessibility, SEO, JSON-LD \
                         and CSP plugins with `dry_run: true`. Exits 0 iff every page \
                         would have passed; otherwise prints the violating pages and \
                         reasons (issue #527 AC3)."
                    )
                    .args(shared()),
            )
            .subcommand(
                Command::new("plugins")
                    .about("Inspect the plugin pipeline")
                    .long_about(
                        "Reports the plugins the build would run, in execution \
                         order, with the optional hooks each opts into. This is \
                         the source of truth for the plugin count quoted in the \
                         README, so documentation cannot drift from the code."
                    )
                    .subcommand(
                        Command::new("list")
                            .about("List registered plugins in execution order")
                            .arg(
                                Arg::new("json")
                                    .help("Emit machine-readable JSON")
                                    .long("json")
                                    .action(ArgAction::SetTrue),
                            )
                            .arg(
                                Arg::new("target")
                                    .help(
                                        "Include the deploy plugin for this \
                                         target, as `ssg deploy` would register it",
                                    )
                                    .long("target")
                                    .value_name("TARGET")
                                    .value_parser(
                                        clap::builder::PossibleValuesParser::new(
                                            DEPLOY_TARGETS,
                                        ),
                                    ),
                            ),
                    ),
            )
            .subcommand(super::audit::build_subcommand())
            .subcommand(
                Command::new("deploy")
                    .about("Build the site and ship to a pluggable target")
                    .long_about(
                        "Runs the build pipeline, then invokes the deploy adapter for \
                         the chosen target. Tokens come from per-target env vars \
                         (e.g. SSG_NETLIFY_TOKEN). The `none` target performs the \
                         build but skips the upload — handy for CI dry-runs."
                    )
                    .args(shared())
                    .arg(
                        Arg::new("target")
                            .help("Deploy target")
                            .long("target")
                            .value_name("TARGET")
                            .required(true)
                            .value_parser(
                                clap::builder::PossibleValuesParser::new(
                                    DEPLOY_TARGETS,
                                ),
                            ),
                    )
                    .arg(
                        Arg::new("drafts")
                            .help("Include draft pages in the deploy build")
                            .long("drafts")
                            .action(ArgAction::SetTrue),
                    ),
            )
    }

    /// Routes `argv` to either the subcommand parser or the legacy
    /// flag parser.
    ///
    /// The contract:
    ///
    /// * If `argv[1]` matches a known subcommand (`build`, `dev`,
    ///   `check`, `deploy`, `help`), parses with the new surface.
    /// * Otherwise, falls back to the legacy parser and prints
    ///   [`LEGACY_DEPRECATION_WARNING`] to stderr (issue #527 AC5,
    ///   except when no args were supplied at all — bare `ssg` is
    ///   silent and behaves like the prior 0.0.42 default).
    ///
    /// Returns a `(CliInvocation, ArgMatches)` pair so the caller can
    /// reuse the existing `SsgConfig::from_matches` / `RunOptions::from_matches`
    /// helpers.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::cmd::{Cli, CliInvocation};
    ///
    /// let (inv, _matches) = Cli::parse_and_dispatch(vec!["ssg", "build"])
    ///     .expect("parses");
    /// assert!(matches!(inv, CliInvocation::Build));
    /// ```
    ///
    /// # Errors
    /// Returns the underlying `clap::Error` if parsing fails — the
    /// caller is expected to print it and exit non-zero.
    pub fn parse_and_dispatch<I, T>(
        argv: I,
    ) -> Result<(CliInvocation, clap::ArgMatches), clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let args: Vec<std::ffi::OsString> =
            argv.into_iter().map(Into::into).collect();

        // Sniff argv[1]. If it's a known subcommand keyword, use the
        // new parser; otherwise fall back to the legacy form.
        let uses_subcommand =
            args.get(1).and_then(|a| a.to_str()).is_some_and(|s| {
                SUBCOMMANDS.contains(&s)
                    || s == "--help"
                    || s == "-h"
                    || s == "--version"
                    || s == "-V"
            });

        if uses_subcommand {
            let matches = Self::subcommand_app().try_get_matches_from(&args)?;
            let inv = match matches.subcommand() {
                Some(("build", _)) => CliInvocation::Build,
                Some(("dev", _)) => CliInvocation::Dev,
                Some(("check", _)) => CliInvocation::Check,
                Some(("audit", _)) => CliInvocation::Audit,
                Some(("plugins", sub_m)) => {
                    let list = sub_m.subcommand_matches("list");
                    CliInvocation::Plugins {
                        json: list.is_some_and(|m| m.get_flag("json")),
                        target: list
                            .and_then(|m| m.get_one::<String>("target"))
                            .cloned(),
                    }
                }
                Some(("deploy", sub_m)) => {
                    let target = sub_m
                        .get_one::<String>("target")
                        .cloned()
                        .unwrap_or_else(|| "none".to_string());
                    CliInvocation::Deploy { target }
                }
                // `--help` / `--version` short-circuit inside clap; if
                // we somehow reach here with no subcommand, treat as
                // legacy no-op (bare invocation).
                _ => CliInvocation::Legacy,
            };
            Ok((inv, matches))
        } else {
            // Legacy path. Only warn if the user actually passed flags
            // — bare `ssg` is the documented default behaviour and
            // shouldn't spam stderr (#527 AC5 talks about
            // `ssg -s public -w`, not bare invocation).
            if args.len() > 1 {
                eprintln!("{LEGACY_DEPRECATION_WARNING}");
            }
            let matches = Self::build().try_get_matches_from(&args)?;
            Ok((CliInvocation::Legacy, matches))
        }
    }

    /// Displays the application banner
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ssg::cmd::Cli;
    ///
    /// // Prints to stdout — runnable in a doctest, no panics.
    /// Cli::print_banner();
    /// ```
    pub fn print_banner() {
        let version = env!("CARGO_PKG_VERSION");
        let mut title = String::with_capacity(16 + version.len());
        title.push_str("SSG \u{1f980} v");
        title.push_str(version);

        let description =
            "A Fast and Flexible Static Site Generator written in Rust";
        let width = title.len().max(description.len()) + 4;
        let line = "\u{2500}".repeat(width - 2);

        println!("\n\u{250c}{line}\u{2510}");
        println!(
            "\u{2502}{:^width$}\u{2502}",
            format!("\x1b[1;32m{title}\x1b[0m"),
            width = width - 3
        );
        println!("\u{251c}{line}\u{2524}");
        println!(
            "\u{2502}{:^width$}\u{2502}",
            format!("\x1b[1;34m{description}\x1b[0m"),
            width = width - 2
        );
        println!("\u{2514}{line}\u{2518}\n");
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn env_bool_accepts_conventional_truthy_values() {
        // Regression: `SetTrue` + `.env()` parses the variable as a value,
        // and clap's default bool parser rejects everything but true/false.
        // `SSG_NO_TAG_PAGES=1` therefore aborted the build with
        // `invalid value '1'` — shipped in 0.0.52 with the release notes
        // advertising exactly that form.
        for v in ["1", "true", "TRUE", "yes", "on", " on "] {
            assert_eq!(parse_env_bool(v), Ok(true), "{v:?}");
        }
        for v in ["0", "false", "no", "off", ""] {
            assert_eq!(parse_env_bool(v), Ok(false), "{v:?}");
        }
    }

    #[test]
    fn env_bool_rejects_garbage_rather_than_defaulting_false() {
        // A typo must not silently produce the opposite of what was asked.
        assert!(parse_env_bool("ture").is_err());
        assert!(parse_env_bool("maybe").is_err());
    }

    use super::*;

    #[test]
    fn test_banner_display() {
        let version = env!("CARGO_PKG_VERSION");
        let title = format!("SSG \u{1f980} v{version}");
        let description =
            "A Fast and Flexible Static Site Generator written in Rust";
        let width = title.len().max(description.len()) + 4;
        let line = "\u{2500}".repeat(width - 2);

        Cli::print_banner();

        assert!(!line.is_empty());
        assert!(title.contains("SSG"));
        assert!(title.contains(version));
    }

    #[test]
    fn build_returns_valid_command() {
        let cmd = Cli::build();
        assert_eq!(cmd.get_name(), env!("CARGO_PKG_NAME"));
        // Ensure all expected arguments are registered
        let arg_names: Vec<&str> =
            cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
        for expected in [
            "config", "new", "content", "output", "template", "theme", "serve",
            "watch", "drafts", "deploy", "validate", "quiet", "verbose",
            "jobs",
        ] {
            assert!(
                arg_names.contains(&expected),
                "missing expected arg: {expected}"
            );
        }
    }

    #[test]
    fn parse_minimal_args() {
        let cmd = Cli::build();
        let matches = cmd.try_get_matches_from(["ssg"]).unwrap();
        // No arguments supplied — all should be absent / false
        assert!(matches.get_one::<PathBuf>("config").is_none());
        assert!(matches.get_one::<PathBuf>("output").is_none());
        assert!(!matches.get_flag("watch"));
        assert!(!matches.get_flag("drafts"));
    }

    #[test]
    fn parse_quiet_flag() {
        let cmd = Cli::build();
        let matches = cmd.try_get_matches_from(["ssg", "--quiet"]).unwrap();
        assert!(matches.get_flag("quiet"));
    }

    #[test]
    fn parse_verbose_flag() {
        let cmd = Cli::build();
        let matches = cmd.try_get_matches_from(["ssg", "--verbose"]).unwrap();
        assert!(matches.get_flag("verbose"));
    }

    #[test]
    fn parse_drafts_flag() {
        let cmd = Cli::build();
        let matches = cmd.try_get_matches_from(["ssg", "--drafts"]).unwrap();
        assert!(matches.get_flag("drafts"));
    }

    #[test]
    fn parse_combined_flags_and_values() {
        let cmd = Cli::build();
        let matches = cmd
            .try_get_matches_from([
                "ssg", "--quiet", "--drafts", "--output", "/tmp/out", "--jobs",
                "4",
            ])
            .unwrap();
        assert!(matches.get_flag("quiet"));
        assert!(matches.get_flag("drafts"));
        assert_eq!(
            matches.get_one::<PathBuf>("output").unwrap(),
            &PathBuf::from("/tmp/out")
        );
        assert_eq!(*matches.get_one::<usize>("jobs").unwrap(), 4);
    }

    #[test]
    // The whole point of this test is to call the derived `Default`
    // impl directly, since nothing else in the crate does — clippy's
    // suggestion to construct `Cli` directly instead would defeat that.
    #[allow(clippy::default_constructed_unit_structs)]
    fn cli_default_is_unit_struct() {
        let _cli = Cli;
        // `Cli` derives `Default` and `Debug` but nothing else in the
        // crate ever calls either derived impl — exercise both
        // directly so they're not dead code from coverage's view.
        let default_cli = Cli::default();
        assert_eq!(format!("{default_cli:?}"), format!("{:?}", Cli));
    }

    // -----------------------------------------------------------------
    // Subcommand parser — added by issue #527
    // -----------------------------------------------------------------

    #[test]
    fn subcommand_app_has_all_four_subcommands() {
        let app = Cli::subcommand_app();
        let names: Vec<&str> =
            app.get_subcommands().map(Command::get_name).collect();
        for expected in ["build", "dev", "check", "deploy"] {
            assert!(
                names.contains(&expected),
                "subcommand `{expected}` missing"
            );
        }
    }

    /// Region-free variant of `assert!(matches!(inv, <Variant>))` —
    /// `matches!` (or a `panic!` fallback arm) would leave a
    /// never-taken region uncovered. `CliInvocation`'s Debug repr is
    /// deterministic, so exact string equality is just as strict.
    fn assert_invocation(inv: &CliInvocation, expected: &str) {
        assert_eq!(format!("{inv:?}"), expected);
    }

    #[test]
    fn parse_build_subcommand() {
        let (inv, _m) = Cli::parse_and_dispatch(["ssg", "build"]).unwrap();
        assert_invocation(&inv, "Build");
    }

    #[test]
    fn parse_dev_subcommand() {
        let (inv, _m) = Cli::parse_and_dispatch(["ssg", "dev"]).unwrap();
        assert_invocation(&inv, "Dev");
    }

    #[test]
    fn parse_check_subcommand() {
        let (inv, _m) = Cli::parse_and_dispatch(["ssg", "check"]).unwrap();
        assert_invocation(&inv, "Check");
    }

    #[test]
    fn parse_audit_subcommand() {
        let (inv, _m) = Cli::parse_and_dispatch(["ssg", "audit"]).unwrap();
        assert_invocation(&inv, "Audit");
    }

    #[test]
    fn parse_deploy_subcommand_with_target() {
        let (inv, _m) =
            Cli::parse_and_dispatch(["ssg", "deploy", "--target", "netlify"])
                .unwrap();
        assert_invocation(&inv, "Deploy { target: \"netlify\" }");
    }

    #[test]
    fn deploy_rejects_unknown_target() {
        let err = Cli::parse_and_dispatch([
            "ssg",
            "deploy",
            "--target",
            "moon-base-alpha",
        ])
        .unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::InvalidValue,
            "unknown deploy target must be rejected by clap"
        );
    }

    #[test]
    fn deploy_requires_target() {
        let err = Cli::parse_and_dispatch(["ssg", "deploy"]).unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::MissingRequiredArgument,
            "--target must be required"
        );
    }

    #[test]
    fn legacy_invocation_with_flags_is_detected() {
        let (inv, _m) =
            Cli::parse_and_dispatch(["ssg", "-s", "public"]).unwrap();
        assert_invocation(&inv, "Legacy");
    }

    #[test]
    fn bare_invocation_routes_through_legacy_parser() {
        let (inv, _m) = Cli::parse_and_dispatch(["ssg"]).unwrap();
        assert_invocation(&inv, "Legacy");
    }

    #[test]
    fn legacy_parser_rejects_unknown_flag() {
        // Covers the `?` propagation from the legacy try_get_matches_from.
        let err = Cli::parse_and_dispatch(["ssg", "--definitely-not-a-flag"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn deploy_targets_const_has_six_entries() {
        // Issue #527 AC4 explicitly lists netlify, vercel,
        // cloudflare-pages, github-pages, s3, none.
        assert_eq!(DEPLOY_TARGETS.len(), 6);
        for t in [
            "netlify",
            "vercel",
            "cloudflare-pages",
            "github-pages",
            "s3",
            "none",
        ] {
            assert!(DEPLOY_TARGETS.contains(&t), "deploy target `{t}` missing");
        }
    }
}
