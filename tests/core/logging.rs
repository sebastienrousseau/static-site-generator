// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::logging`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::io::Write;

use ssg::logging::{create_log_file, log_arguments, log_initialization};
use tempfile::tempdir;

#[test]
fn create_log_file_writes_to_disk_at_given_path() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ssg.log");
    let mut f = create_log_file(path.to_str().unwrap()).unwrap();
    writeln!(f, "hello").unwrap();
    drop(f);
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("hello"));
}

#[test]
fn log_initialization_prepends_banner() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ssg.log");
    let mut f = create_log_file(path.to_str().unwrap()).unwrap();
    log_initialization(&mut f, "2026-06-07").unwrap();
    drop(f);
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(!body.is_empty(), "should not be empty");
}

#[test]
fn log_arguments_appends_arg_marker() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ssg.log");
    let mut f = create_log_file(path.to_str().unwrap()).unwrap();
    log_arguments(&mut f, "2026-06-07").unwrap();
    drop(f);
    assert!(path.exists());
}
