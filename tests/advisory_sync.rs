// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Keeps the two advisory ignore lists consistent.
//!
//! `deny.toml` drives `cargo-deny`; `osv-scanner.toml` drives the
//! `OSV-based` checks `OpenSSF` Scorecard runs. They answer the same
//! question about the same project and are maintained by hand, which is
//! how they drifted: `osv-scanner.toml` carried five advisory IDs that
//! `deny.toml` did not, and lacked one that it did, while its own header
//! claimed the two matched "exactly". Nothing checked, so nothing
//! noticed.
//!
//! The invariant asserted here is a superset, not equality, because the
//! two tools resolve different graphs. `cargo-deny` walks the
//! feature-resolved tree; `OSV` scans the lockfile, which lists optional
//! dependencies whether or not a feature enables them. So `OSV` can
//! legitimately need to ignore something cargo-deny never sees —
//! RUSTSEC-2026-0235 is exactly that — but the reverse is always a
//! mistake: an advisory deliberately accepted for cargo-deny and absent
//! here means Scorecard reports a vulnerability the project has already
//! reasoned about.

use std::collections::BTreeSet;
use std::fs;

fn read(name: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!("{root}/{name}"))
        .unwrap_or_else(|e| panic!("cannot read {name}: {e}"))
}

/// Advisory IDs in `deny.toml`'s active `ignore = [...]` array.
///
/// Scoped to the array rather than the whole file on purpose: that file
/// also mentions retired advisories in prose, explaining why they were
/// removed. A comment about an advisory is not an ignore of it — the
/// same distinction that made an earlier version of the `$OUT` gate
/// pass a mutation it should have failed.
fn deny_ignores() -> BTreeSet<String> {
    let text = read("deny.toml");
    let Some((_, rest)) = text.split_once("ignore = [") else {
        panic!("deny.toml has no `ignore = [` array");
    };
    let Some((body, _)) = rest.split_once(']') else {
        panic!("deny.toml's ignore array is unterminated");
    };
    body.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('"'))
        .filter_map(|l| l.split('"').nth(1))
        .map(str::to_owned)
        .collect()
}

/// Advisory IDs in `osv-scanner.toml`'s `[[IgnoredVulns]]` entries.
fn osv_ignores() -> BTreeSet<String> {
    read("osv-scanner.toml")
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("id ="))
        .filter_map(|l| l.split('"').nth(1))
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_deny_ignore_is_also_ignored_by_osv() {
    let deny = deny_ignores();
    let osv = osv_ignores();

    assert!(
        !deny.is_empty() && !osv.is_empty(),
        "parsed {} deny and {} osv entries — a parser returning nothing \
         would make this gate pass without comparing anything",
        deny.len(),
        osv.len()
    );

    let missing: Vec<&String> = deny.difference(&osv).collect();
    assert!(
        missing.is_empty(),
        "these advisories are accepted in deny.toml but not in \
         osv-scanner.toml, so Scorecard will report them as \
         vulnerabilities the project has already reasoned about: \
         {missing:?}"
    );
}

/// Entries `OSV` ignores that cargo-deny does not are allowed, but each
/// must carry a reason. An unexplained suppression is indistinguishable
/// from one nobody has revisited.
#[test]
fn every_osv_only_ignore_states_a_reason() {
    let text = read("osv-scanner.toml");
    let blocks: Vec<&str> = text.split("[[IgnoredVulns]]").skip(1).collect();
    // One block per `id =` line, and at least one: a fixed floor here
    // failed when dead entries were removed, which is the wrong outcome
    // for a cleanup this file asks for.
    let ids = osv_ignores().len();
    assert!(
        ids > 0 && blocks.len() == ids,
        "{} IgnoredVulns blocks parsed for {ids} ids — the split is wrong",
        blocks.len()
    );

    let unexplained: Vec<String> = blocks
        .iter()
        .filter(|b| !b.contains("reason"))
        .filter_map(|b| {
            b.lines()
                .find(|l| l.trim().starts_with("id ="))
                .and_then(|l| l.split('"').nth(1))
                .map(str::to_owned)
        })
        .collect();

    assert!(
        unexplained.is_empty(),
        "these osv-scanner.toml entries suppress an advisory without \
         saying why: {unexplained:?}"
    );
}
