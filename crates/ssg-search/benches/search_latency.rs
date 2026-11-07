// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
//! Latency bench (issue #545 AC3).
//!
//! Builds a 1000-doc corpus, then measures `embed + search` query
//! latency. Asserts the p99 stays under 100 ms on the build host —
//! native Rust is a strict upper bound on WASM (WASM-SIMD typically
//! comes within 1.2-1.5x of native for tight f32 loops).

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use ssg_search::artifacts::{Artifacts, InputDoc};
use ssg_search::VectorEngine;
use std::time::Instant;

fn build_corpus(n: usize) -> Artifacts {
    // Long-tail content seeded with a handful of topic stems so the
    // corpus has meaningful structure rather than uniform noise.
    let topics = [
        "rust webassembly compiles deterministically with simd intrinsics",
        "static site generators built in rust with rayon parallelism",
        "baking sourdough bread starter flour water salt and time",
        "italian pasta carbonara guanciale pecorino romano cheese",
        "photography portrait lighting reflectors softbox umbrella",
        "machine learning embeddings vector spaces cosine similarity",
        "browser performance core web vitals largest contentful paint",
        "css grid layout responsive design fluid typography",
        "kubernetes pod autoscaling reference architecture observability",
        "typescript generics conditional types template literal types",
    ];
    let docs: Vec<InputDoc> = (0..n)
        .map(|i| {
            let topic = topics[i % topics.len()];
            InputDoc {
                url: format!("/doc/{i}"),
                title: format!("Document {i}"),
                body: format!("{topic} — extra body number {i}"),
                excerpt: topic.chars().take(80).collect(),
            }
        })
        .collect();
    Artifacts::from_docs(&docs)
}

fn percentile(mut xs: Vec<f64>, p: f64) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((xs.len() as f64) * p).floor() as usize;
    xs[idx.min(xs.len() - 1)]
}

fn main() {
    let n = 1000usize;
    let arts = build_corpus(n);
    let engine = VectorEngine::new(
        &arts.model,
        &arts.tokenizer,
        &arts.embeddings,
        arts.count(),
    )
    .expect("engine");

    let queries = [
        "rust wasm simd",
        "sourdough recipe",
        "css responsive grid",
        "kubernetes autoscaling",
        "vector embeddings",
        "italian pasta",
        "core web vitals",
        "typescript generics",
        "portrait lighting",
        "static site generator",
    ];

    // Warm-up.
    for q in &queries {
        let _ = engine.search(q, 10);
    }

    let mut samples = Vec::with_capacity(1000);
    for _ in 0..100 {
        for q in &queries {
            let t0 = Instant::now();
            let r = engine.search(q, 10);
            let elapsed_us = t0.elapsed().as_secs_f64() * 1e6;
            samples.push(elapsed_us);
            // Anti-DCE: read first element so the compiler can't elide.
            assert_ne!(r, Vec::<f32>::new());
        }
    }

    let p50 = percentile(samples.clone(), 0.50);
    let p90 = percentile(samples.clone(), 0.90);
    let p99 = percentile(samples.clone(), 0.99);
    let p999 = percentile(samples.clone(), 0.999);

    println!("ssg-search bench / N={n} / native");
    println!("  p50  = {:8.1} µs", p50);
    println!("  p90  = {:8.1} µs", p90);
    println!("  p99  = {:8.1} µs ({:.3} ms)", p99, p99 / 1_000.0);
    println!("  p999 = {:8.1} µs ({:.3} ms)", p999, p999 / 1_000.0);

    // AC3: p99 query latency < 100 ms.
    let p99_ms = p99 / 1_000.0;
    assert!(
        p99_ms < 100.0,
        "p99 latency {p99_ms:.3} ms exceeds 100 ms budget"
    );
}
