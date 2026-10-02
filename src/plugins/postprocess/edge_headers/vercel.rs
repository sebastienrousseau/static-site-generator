// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Vercel `vercel.json` headers-array emitter (issue #550 AC3).
//!
//! Vercel's deployment config doesn't tolerate JSON comments, so the
//! PQC documentation is carried as a top-level `_pqc_note` array of
//! strings (Vercel ignores keys it doesn't recognise). The
//! canonical `headers` array follows the Vercel schema verbatim:
//!
//! ```json
//! {
//!   "_pqc_note": ["…"],
//!   "headers": [
//!     {
//!       "source": "/(.*)",
//!       "headers": [
//!         { "key": "Strict-Transport-Security", "value": "…" },
//!         …
//!       ]
//!     }
//!   ]
//! }
//! ```

use super::PQC_NOTE_LINES;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Renders the Vercel JSON body.
///
/// `page_csp` appends one route entry per page that carries a
/// hash-strict per-page `Content-Security-Policy` (spec B4). The
/// per-page entries come **after** the global `/(.*)` group because
/// Vercel applies every matching route in order and later values win
/// for a duplicated key — so pages with inline hashes get their
/// specific policy while everything else keeps the global one.
/// Entries render in the map's sorted order (deterministic output).
///
/// # Errors
///
/// Returns `serde_json::Error` only if the produced `Value` fails to
/// serialise, which in practice never happens because the structure
/// is built from owned strings.
pub(super) fn render(
    headers: &[(String, String)],
    page_csp: &BTreeMap<String, String>,
) -> Result<String, serde_json::Error> {
    let headers_arr: Vec<Value> = headers
        .iter()
        .map(|(k, v)| json!({ "key": k, "value": v }))
        .collect();

    let pqc_note: Vec<Value> = PQC_NOTE_LINES
        .iter()
        .map(|s| Value::String((*s).into()))
        .collect();

    let mut groups: Vec<Value> = vec![json!({
        "source": "/(.*)",
        "headers": headers_arr,
    })];

    // Per-page CSP route entries (spec B4) — sorted BTreeMap order.
    for (path, policy) in page_csp {
        groups.push(json!({
            "source": path,
            "headers": [
                { "key": "Content-Security-Policy", "value": policy }
            ],
        }));
    }

    let config = json!({
        "_pqc_note": pqc_note,
        "headers": groups,
    });

    serde_json::to_string_pretty(&config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postprocess::edge_headers::merged_headers;
    use std::collections::BTreeMap;

    #[test]
    fn render_includes_pqc_note_field() {
        let body = render(&merged_headers(&BTreeMap::new()), &BTreeMap::new())
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let note = parsed.get("_pqc_note").and_then(|v| v.as_array()).unwrap();
        let joined: String = note
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("X25519+ML-KEM-768"));
        assert!(joined.contains("vercel.com"));
    }

    #[test]
    fn render_uses_canonical_headers_array_shape() {
        let body = render(&merged_headers(&BTreeMap::new()), &BTreeMap::new())
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let group = parsed
            .get("headers")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .unwrap();
        assert_eq!(group.get("source").and_then(|v| v.as_str()), Some("/(.*)"));
        let arr = group.get("headers").and_then(|v| v.as_array()).unwrap();
        assert_eq!(arr.len(), 5);
        for item in arr {
            assert!(item.get("key").is_some(), "each entry needs a key");
            assert!(item.get("value").is_some(), "each entry needs a value");
        }
    }

    #[test]
    fn render_emits_one_csp_entry_only() {
        // AC7: no duplicate Content-Security-Policy across the array.
        let body = render(&merged_headers(&BTreeMap::new()), &BTreeMap::new())
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let arr = parsed["headers"][0]["headers"].as_array().unwrap();
        let count = arr
            .iter()
            .filter(|h| {
                h.get("key").and_then(|k| k.as_str()).is_some_and(|s| {
                    s.eq_ignore_ascii_case("Content-Security-Policy")
                })
            })
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn render_keys_are_in_baseline_order() {
        let body = render(&merged_headers(&BTreeMap::new()), &BTreeMap::new())
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let arr = parsed["headers"][0]["headers"].as_array().unwrap();
        let keys: Vec<&str> =
            arr.iter().map(|h| h["key"].as_str().unwrap()).collect();
        assert_eq!(
            keys,
            vec![
                "Strict-Transport-Security",
                "Content-Security-Policy",
                "X-Content-Type-Options",
                "Referrer-Policy",
                "Permissions-Policy",
            ]
        );
    }

    #[test]
    fn render_emits_pretty_printed_json() {
        let body = render(&merged_headers(&BTreeMap::new()), &BTreeMap::new())
            .unwrap();
        // serde_json::to_string_pretty indents with two spaces;
        // assert at least one newline + two-space indent appears.
        assert!(body.contains("\n  "), "pretty output expected");
    }

    #[test]
    fn render_with_empty_headers_produces_valid_json() {
        let body = render(&[], &BTreeMap::new()).unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let arr = parsed["headers"][0]["headers"].as_array().unwrap();
        assert!(arr.is_empty(), "{:?}", arr);
    }

    #[test]
    fn render_pqc_note_lines_match_constant() {
        let body = render(&merged_headers(&BTreeMap::new()), &BTreeMap::new())
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let note = parsed.get("_pqc_note").and_then(|v| v.as_array()).unwrap();
        assert_eq!(note.len(), PQC_NOTE_LINES.len());
        for (i, line) in PQC_NOTE_LINES.iter().enumerate() {
            assert_eq!(note[i].as_str(), Some(*line));
        }
    }

    #[test]
    fn render_appends_per_page_csp_routes_after_global() {
        // spec B4: per-page routes follow `/(.*)`; later matching
        // routes win on Vercel, so the specific policy applies.
        let mut pages: BTreeMap<String, String> = BTreeMap::new();
        let _ = pages.insert(
            "/blog/post/".to_string(),
            "script-src 'self' 'sha256-abc'".to_string(),
        );
        let body = render(&merged_headers(&BTreeMap::new()), &pages).unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let groups = parsed["headers"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["source"].as_str(), Some("/(.*)"));
        assert_eq!(groups[1]["source"].as_str(), Some("/blog/post/"));
        assert_eq!(
            groups[1]["headers"][0]["key"].as_str(),
            Some("Content-Security-Policy")
        );
        assert_eq!(
            groups[1]["headers"][0]["value"].as_str(),
            Some("script-src 'self' 'sha256-abc'")
        );
    }

    #[test]
    fn render_with_custom_headers_round_trips_through_serde() {
        let custom = vec![
            ("Strict-Transport-Security".to_string(), "x".to_string()),
            ("Custom-Key".to_string(), "value".to_string()),
        ];
        let body = render(&custom, &BTreeMap::new()).unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let arr = parsed["headers"][0]["headers"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["key"].as_str(), Some("Custom-Key"));
        assert_eq!(arr[1]["value"].as_str(), Some("value"));
    }

    #[test]
    fn render_source_route_matches_every_path() {
        let body = render(&merged_headers(&BTreeMap::new()), &BTreeMap::new())
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let source = parsed["headers"][0]["source"].as_str().unwrap();
        // Vercel regex matching every path is `/(.*)`.
        assert_eq!(source, "/(.*)");
    }

    #[test]
    fn render_with_special_characters_in_value_escapes_correctly() {
        let custom =
            vec![("Backslash-Test".to_string(), "a\"b\\c".to_string())];
        let body = render(&custom, &BTreeMap::new()).unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let value = parsed["headers"][0]["headers"][0]["value"]
            .as_str()
            .unwrap();
        assert_eq!(value, "a\"b\\c");
    }
}
