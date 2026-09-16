// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! # Static Site Generator - Main Entry Point
//!
//! This module contains the main entry point for initiating the Static Site Generator.
//! It defines the `main` function and an `execute_main_logic` helper function, which together
//! handle the core execution flow, including error handling.
//!
//! ## Core Behaviour
//! - **Execution Flow**: Calls `run` from the `ssg` module, which parses
//!   argv and dispatches the selected subcommand.
//! - **Exit Status**: On success, prints nothing — each invocation reports
//!   its own result, so only the site-producing ones announce a build. On
//!   failure, prints `error: <cause>` to stderr and exits non-zero.
//!
//! ## Example Usage
//! ```rust,no_run
//! use ssg::run;
//! // Just call `run` and handle success or error.
//! match run() {
//!     Ok(_) => {} // `run` reports its own completion
//!     Err(e) => eprintln!("Error encountered: {}", e),
//! }
//! ```

/// The main entry point of the Static Site Generator.
///
/// Delegates to [`ssg::run`] and maps the result to an exit code.
///
/// ### Exit Codes
/// - Returns `0` on success, including for a bare `ssg` that only
///   printed help.
/// - Returns a non-zero status code if an error occurs.
fn main() {
    match ssg::run() {
        // The completion line is emitted by `dispatch_invocation`, which
        // knows whether the invocation actually produced a site. Printing
        // it here claimed one for `check`, `audit` and `plugins` too.
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
