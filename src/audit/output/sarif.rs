// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! SARIF v2.1.0 formatter for audit reports (issue #562).
//!
//! [SARIF](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html)
//! (Static Analysis Results Interchange Format) is the OASIS-standard
//! schema for static-analysis findings. It is the native ingestion
//! format for GitHub Advanced Security (Code Scanning), GitLab Ultra,
//! Sonatype Lifecycle, and most other enterprise security platforms.
//!
//! The emitter maps `ssg audit`'s domain model 1:1 to SARIF:
//!
//! | ssg field                       | SARIF field                                            |
//! | ------------------------------- | ------------------------------------------------------ |
//! | [`crate::audit::AuditReport`]   | `runs[0]`                                              |
//! | [`crate::audit::GateResult`]    | `runs[0].tool.driver.rules[]` (one per gate)           |
//! | [`crate::audit::Finding`]       | `runs[0].results[]`                                    |
//! | [`crate::audit::Finding::gate`] | `result.ruleId` (or `<gate>.<code>` when code present) |
//! | [`crate::audit::Severity`]      | `result.level` (`error` / `warning` / `note`)          |
//! | [`crate::audit::Finding::path`] | `result.locations[0].physicalLocation.artifactLocation.uri` |
//! | [`crate::audit::Finding::message`] | `result.message.text`                               |
//!
//! Site-wide findings (no `path`) omit the `locations` array per
//! the SARIF spec.
//!
//! ## Acceptance criteria (issue #562)
//!
//! 1. `ssg audit --format=sarif > out.sarif` produces a file that
//!    passes the [SARIF v2.1.0 validator](https://sarifweb.azurewebsites.net/Validation).
//! 2. New CI step uploads the SARIF artefact via
//!    `github/codeql-action/upload-sarif@v3`.
//! 3. Findings appear in the GitHub Security tab on a deliberately-
//!    broken fixture.

use crate::audit::{AuditReport, Finding, GateResult, Severity};

/// Serialises `report` to a pretty-printed SARIF v2.1.0 JSON string.
///
/// # Examples
///
/// ```
/// use ssg::audit::AuditReport;
/// use ssg::audit::output::sarif::format;
///
/// let s = format(&AuditReport { gates: vec![] });
/// // Top-level shape: $schema, version, runs[].
/// assert!(s.contains("\"version\": \"2.1.0\""));
/// assert!(s.contains("\"runs\""));
/// ```
#[must_use]
pub fn format(report: &AuditReport) -> String {
    let runs = serde_json::json!([build_run(report)]);
    let doc = serde_json::json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": runs,
    });
    // Infallible: every value is a String / number / Map / Vec /
    // serde_json::Value. The `Result<String>` path is impossible to
    // exercise in safe Rust, which is why we don't surface one to
    // callers.
    stringify(serde_json::to_string_pretty(&doc))
}

/// Unwraps the serialised document, degrading to a minimal (but
/// schema-valid) empty SARIF document on the impossible-in-practice
/// serialisation failure.
fn stringify(res: Result<String, serde_json::Error>) -> String {
    res.unwrap_or_else(|_| String::from("{\"version\":\"2.1.0\",\"runs\":[]}"))
}

/// Builds the per-run SARIF object: tool descriptor + rules + results.
fn build_run(report: &AuditReport) -> serde_json::Value {
    serde_json::json!({
        "tool": {
            "driver": {
                "name":            "ssg",
                "informationUri":  "https://github.com/sebastienrousseau/static-site-generator",
                "semanticVersion": env!("CARGO_PKG_VERSION"),
                "rules":           rules_for(report),
            }
        },
        "results": results_for(report),
    })
}

/// One SARIF rule descriptor per gate that appears in the report.
fn rules_for(report: &AuditReport) -> Vec<serde_json::Value> {
    report.gates.iter().map(rule_for_gate).collect()
}

/// Synthesises a `reportingDescriptor` for a single gate.
fn rule_for_gate(g: &GateResult) -> serde_json::Value {
    serde_json::json!({
        "id":               g.name,
        "name":             g.name,
        "shortDescription": { "text": format!("ssg audit gate `{}`", g.name) },
        "fullDescription":  { "text": describe_gate(&g.name) },
        "helpUri":          format!(
            "https://github.com/sebastienrousseau/static-site-generator#audit-gate-{}",
            g.name
        ),
    })
}

/// Returns a short human-readable description for known gates.
/// Falls back to a generic phrasing for unknown gate names so the
/// emitter never crashes on a custom gate.
fn describe_gate(name: &str) -> String {
    match name {
        "wcag"          => "WCAG 2.2 Level AA conformance checks against the built HTML.".to_string(),
        "jsonld"        => "Schema.org JSON-LD blocks: schema validity, required fields, type coherence.".to_string(),
        "hreflang"      => "Multilingual hreflang link-rel completeness and reciprocity.".to_string(),
        "csp_sri"       => "Content Security Policy + Subresource Integrity hashes.".to_string(),
        "pqc_tls"       => "Post-quantum-aware TLS and HSTS edge headers.".to_string(),
        "html5"         => "HTML5 structural validity (doctype, charset, landmarks).".to_string(),
        "links"         => "Internal link resolution; optional external HEAD probing.".to_string(),
        "metadata"      => "Open Graph + Twitter Card + canonical chain completeness.".to_string(),
        "markdownlint"  => "Markdown formatting and frontmatter sanity.".to_string(),
        "performance"   => "Lighthouse-aligned performance budgets (Speed Index, LCP, CLS).".to_string(),
        "ai_discovery"  => "agents.txt + .well-known/{ai-plugin.json,mcp.json} discovery files.".to_string(),
        "feeds"         => "RSS / Atom / JSON Feed schema validity.".to_string(),
        "images"        => "Responsive <picture>, alt text presence, modern format coverage.".to_string(),
        "search_index"  => "ssg-search artefacts: embeddings.bin, model.bin, tokenizer.bin, manifest.json.".to_string(),
        "lang_consistency" => "JSON-LD inLanguage vs <html lang> base-language consistency.".to_string(),
        other            => format!("ssg audit gate `{other}` (third-party).") ,
    }
}

/// SARIF `result` array for every finding across every gate.
fn results_for(report: &AuditReport) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for gate in &report.gates {
        for finding in &gate.findings {
            out.push(result_for_finding(finding));
        }
    }
    out
}

/// Maps one [`Finding`] to one SARIF `result` object.
fn result_for_finding(f: &Finding) -> serde_json::Value {
    let mut obj = serde_json::Map::new();

    // ruleId: prefer `<gate>.<code>` when code is present so two
    // different findings under the same gate stay distinguishable;
    // otherwise fall back to the bare gate name.
    let rule_id = match &f.code {
        Some(code) => format!("{}.{}", f.gate, code),
        None => f.gate.clone(),
    };
    let _ = obj.insert("ruleId".into(), serde_json::Value::String(rule_id));

    // level: SARIF's three canonical levels.
    let level = match f.severity {
        Severity::Error => "error",
        Severity::Warn => "warning",
        Severity::Info => "note",
    };
    let _ = obj.insert("level".into(), serde_json::Value::String(level.into()));

    let _ =
        obj.insert("message".into(), serde_json::json!({ "text": f.message }));

    // SARIF spec allows omitting `locations` for site-wide findings,
    // but GitHub Code Scanning requires every result to carry at
    // least one location. For findings without a specific path we
    // emit a synthetic site-wide location pointing at the audit
    // surface itself; downstream tooling can de-prioritise the
    // "<site-wide>" URI if it wants per-file grouping only.
    let uri = f.path.as_deref().unwrap_or("<site-wide>");
    let _ = obj.insert(
        "locations".into(),
        serde_json::json!([{
            "physicalLocation": {
                "artifactLocation": {
                    "uri": uri,
                    "uriBaseId": "%SRCROOT%",
                }
            }
        }]),
    );

    // Custom properties block carrying gate context.
    let _ = obj.insert(
        "properties".into(),
        serde_json::json!({
            "gate":     f.gate,
            "severity": severity_str(f.severity),
        }),
    );

    serde_json::Value::Object(obj)
}

/// Returns the lowercase canonical string for a severity.
const fn severity_str(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "error",
        Severity::Warn => "warn",
        Severity::Info => "info",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{Finding, GateResult, SeverityCounts};

    fn empty_report() -> AuditReport {
        AuditReport { gates: vec![] }
    }

    fn report_with_two_findings() -> AuditReport {
        AuditReport {
            gates: vec![
                GateResult {
                    name: "wcag".to_string(),
                    skipped: false,
                    skip_reason: None,
                    severity_counts: SeverityCounts {
                        info: 0,
                        warn: 0,
                        error: 1,
                    },
                    findings: vec![Finding::new(
                        "wcag",
                        Severity::Error,
                        "<img> missing alt",
                    )
                    .with_code("WCAG-1.1.1")
                    .with_path("blog/post.html")],
                },
                GateResult {
                    name: "hreflang".to_string(),
                    skipped: false,
                    skip_reason: None,
                    severity_counts: SeverityCounts {
                        info: 1,
                        warn: 0,
                        error: 0,
                    },
                    findings: vec![Finding::new(
                        "hreflang",
                        Severity::Info,
                        "no-op site-wide finding",
                    )],
                },
            ],
        }
    }

    #[test]
    fn emits_top_level_sarif_shape() {
        let s = format(&empty_report());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["version"], "2.1.0");
        assert!(v["$schema"].as_str().unwrap().contains("sarif-spec"));
        assert!(v["runs"].is_array());
        assert_eq!(v["runs"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn driver_carries_name_and_semantic_version() {
        let s = format(&empty_report());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let driver = &v["runs"][0]["tool"]["driver"];
        assert_eq!(driver["name"], "ssg");
        assert_eq!(driver["semanticVersion"], env!("CARGO_PKG_VERSION"));
        assert!(driver["informationUri"]
            .as_str()
            .unwrap()
            .starts_with("https://"));
    }

    #[test]
    fn one_rule_per_gate() {
        let s = format(&report_with_two_findings());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
        let ids: Vec<&str> =
            rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"wcag"));
        assert!(ids.contains(&"hreflang"));
    }

    #[test]
    fn known_gate_has_description() {
        // Spot-check that the describe_gate helper is wired.
        let s = format(&report_with_two_findings());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let wcag = v["runs"][0]["tool"]["driver"]["rules"][0]
            ["fullDescription"]["text"]
            .as_str()
            .unwrap()
            .to_string();
        // wcag is the first gate in our fixture.
        assert!(wcag.contains("WCAG"));
    }

    #[test]
    fn unknown_gate_uses_generic_description() {
        let report = AuditReport {
            gates: vec![GateResult {
                name: "thirdparty".to_string(),
                skipped: false,
                skip_reason: None,
                severity_counts: SeverityCounts::default(),
                findings: vec![],
            }],
        };
        let s = format(&report);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let txt = v["runs"][0]["tool"]["driver"]["rules"][0]["fullDescription"]
            ["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(txt.contains("third-party"));
    }

    #[test]
    fn severity_to_sarif_level_mapping() {
        let report = report_with_two_findings();
        let s = format(&report);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let results = v["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results[0]["level"], "error");
        assert_eq!(results[1]["level"], "note");
    }

    #[test]
    fn warn_severity_maps_to_warning_level_and_warn_str() {
        // Covers the `Severity::Warn` arm of both `level` (line 151)
        // and `severity_str` (line 194) which are otherwise unhit by
        // the Error+Info fixture above.
        let report = AuditReport {
            gates: vec![GateResult {
                name: "links".to_string(),
                skipped: false,
                skip_reason: None,
                severity_counts: SeverityCounts {
                    info: 0,
                    warn: 1,
                    error: 0,
                },
                findings: vec![Finding::new(
                    "links",
                    Severity::Warn,
                    "soft warning",
                )
                .with_path("blog/x.html")],
            }],
        };
        let s = format(&report);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let r0 = &v["runs"][0]["results"][0];
        assert_eq!(r0["level"], "warning");
        assert_eq!(r0["properties"]["severity"], "warn");
    }

    #[test]
    fn rule_id_includes_code_when_present() {
        let s = format(&report_with_two_findings());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let r0 = &v["runs"][0]["results"][0];
        assert_eq!(r0["ruleId"], "wcag.WCAG-1.1.1");
    }

    #[test]
    fn rule_id_omits_code_when_absent() {
        let s = format(&report_with_two_findings());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let r1 = &v["runs"][0]["results"][1];
        assert_eq!(r1["ruleId"], "hreflang");
    }

    #[test]
    fn path_emits_physical_location_block() {
        let s = format(&report_with_two_findings());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let r0 = &v["runs"][0]["results"][0];
        assert_eq!(
            r0["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "blog/post.html"
        );
        assert_eq!(
            r0["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uriBaseId"],
            "%SRCROOT%"
        );
    }

    #[test]
    fn site_wide_finding_emits_synthetic_location() {
        // GitHub Code Scanning rejects results without locations[],
        // so site-wide findings now carry a synthetic `<site-wide>`
        // URI rather than omitting the field.
        let s = format(&report_with_two_findings());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let r1 = &v["runs"][0]["results"][1];
        let loc = &r1["locations"][0]["physicalLocation"]["artifactLocation"];
        assert_eq!(loc["uri"], "<site-wide>");
        assert_eq!(loc["uriBaseId"], "%SRCROOT%");
    }

    #[test]
    fn properties_carry_gate_and_severity() {
        let s = format(&report_with_two_findings());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let r0 = &v["runs"][0]["results"][0];
        assert_eq!(r0["properties"]["gate"], "wcag");
        assert_eq!(r0["properties"]["severity"], "error");
    }

    #[test]
    fn message_text_carries_human_readable() {
        let s = format(&report_with_two_findings());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(
            v["runs"][0]["results"][0]["message"]["text"],
            "<img> missing alt"
        );
    }

    #[test]
    fn stringify_falls_back_to_minimal_document_on_error() {
        let err = serde_json::from_str::<serde_json::Value>("{")
            .expect_err("truncated JSON must fail");
        let s = stringify(Err(err));
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["version"], "2.1.0");
        assert!(
            v["runs"].as_array().unwrap().is_empty(),
            "{:?}",
            v["runs"].as_array().unwrap()
        );
    }

    #[test]
    fn empty_report_emits_empty_results_array() {
        let s = format(&empty_report());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let results = v["runs"][0]["results"].as_array().unwrap();
        assert!(results.is_empty(), "{:?}", results);
    }
}
