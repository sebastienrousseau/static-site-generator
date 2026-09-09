// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    missing_docs,
    unused_results
)]

//! Benchmarks for `ssg::content` schema parsing.

use criterion::{criterion_group, Criterion};
use ssg::content::parse_schemas;

// Keys must match `RawSchema` / `RawField` in `src/core/content.rs`: the
// schema is keyed by `name`, and each field's type is `type` (serde
// renames it from `field_type`), lower-case, from the `FieldType` set.
// This fixture had `content_type` / `field_type = "DateTime"`, none of
// which parse — the bench panicked on the first iteration. Nothing
// noticed, because the `core` bench target had no `harness = false` and
// so never ran.
const POST_SCHEMA: &str = r#"
[[schemas]]
name = "post"

[[schemas.fields]]
name = "title"
type = "string"
required = true

[[schemas.fields]]
name = "date"
type = "date"
required = true
"#;

fn bench_parse_schemas(c: &mut Criterion) {
    c.bench_function("content::parse_schemas", |b| {
        b.iter(|| parse_schemas(POST_SCHEMA).unwrap());
    });
}

criterion_group!(benches, bench_parse_schemas);
