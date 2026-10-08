use super::jsonl::JSONL_TEXT_LIMIT_CHARS;
use super::*;
use crate::config::{Config, ConfigOverrides, ReporterKind};
use crate::model::{
    AnalysisResult, DefinitionCluster, DefinitionComparison, Finding, FindingEvidence, Framework,
    HandlerFingerprint, MatcherDiff, MatcherKind, Rule, Severity, SourceLocation, StepDefinition,
    Suppression,
};
use std::path::{Path, PathBuf};

fn definition(location: SourceLocation, matcher: &str) -> StepDefinition {
    StepDefinition {
        matcher: matcher.to_owned(),
        normalized_matcher: matcher.to_owned(),
        matcher_kind: MatcherKind::CucumberExpression,
        matcher_flags: String::new(),
        handler: HandlerFingerprint {
            exact: String::new(),
            normalized: String::new(),
            alpha_normalized: String::new(),
            structural: String::new(),
            behavior_signature: Vec::new(),
            source_snippet: String::new(),
            comparable: false,
            trivial: true,
        },
        framework: Framework::Unknown,
        registration: "Given".to_owned(),
        location,
        inline_suppressions: Vec::new(),
    }
}

fn result(root: &Path) -> AnalysisResult {
    let primary = SourceLocation::new(root.join("steps/a.ts"), 2, 1, 2, 20);
    let related = SourceLocation::new(root.join("steps/b.ts"), 3, 1, 3, 20);
    AnalysisResult {
        definitions: vec![
            definition(primary.clone(), "the item is visible"),
            definition(related.clone(), "the items are visible"),
        ],
        feature_steps: Vec::new(),
        findings: vec![Finding {
            rule: Rule::DuplicateMatcher,
            severity: Severity::Error,
            message: "Duplicate <matcher>".to_owned(),
            primary,
            related: vec![related],
            evidence: FindingEvidence {
                matcher_similarity: Some(1.0),
                handler_similarity: Some(0.5),
                matcher_difference: "`x` ↔ `x`".to_owned(),
                handler_evidence: "different handlers".to_owned(),
                comparison: Some(DefinitionComparison {
                    left_fingerprint: "left-fingerprint".to_owned(),
                    right_fingerprint: "right-fingerprint".to_owned(),
                    left_matcher: "the item is visible".to_owned(),
                    right_matcher: "the items are visible".to_owned(),
                    left_handler: "() => '</script><script>bad()</script>'".to_owned(),
                    right_handler: "() => safe()".to_owned(),
                    matcher_diff: MatcherDiff {
                        prefix: "the item".to_owned(),
                        left_change: String::new(),
                        right_change: "s".to_owned(),
                        suffix: " are visible".to_owned(),
                    },
                }),
                cluster: None,
            },
            suggested_action: "Keep one".to_owned(),
            suppression: None,
        }],
    }
}

#[test]
fn json_uses_versioned_schema_and_relative_paths() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    analysis.findings[0]
        .message
        .push_str("\u{202e}\u{200b}\u{2060}\u{feff}\u{00ad}\u{e0001}\u{e007f}");
    let json = render_json(&ReportContext::new(&analysis, &root, 0.0)).unwrap();
    assert!(json.contains("\"schemaVersion\": \"3\""));
    assert!(json.contains("\"path\": \"steps/a.ts\""));
    assert!(json.contains("\"leftHandler\": \"() => '</script><script>bad()</script>'\""));
    assert!(!json.contains("/repo/steps"));
    for invisible in [
        '\u{202e}',
        '\u{200b}',
        '\u{2060}',
        '\u{feff}',
        '\u{00ad}',
        '\u{e0001}',
        '\u{e007f}',
    ] {
        assert!(!json.contains(invisible));
    }
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let evidence = &value["findings"][0]["evidence"];
    assert!(evidence.get("matcherSimilarity").is_some());
    assert!(evidence.get("handlerSimilarity").is_some());
    assert!(evidence.get("matcher_similarity").is_none());
}

#[test]
fn default_summary_uses_the_strict_zero_percent_threshold() {
    let root = PathBuf::from("/repo");
    let summary = Summary::from_result(&result(&root));
    assert_eq!(summary.findings, 1);
    assert_eq!(summary.duplication.threshold, 0.0);
    assert!(!summary.duplication.passed);
}

#[test]
fn terminal_report_has_version_and_finding_spacing() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    let mut warning = analysis.findings[0].clone();
    warning.severity = Severity::Warning;
    warning.message = "Check this definition".to_owned();
    analysis.findings.push(warning);
    let metrics = ExecutionMetrics::new(1, 1, 1.0, 2.0, 3.0);
    let context = ReportContext::with_metrics(&analysis, &root, 100.0, &metrics);

    let mut plain = Vec::new();
    write_terminal(&context, &mut plain).unwrap();
    let plain = String::from_utf8(plain).unwrap();
    assert!(plain.starts_with(&format!("CukeDedup v{}\n\n", env!("CARGO_PKG_VERSION"))));
    assert!(plain.contains("  [error] Duplicate <matcher>"));
    assert!(plain.contains("duplicate-matcher\n\n  [error]"));
    assert!(plain.contains("    action: Keep one\n\n  [warning]"));
    assert!(!plain.contains('─'));
    assert!(plain.ends_with("Found 2 findings.\nDetection time: 6.0 ms\n"));
    assert!(!plain.contains('\u{1b}'));
}

#[test]
fn terminal_report_colors_only_selected_text() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    let mut warning = analysis.findings[0].clone();
    warning.severity = Severity::Warning;
    warning.message = "Check this definition".to_owned();
    analysis.findings.push(warning);
    let metrics = ExecutionMetrics::new(1, 1, 1.0, 2.0, 3.0);
    let context = ReportContext::with_metrics(&analysis, &root, 100.0, &metrics);

    let mut plain = Vec::new();
    write_terminal(&context, &mut plain).unwrap();
    let plain = String::from_utf8(plain).unwrap();

    let mut colored = Vec::new();
    super::terminal::write_terminal_with_color(&context, &mut colored, true).unwrap();
    let colored = String::from_utf8(colored).unwrap();
    assert!(colored.contains("\u{1b}[31m[error]\u{1b}[0m Duplicate <matcher>"));
    assert!(colored.contains("\u{1b}[33m[warning]\u{1b}[0m Check this definition"));
    assert!(colored.contains("\u{1b}[1;36mduplicate-matcher\u{1b}[0m\n\n  \u{1b}[31m[error]"));
    assert!(colored.contains("    --> \u{1b}[94msteps/a.ts:2:1\u{1b}[0m"));
    assert!(colored.contains("    related: \u{1b}[94msteps/b.ts:3:1\u{1b}[0m"));
    assert!(colored.ends_with(
        "\u{1b}[90mFound 2 findings.\u{1b}[0m\n\u{1b}[90mDetection time: 6.0 ms\u{1b}[0m\n"
    ));
    assert_eq!(
        colored
            .replace("\u{1b}[31m", "")
            .replace("\u{1b}[33m", "")
            .replace("\u{1b}[1;36m", "")
            .replace("\u{1b}[94m", "")
            .replace("\u{1b}[90m", "")
            .replace("\u{1b}[0m", ""),
        plain
    );
}

#[test]
fn terminal_report_uses_singular_finding_and_omits_optional_timing() {
    let root = PathBuf::from("/repo");
    let analysis = result(&root);
    let without_metrics = ReportContext::new(&analysis, &root, 100.0);
    let mut no_timing = Vec::new();
    write_terminal(&without_metrics, &mut no_timing).unwrap();
    let no_timing = String::from_utf8(no_timing).unwrap();
    assert!(no_timing.ends_with("Found 1 finding.\n"));
    assert!(!no_timing.contains("Detection time:"));
}

/// A writer that fails with `BrokenPipe` once `remaining` bytes have been written.
struct LimitedWriter {
    remaining: usize,
}

impl std::io::Write for LimitedWriter {
    /// Accepts at most `remaining` bytes, then fails every write with `BrokenPipe`.
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        let written = bytes.len().min(self.remaining);
        self.remaining -= written;
        Ok(written)
    }

    /// Buffers nothing, so flushing always succeeds.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A terminal write failure at the footer propagates to the caller instead of being dropped.
#[test]
fn terminal_report_propagates_footer_write_failures() {
    let root = PathBuf::from("/repo");
    let analysis = result(&root);
    let metrics = ExecutionMetrics::new(1, 1, 1.0, 2.0, 3.0);
    let context = ReportContext::with_metrics(&analysis, &root, 100.0, &metrics);
    let mut complete = Vec::new();
    write_terminal(&context, &mut complete).unwrap();
    let complete = String::from_utf8(complete).unwrap();

    for marker in ["Found 1 finding.", "Detection time:"] {
        let mut writer = LimitedWriter {
            remaining: complete.find(marker).unwrap(),
        };
        let error = write_terminal(&context, &mut writer).unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::BrokenPipe,
            "write failure at {marker} should propagate"
        );
    }
}

#[test]
fn jsonl_emits_self_contained_findings_and_a_final_summary() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    analysis.findings[0]
        .evidence
        .comparison
        .as_mut()
        .unwrap()
        .left_handler = "界".repeat(JSONL_TEXT_LIMIT_CHARS + 1);

    let jsonl = render_jsonl(&ReportContext::new(&analysis, &root, 100.0)).unwrap();
    assert!(jsonl.ends_with('\n'));
    let records = jsonl
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);

    let finding = &records[0];
    assert_eq!(finding["schemaVersion"], JSONL_SCHEMA_VERSION);
    assert_eq!(finding["type"], "finding");
    assert_eq!(finding["rule"], "duplicate-matcher");
    assert_eq!(finding["primary"]["path"], "steps/a.ts");
    assert_eq!(finding["active"], true);
    assert_eq!(finding["contributesToThreshold"], true);
    assert_eq!(finding["evidence"]["matcherSimilarity"], 1.0);
    assert_eq!(finding["fingerprint"].as_str().unwrap().len(), 32);
    assert_eq!(
        finding["truncatedFields"],
        serde_json::json!(["evidence.comparison.leftHandler"])
    );
    assert_eq!(
        finding["evidence"]["comparison"]["leftHandler"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        JSONL_TEXT_LIMIT_CHARS
    );

    let summary = &records[1];
    assert_eq!(summary["schemaVersion"], JSONL_SCHEMA_VERSION);
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["summary"]["findings"], 1);
    assert_eq!(summary["summary"]["duplication"]["threshold"], 100.0);
    assert_eq!(summary["recordCount"], 2);
    assert_eq!(summary["truncated"], false);
}

#[test]
fn machine_reporters_preserve_cluster_evidence() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    analysis.findings[0].evidence.comparison = None;
    analysis.findings[0].evidence.cluster = Some(DefinitionCluster {
        member_count: 2,
        definition_fingerprints: vec!["alpha".into(), "bravo".into()],
        pair_findings_collapsed: 7,
        members_truncated: true,
    });
    let context = ReportContext::new(&analysis, &root, 100.0);

    let json: serde_json::Value = serde_json::from_str(&render_json(&context).unwrap()).unwrap();
    let jsonl: serde_json::Value =
        serde_json::from_str(render_jsonl(&context).unwrap().lines().next().unwrap()).unwrap();
    for evidence in [&json["findings"][0]["evidence"], &jsonl["evidence"]] {
        assert_eq!(
            evidence["cluster"]["definitionFingerprints"],
            serde_json::json!(["alpha", "bravo"])
        );
        assert_eq!(evidence["cluster"]["pairFindingsCollapsed"], 7);
        assert_eq!(evidence["cluster"]["membersTruncated"], true);
        assert!(evidence.get("comparison").is_none());
    }
    let sarif: serde_json::Value = serde_json::from_str(&render_sarif(&context).unwrap()).unwrap();
    assert_eq!(
        sarif["runs"][0]["results"][0]["properties"]["clusterMembersTruncated"],
        true
    );
}

#[test]
fn every_reporter_bounds_large_cluster_records_and_signals_omissions() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    let member_count = 300;
    analysis.findings[0].related = (1..member_count)
        .map(|index| {
            SourceLocation::new(root.join(format!("steps/member-{index}.ts")), 1, 1, 1, 10)
        })
        .collect();
    analysis.findings[0].evidence.comparison = None;
    analysis.findings[0].evidence.cluster = Some(DefinitionCluster {
        member_count,
        definition_fingerprints: (0..member_count)
            .map(|index| format!("{index:016x}"))
            .collect(),
        pair_findings_collapsed: member_count - 1,
        members_truncated: false,
    });
    let context = ReportContext::new(&analysis, &root, 100.0);

    let json: serde_json::Value = serde_json::from_str(&render_json(&context).unwrap()).unwrap();
    let finding = &json["findings"][0];
    assert_eq!(
        finding["related"].as_array().unwrap().len(),
        super::shared::MAX_REPORTED_CLUSTER_MEMBERS - 1
    );
    assert_eq!(finding["relatedLocationsTruncated"], true);
    assert_eq!(finding["evidence"]["cluster"]["memberCount"], member_count);
    assert_eq!(
        finding["evidence"]["cluster"]["definitionFingerprints"]
            .as_array()
            .unwrap()
            .len(),
        super::shared::MAX_REPORTED_CLUSTER_MEMBERS
    );
    assert_eq!(finding["evidence"]["cluster"]["membersTruncated"], true);

    let jsonl = render_jsonl(&context).unwrap();
    let jsonl_finding: serde_json::Value =
        serde_json::from_str(jsonl.lines().next().unwrap()).unwrap();
    assert_eq!(jsonl_finding["relatedLocationsTruncated"], true);
    assert!(jsonl.lines().next().unwrap().len() < 20_000);

    let html = render_html(&context).unwrap();
    assert!(html.contains("more locations omitted"));
    let mut terminal = Vec::new();
    write_terminal(&context, &mut terminal).unwrap();
    assert!(String::from_utf8(terminal)
        .unwrap()
        .contains("more locations omitted"));

    let sarif: serde_json::Value = serde_json::from_str(&render_sarif(&context).unwrap()).unwrap();
    let sarif_result = &sarif["runs"][0]["results"][0];
    assert_eq!(
        sarif_result["relatedLocations"].as_array().unwrap().len(),
        super::shared::MAX_REPORTED_CLUSTER_MEMBERS - 1
    );
    assert_eq!(
        sarif_result["properties"]["relatedLocationsTruncated"],
        true
    );

    assert_eq!(analysis.findings[0].related.len(), member_count - 1);
    assert_eq!(
        analysis.findings[0]
            .evidence
            .cluster
            .as_ref()
            .unwrap()
            .definition_fingerprints
            .len(),
        member_count
    );
}

#[test]
fn sarif_contains_only_active_findings_with_stable_locations() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    analysis.findings.push(Finding {
        suppression: Some(Suppression {
            reason: "accepted".to_owned(),
        }),
        ..analysis.findings[0].clone()
    });
    let sarif: serde_json::Value =
        serde_json::from_str(&render_sarif(&ReportContext::new(&analysis, &root, 100.0)).unwrap())
            .unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(sarif["runs"][0]["columnKind"], "unicodeCodePoints");
    assert_eq!(
        sarif["runs"][0]["tool"]["driver"]["rules"][0]["helpUri"],
        "https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/rules.md#duplicate-matcher"
    );
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        true
    );
    assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
            ["uri"],
        "steps/a.ts"
    );
    let fingerprint = sarif["runs"][0]["results"][0]["partialFingerprints"]
        ["cukeDedupFingerprint/v3"]
        .as_str()
        .unwrap();
    assert_eq!(fingerprint.len(), 32);
    assert!(fingerprint
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    assert!(
        sarif["runs"][0]["results"][0]["partialFingerprints"]["cukeDedupFingerprint/v2"].is_null()
    );
}

#[test]
fn every_sarif_rule_links_to_its_documentation_section() {
    let rule_documentation = include_str!("../../docs/rules.md");
    let root = PathBuf::from("/repo");
    let template = result(&root).findings.remove(0);
    let mut analysis = result(&root);
    analysis.findings = Rule::ALL
        .iter()
        .copied()
        .enumerate()
        .map(|(index, rule)| Finding {
            rule,
            primary: SourceLocation::new(root.join(format!("steps/rule-{index}.ts")), 1, 1, 1, 10),
            ..template.clone()
        })
        .collect();

    let sarif: serde_json::Value =
        serde_json::from_str(&render_sarif(&ReportContext::new(&analysis, &root, 100.0)).unwrap())
            .unwrap();
    let descriptors = sarif["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .unwrap();
    assert_eq!(descriptors.len(), Rule::ALL.len());
    for &rule in Rule::ALL {
        let heading = format!("### {}", rule.as_str());
        assert_eq!(
            rule_documentation.matches(&heading).count(),
            1,
            "rule documentation must contain exactly one stable heading for {rule}"
        );
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor["id"] == rule.as_str())
            .unwrap();
        assert_eq!(descriptor["helpUri"], rule.documentation_url());
    }
}

#[test]
fn html_is_self_contained_and_escapes_finding_text() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    analysis.findings[0]
        .message
        .push_str(" safe\u{1b}[31m\u{202e}spoof");
    let html = render_html(&ReportContext::new(&analysis, &root, 0.0)).unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("Duplicate &lt;matcher&gt;"));
    assert!(html.contains("application/json"));
    assert!(html.contains("<mark>s</mark>"));
    assert!(html.contains("&lt;/script&gt;&lt;script&gt;bad()&lt;/script&gt;"));
    assert!(!html.contains("</script><script>bad()"));
    assert!(html.contains("1 error"));
    assert!(!html.contains("1 errors"));
    assert!(html.contains("id=\"theme-toggle\""));
    assert!(html.contains("aria-label=\"Switch color theme\""));
    assert!(html.contains("localStorage.getItem('cuke-dedup-theme')"));
    assert!(html.contains("localStorage.setItem('cuke-dedup-theme',theme)"));
    assert!(html.contains(":root[data-theme=light]"));
    assert!(html.contains(":root[data-theme=dark]"));
    assert!(html.contains("prefers-color-scheme:dark"));
    assert!(html.contains("Findings by rule"));
    assert!(html.contains("<strong>1</strong>"));
    assert!(!html.contains('\u{1b}'));
    assert!(!html.contains('\u{202e}'));
    assert!(!html.contains("\\u202e"));
}

#[test]
fn buffered_reports_cap_findings_and_signal_truncation() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    analysis.findings = (0..=super::shared::MAX_BUFFERED_REPORT_FINDINGS)
        .map(|index| {
            let mut finding = analysis.findings[0].clone();
            finding.message = format!("finding {index}");
            finding
        })
        .collect();

    let json: serde_json::Value =
        serde_json::from_str(&render_json(&ReportContext::new(&analysis, &root, 0.0)).unwrap())
            .unwrap();
    assert_eq!(
        json["findings"].as_array().unwrap().len(),
        super::shared::MAX_BUFFERED_REPORT_FINDINGS
    );
    assert_eq!(json["findingsTruncated"], 1);

    let html = render_html(&ReportContext::new(&analysis, &root, 0.0)).unwrap();
    assert_eq!(
        html.matches("<article class=\"finding\"").count(),
        super::shared::MAX_BUFFERED_REPORT_FINDINGS
    );
    assert!(html.contains("1 additional findings"));
    assert!(html.contains(r#"class="truncation-notice" role="status""#));

    let corpus = CorpusCensus {
        definition_files: 1,
        definition_files_with_definitions: 1,
        definitions_extracted: analysis.definitions.len(),
        feature_files: 0,
        feature_files_parsed: 0,
        feature_files_without_steps: 0,
        incomplete: false,
    };
    let incomplete = crate::analysis::AnalysisCensus {
        truncated: true,
        candidate_comparisons_evaluated: 1,
        skipped_candidate_comparisons: 1,
        ..crate::analysis::AnalysisCensus::default()
    };
    let metadata = CliReportMetadata {
        corpus: &corpus,
        analysis: &incomplete,
        execution_successful: false,
    };
    let context = ReportContext::new(&analysis, &root, 0.0);
    let combined = html::render_html_context_with_metadata(&context, Some(&metadata)).unwrap();
    assert!(combined.contains(r#"class="truncation-notice" role="alert""#));
    assert!(combined.contains(r#"class="truncation-notice" role="status""#));

    let sarif: serde_json::Value =
        serde_json::from_str(&render_sarif(&ReportContext::new(&analysis, &root, 0.0)).unwrap())
            .unwrap();
    assert_eq!(
        sarif["runs"][0]["results"].as_array().unwrap().len(),
        super::shared::MAX_BUFFERED_REPORT_FINDINGS
    );
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["properties"]["findingsTruncated"],
        1
    );

    let mut terminal = Vec::new();
    write_terminal(&ReportContext::new(&analysis, &root, 0.0), &mut terminal).unwrap();
    let terminal = String::from_utf8(terminal).unwrap();
    assert_eq!(
        terminal
            .lines()
            .filter(|line| line.starts_with("  ["))
            .count(),
        super::shared::MAX_BUFFERED_REPORT_FINDINGS
    );
    assert!(terminal.contains("1 additional active findings omitted"));
}

#[test]
fn bounded_reports_never_displace_an_error_with_warnings() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    let template = analysis.findings[0].clone();
    analysis.findings = (0..super::shared::MAX_BUFFERED_REPORT_FINDINGS)
        .map(|index| Finding {
            severity: Severity::Warning,
            message: format!("legacy warning {index}"),
            primary: SourceLocation::new(root.join(format!("aaa/{index}.ts")), 1, 1, 1, 2),
            ..template.clone()
        })
        .collect();
    analysis.findings.push(Finding {
        message: "release-blocking error".to_owned(),
        primary: SourceLocation::new(root.join("zzz/error.ts"), 1, 1, 1, 2),
        ..template
    });

    let json: serde_json::Value =
        serde_json::from_str(&render_json(&ReportContext::new(&analysis, &root, 0.0)).unwrap())
            .unwrap();
    assert!(json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["message"] == "release-blocking error"));

    let html = render_html(&ReportContext::new(&analysis, &root, 0.0)).unwrap();
    assert!(html.contains("release-blocking error"));

    let sarif: serde_json::Value =
        serde_json::from_str(&render_sarif(&ReportContext::new(&analysis, &root, 0.0)).unwrap())
            .unwrap();
    assert!(sarif["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["message"]["text"] == "release-blocking error"));

    let mut terminal = Vec::new();
    write_terminal(&ReportContext::new(&analysis, &root, 0.0), &mut terminal).unwrap();
    assert!(String::from_utf8(terminal)
        .unwrap()
        .contains("release-blocking error"));
}

#[test]
fn terminal_groups_each_rule_once_and_removes_control_characters() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    let mut warning = analysis.findings[0].clone();
    warning.rule = Rule::NearDuplicateStep;
    warning.primary = SourceLocation::new(root.join("a.ts"), 1, 1, 1, 2);
    let mut duplicate = analysis.findings[0].clone();
    duplicate.primary = SourceLocation::new(root.join("z.ts"), 1, 1, 1, 2);
    duplicate.evidence.matcher_difference =
        "safe\u{1b}[31m\u{202e}\u{e0001}\u{e007f}spoof".to_owned();
    analysis.findings.extend([warning, duplicate]);

    let mut terminal = Vec::new();
    write_terminal(&ReportContext::new(&analysis, &root, 0.0), &mut terminal).unwrap();
    let terminal = String::from_utf8(terminal).unwrap();
    assert_eq!(terminal.matches("\nduplicate-matcher\n").count(), 1);
    assert_eq!(terminal.matches("\nnear-duplicate-step\n").count(), 1);
    assert!(terminal.contains("\n\nnear-duplicate-step\n"));
    assert!(!terminal.contains('\u{1b}'));
    assert!(!terminal.contains('\u{202e}'));
    assert!(!terminal.contains('\u{e0001}'));
    assert!(!terminal.contains('\u{e007f}'));
}

#[test]
fn reporters_preserve_finding_counts_and_information_across_formats() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);

    let mut warning = analysis.findings[0].clone();
    warning.rule = Rule::NearDuplicateStep;
    warning.severity = Severity::Warning;
    warning.message = "Matcher wording drifted".to_owned();
    warning.primary = SourceLocation::new(root.join("steps/c.ts"), 7, 2, 7, 30);
    warning.related = vec![SourceLocation::new(root.join("steps/d.ts"), 11, 4, 11, 32)];
    warning.evidence.matcher_difference = "`one item` ↔ `an item`".to_owned();
    warning.evidence.handler_evidence = "Handlers share one structure".to_owned();
    warning.suggested_action = "Use one phrase".to_owned();
    warning.evidence.comparison = None;

    let mut suppressed = warning.clone();
    suppressed.rule = Rule::UnusedDefinition;
    suppressed.message = "Suppressed legacy definition".to_owned();
    suppressed.suppression = Some(Suppression {
        reason: "Covered by migration baseline".to_owned(),
    });
    analysis.findings.extend([warning, suppressed]);

    let mut terminal = Vec::new();
    let context = ReportContext::new(&analysis, &root, 100.0);
    write_terminal(&context, &mut terminal).unwrap();
    let terminal = String::from_utf8(terminal).unwrap();
    let json: serde_json::Value = serde_json::from_str(&render_json(&context).unwrap()).unwrap();
    let html = render_html(&context).unwrap();
    let sarif: serde_json::Value = serde_json::from_str(&render_sarif(&context).unwrap()).unwrap();
    let jsonl = render_jsonl(&context).unwrap();
    let jsonl_records = jsonl
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();

    let embedded_start = r#"<script type="application/json" id="report-data">"#;
    let embedded_json = html
        .split_once(embedded_start)
        .unwrap()
        .1
        .split_once("</script>")
        .unwrap()
        .0;
    let html_data: serde_json::Value = serde_json::from_str(embedded_json).unwrap();
    let active_findings = analysis
        .findings
        .iter()
        .filter(|finding| finding.is_active())
        .collect::<Vec<_>>();
    let terminal_findings = terminal
        .lines()
        .filter(|line| line.starts_with("  ["))
        .count();
    let html_cards = html.matches("<article class=\"finding\"").count();

    assert_eq!(active_findings.len(), 2);
    assert_eq!(terminal_findings, active_findings.len());
    assert_eq!(json["summary"]["findings"], active_findings.len());
    assert_eq!(jsonl_records.len(), analysis.findings.len() + 1);
    assert_eq!(jsonl_records.last().unwrap()["type"], "summary");
    assert_eq!(jsonl_records.last().unwrap()["summary"], json["summary"]);
    assert_eq!(html_cards, active_findings.len());
    assert_eq!(
        sarif["runs"][0]["results"].as_array().unwrap().len(),
        active_findings.len()
    );
    assert!(terminal.contains("1 error, 1 warning, 1 suppressed"));
    assert!(html.contains("1 error · 1 warning · 1 suppressed"));
    assert_eq!(html_data, json);
    assert_eq!(json["findings"].as_array().unwrap().len(), 3);
    assert_eq!(json["summary"]["suppressed"], 1);
    assert!(
        terminal.contains("Duplication: 2 of 2 definitions (100.00%), threshold 100.00% — PASS")
    );
    assert!(html.contains("Duplication: 2 of 2 definitions (100.00%) · threshold 100.00% · PASS"));
    assert_eq!(json["summary"]["duplication"]["duplicatedDefinitions"], 2);
    assert_eq!(json["summary"]["duplication"]["totalDefinitions"], 2);
    assert_eq!(json["summary"]["duplication"]["percentage"], 100.0);
    assert_eq!(json["summary"]["duplication"]["threshold"], 100.0);
    assert_eq!(json["summary"]["duplication"]["passed"], true);
    let sarif_threshold = &sarif["runs"][0]["invocations"][0]["properties"];
    assert_eq!(sarif_threshold["threshold"], 100.0);
    assert_eq!(sarif_threshold["duplicatedDefinitions"], 2);
    assert_eq!(sarif_threshold["totalDefinitions"], 2);
    assert_eq!(sarif_threshold["duplicationPercentage"], 100.0);
    assert_eq!(sarif_threshold["thresholdPassed"], true);
    assert_eq!(
        sarif_threshold["contributingRules"],
        serde_json::json!(["duplicate-matcher"])
    );

    for finding in active_findings {
        for expected in [
            finding.rule.to_string(),
            finding.severity.to_string(),
            finding.message.clone(),
            finding.primary.display(&root),
            finding.evidence.matcher_difference.clone(),
            finding.evidence.handler_evidence.clone(),
            finding.suggested_action.clone(),
        ] {
            assert!(
                terminal.contains(&expected),
                "terminal report lost finding information: {expected}"
            );
        }
        for related in &finding.related {
            let expected = related.display(&root);
            assert!(
                terminal.contains(&expected),
                "terminal report lost related location: {expected}"
            );
        }
        assert!(sarif["runs"][0]["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| {
                result["ruleId"] == finding.rule.to_string()
                    && result["message"]["text"] == finding.message
                    && result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
                        == finding.primary.display(&root).split(':').next().unwrap()
            }));
        assert!(jsonl_records.iter().any(|record| {
            record["type"] == "finding"
                && record["rule"] == finding.rule.to_string()
                && record["message"] == finding.message
                && record["primary"]["path"]
                    == finding.primary.display(&root).split(':').next().unwrap()
        }));
    }
}

#[test]
fn public_report_writer_uses_one_context_for_every_configured_format() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            reporters: Some(vec![
                ReporterKind::Terminal,
                ReporterKind::Json,
                ReporterKind::Html,
                ReporterKind::Sarif,
            ]),
            output: Some(PathBuf::from("reports")),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    let analysis = result(&config.root);
    let metrics = ExecutionMetrics::new(1, 1, 1.0, 2.0, 3.0);
    let context = ReportContext::with_metrics(&analysis, &config.root, 50.0, &metrics);
    let mut terminal = Vec::new();
    let written = write_reports(&context, &config, &mut terminal).unwrap();

    assert_eq!(written.len(), 3);
    assert!(written.iter().all(|path| path.is_file()));
    let terminal = String::from_utf8(terminal).unwrap();
    assert!(terminal.contains("CukeDedup"));

    let jsonl_config = Config::load(
        directory.path(),
        ConfigOverrides {
            reporters: Some(vec![ReporterKind::Jsonl]),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    let mut jsonl = Vec::new();
    assert!(write_reports(&context, &jsonl_config, &mut jsonl)
        .unwrap()
        .is_empty());
    assert!(String::from_utf8(jsonl)
        .unwrap()
        .lines()
        .any(|line| line.contains("\"type\":\"summary\"")));
}

/// A Ruby suite analyzed through the Ruby adapter, keeping its temporary root alive.
struct RubyAnalysis {
    _directory: tempfile::TempDir,
    config: Config,
    result: AnalysisResult,
}

/// Extracts `source` as `steps/steps.rb` with the Ruby adapter and analyzes it, so every reporter
/// renders findings the Ruby frontend produced (paths, handler text, fingerprints, rules).
fn ruby_analysis(source: &str, overrides: ConfigOverrides) -> RubyAnalysis {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), overrides).unwrap();
    let path = config.root.join("steps/steps.rb");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, source).unwrap();
    let file = crate::source_adapter::SourceFile {
        path,
        language: crate::source_adapter::SourceLanguage::Ruby,
    };
    let extracted = crate::source_adapter::adapter_for_language(file.language)
        .extract(source, &file)
        .unwrap();
    let result =
        crate::analysis::analyze_with_diagnostics(extracted.definitions, Vec::new(), &config)
            .unwrap()
            .result;
    RubyAnalysis {
        _directory: directory,
        config,
        result,
    }
}

/// Renders `context` through the public plain terminal writer.
fn ruby_terminal(context: &ReportContext<'_>) -> String {
    let mut bytes = Vec::new();
    write_terminal(context, &mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}

/// Ruby source with an error, a warning, a suppressed duplicate and HTML-like handler text.
const RUBY_REPORT_SOURCE: &str = "Given('the item is visible') do\n  render('</script><script>bad()</script>')\nend\nGiven('the item is visible') do\n  safe\nend\n# cuke-dedup:ignore duplicate-matcher -- accepted\nGiven('legacy step') { old(1) }\nGiven('legacy step') { old(2) }\nGiven('one item is listed') { list(1) }\nGiven('an item is listed') { list(1) }\n";

/// Terminal output over Ruby findings: version header and spacing, colors that only wrap selected
/// text, singular wording without timing, footer write failures, one group per rule, and
/// stripped control and bidi characters from Ruby matcher text.
#[test]
fn ruby_findings_render_through_the_terminal_reporter() {
    let analysis = ruby_analysis(RUBY_REPORT_SOURCE, ConfigOverrides::default());
    let root = &analysis.config.root;
    let timings = ExecutionMetrics::new(1, 1, 1.0, 2.0, 3.0);
    let context = ReportContext::with_metrics(&analysis.result, root, 100.0, &timings);
    let plain = ruby_terminal(&context);
    assert!(plain.starts_with(&format!("CukeDedup v{}\n\n", env!("CARGO_PKG_VERSION"))));
    assert!(plain.contains(
        "duplicate-matcher\n\n  [error] Two step definitions use the same effective matcher"
    ));
    assert!(plain.contains("    --> steps/steps.rb:1:1"));
    assert!(plain.contains("\n\n  [warning] Matcher wording is very close"));
    assert!(plain.ends_with("Found 3 findings.\nDetection time: 6.0 ms\n"));
    assert_eq!(plain.matches("\nduplicate-matcher\n").count(), 1);
    assert_eq!(plain.matches("\nnear-duplicate-step\n").count(), 1);

    let mut colored_bytes = Vec::new();
    super::terminal::write_terminal_with_color(&context, &mut colored_bytes, true).unwrap();
    let colored = String::from_utf8(colored_bytes).unwrap();
    assert!(colored.contains("    --> \u{1b}[94msteps/steps.rb:1:1\u{1b}[0m"));
    let uncolored = [
        "\u{1b}[31m",
        "\u{1b}[33m",
        "\u{1b}[1;36m",
        "\u{1b}[94m",
        "\u{1b}[90m",
        "\u{1b}[0m",
    ]
    .into_iter()
    .fold(colored, |text, code| text.replace(code, ""));
    assert_eq!(uncolored, plain);

    for marker in ["Found 3 findings.", "Detection time:"] {
        let remaining = plain.find(marker).unwrap();
        let error = write_terminal(&context, &mut LimitedWriter { remaining }).unwrap_err();
        let kind = error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind);
        assert_eq!(kind, Some(std::io::ErrorKind::BrokenPipe), "{marker}");
    }

    let single = ruby_analysis(
        "Given('same step') { first }\nGiven('same step') { second }\n",
        ConfigOverrides::default(),
    );
    let no_timing = ruby_terminal(&ReportContext::new(
        &single.result,
        &single.config.root,
        100.0,
    ));
    assert!(no_timing.ends_with("Found 1 finding.\n"));
    assert!(!no_timing.contains("Detection time:"));

    // Two duplicate-matcher findings with other rules' findings between them in source order:
    // one heading per rule, not per finding.
    let grouped = ruby_analysis(
        "Given('alpha step') { a1 }\nGiven('alpha step') { a2 }\nGiven('one item is listed') { list(1) }\nGiven('an item is listed') { list(1) }\nGiven('omega step') { o1 }\nGiven('omega step') { o2 }\n",
        ConfigOverrides::default(),
    );
    let grouped_terminal = ruby_terminal(&ReportContext::new(
        &grouped.result,
        &grouped.config.root,
        0.0,
    ));
    assert_eq!(
        grouped_terminal
            .matches("[error] Two step definitions use the same effective matcher")
            .count(),
        2
    );
    for heading in [
        "duplicate-matcher",
        "duplicate-handler",
        "near-duplicate-step",
    ] {
        assert_eq!(
            grouped_terminal.matches(&format!("\n{heading}\n")).count(),
            1,
            "{heading}"
        );
    }

    let spoofed = ruby_analysis(
        "Given(\"safe \u{1b}[31m\u{202e}\u{e0001}spoof\") { first }\nGiven(\"safe \u{1b}[31m\u{202e}\u{e0001}spoof\") { second }\n",
        ConfigOverrides::default(),
    );
    let terminal = ruby_terminal(&ReportContext::new(
        &spoofed.result,
        &spoofed.config.root,
        0.0,
    ));
    assert!(terminal.contains("duplicate-matcher"));
    let matcher = &spoofed.result.definitions[0].matcher;
    for hidden in ['\u{1b}', '\u{202e}', '\u{e0001}'] {
        assert!(matcher.contains(hidden), "{hidden:?}");
        assert!(!terminal.contains(hidden), "{hidden:?}");
    }
}

/// Machine formats over Ruby findings: versioned JSON with relative `.rb` paths and the strict
/// default summary, self-contained JSONL records with truncation of an over-long Ruby matcher,
/// active-only SARIF with stable locations and documented rules, escaped self-contained HTML,
/// one finding count across formats, and the public writer for every configured format.
#[test]
fn ruby_findings_render_through_every_machine_reporter() {
    let analysis = ruby_analysis(RUBY_REPORT_SOURCE, ConfigOverrides::default());
    let root = &analysis.config.root;
    let context = ReportContext::new(&analysis.result, root, 0.0);
    let summary = Summary::from_result(&analysis.result);
    assert_eq!(summary.findings, 3);
    assert_eq!(summary.duplication.threshold, 0.0);
    assert!(!summary.duplication.passed);

    let json_text = render_json(&context).unwrap();
    assert!(json_text.contains("\"schemaVersion\": \"3\""));
    assert!(json_text.contains("\"path\": \"steps/steps.rb\""));
    assert!(!json_text.contains(&root.display().to_string()));
    let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
    assert_eq!(json["summary"]["suppressed"], 1);
    assert_eq!(json["findings"].as_array().unwrap().len(), 4);
    assert!(json["findings"][0]["evidence"]
        .get("matcherSimilarity")
        .is_some());
    assert_eq!(
        json["findings"][0]["evidence"]["comparison"]["leftHandler"],
        "do\n  render('</script><script>bad()</script>')\nend"
    );

    let jsonl = render_jsonl(&context).unwrap();
    let records: Vec<serde_json::Value> = jsonl
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), analysis.result.findings.len() + 1);
    assert!(records[..records.len() - 1].iter().all(|record| {
        record["schemaVersion"] == JSONL_SCHEMA_VERSION
            && record["type"] == "finding"
            && record["primary"]["path"] == "steps/steps.rb"
            && record["fingerprint"].as_str().unwrap().len() == 32
    }));
    assert!(records
        .iter()
        .any(|record| record["active"] == false && record["suppression"]["reason"] == "accepted"));
    let last = records.last().unwrap();
    assert_eq!(last["type"], "summary");
    assert_eq!(last["summary"], json["summary"]);
    assert_eq!(last["recordCount"], records.len());
    assert_eq!(last["truncated"], false);

    let long_matcher = "界".repeat(JSONL_TEXT_LIMIT_CHARS + 1);
    let long = ruby_analysis(
        &format!("Given('{long_matcher}') {{ first }}\nGiven('{long_matcher}') {{ second }}\n"),
        ConfigOverrides::default(),
    );
    let long_jsonl =
        render_jsonl(&ReportContext::new(&long.result, &long.config.root, 0.0)).unwrap();
    let long_finding: serde_json::Value =
        serde_json::from_str(long_jsonl.lines().next().unwrap()).unwrap();
    assert_eq!(long_finding["rule"], "duplicate-matcher");
    assert!(long_finding["truncatedFields"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("evidence.comparison.leftMatcher")));
    assert_eq!(
        long_finding["evidence"]["comparison"]["leftMatcher"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        JSONL_TEXT_LIMIT_CHARS
    );

    let sarif: serde_json::Value = serde_json::from_str(&render_sarif(&context).unwrap()).unwrap();
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    for result in results {
        assert_eq!(
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "steps/steps.rb"
        );
        let fingerprint = result["partialFingerprints"]["cukeDedupFingerprint/v3"]
            .as_str()
            .unwrap();
        assert_eq!(fingerprint.len(), 32);
    }
    let descriptors = sarif["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .unwrap();
    for rule in [
        Rule::DuplicateMatcher,
        Rule::DuplicateHandler,
        Rule::NearDuplicateStep,
    ] {
        assert!(descriptors
            .iter()
            .any(|descriptor| descriptor["id"] == rule.as_str()
                && descriptor["helpUri"] == rule.documentation_url()));
    }

    let html = render_html(&context).unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("&lt;/script&gt;&lt;script&gt;bad()&lt;/script&gt;"));
    assert!(!html.contains("</script><script>bad()"));
    assert_eq!(html.matches("<article class=\"finding\"").count(), 3);
    assert!(html.contains("2 errors · 1 warning · 1 suppressed"));
    let embedded = html
        .split_once(r#"<script type="application/json" id="report-data">"#)
        .unwrap()
        .1
        .split_once("</script>")
        .unwrap()
        .0;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(embedded).unwrap(),
        json
    );

    let terminal = ruby_terminal(&context);
    assert!(terminal.contains("2 errors, 1 warning, 1 suppressed"));
    assert_eq!(
        terminal
            .lines()
            .filter(|line| line.starts_with("  ["))
            .count(),
        3
    );

    let writer = ruby_analysis(
        RUBY_REPORT_SOURCE,
        ConfigOverrides {
            reporters: Some(
                [
                    ReporterKind::Terminal,
                    ReporterKind::Json,
                    ReporterKind::Html,
                    ReporterKind::Sarif,
                ]
                .to_vec(),
            ),
            output: Some(PathBuf::from("reports")),
            ..ConfigOverrides::default()
        },
    );
    let writer_context = ReportContext::new(&writer.result, &writer.config.root, 0.0);
    let mut writer_terminal = Vec::new();
    let written = write_reports(&writer_context, &writer.config, &mut writer_terminal).unwrap();
    assert_eq!(written.len(), 3);
    for path in &written {
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .contains("steps/steps.rb"));
    }
    assert!(String::from_utf8(writer_terminal)
        .unwrap()
        .contains("steps/steps.rb:1:1"));
}

/// Bounds over Ruby populations: a 300-member Ruby duplicate cluster is capped with an omission
/// signal and keeps its cluster evidence, more than the buffered cap of Ruby findings is
/// truncated with a notice, and the one error survives a full page of Ruby warnings.
#[test]
fn ruby_findings_respect_every_reporter_bound() {
    let member_count = 300;
    let cluster = ruby_analysis(
        &(0..member_count)
            .map(|index| format!("Given('same step') {{ work({index}) }}\n"))
            .collect::<String>(),
        ConfigOverrides::default(),
    );
    assert_eq!(cluster.result.findings.len(), 1);
    let context = ReportContext::new(&cluster.result, &cluster.config.root, 100.0);
    let json: serde_json::Value = serde_json::from_str(&render_json(&context).unwrap()).unwrap();
    let finding = &json["findings"][0];
    let reported_cap = super::shared::MAX_REPORTED_CLUSTER_MEMBERS;
    assert_eq!(
        finding["related"].as_array().unwrap().len(),
        reported_cap - 1
    );
    assert_eq!(finding["relatedLocationsTruncated"], true);
    let cluster_evidence = &finding["evidence"]["cluster"];
    assert_eq!(cluster_evidence["memberCount"], member_count);
    assert_eq!(cluster_evidence["membersTruncated"], true);
    let fingerprints = cluster_evidence["definitionFingerprints"]
        .as_array()
        .unwrap();
    assert_eq!(fingerprints.len(), reported_cap);
    let jsonl: serde_json::Value =
        serde_json::from_str(render_jsonl(&context).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(jsonl["relatedLocationsTruncated"], true);
    assert_eq!(jsonl["evidence"]["cluster"]["memberCount"], member_count);
    assert_eq!(jsonl["evidence"]["cluster"]["membersTruncated"], true);
    let sarif: serde_json::Value = serde_json::from_str(&render_sarif(&context).unwrap()).unwrap();
    assert_eq!(
        sarif["runs"][0]["results"][0]["properties"]["relatedLocationsTruncated"],
        true
    );
    assert_eq!(
        sarif["runs"][0]["results"][0]["properties"]["clusterMembersTruncated"],
        true
    );
    for rendered in [render_html(&context).unwrap(), ruby_terminal(&context)] {
        assert!(rendered.contains("more locations omitted"));
    }

    let cap = super::shared::MAX_BUFFERED_REPORT_FINDINGS;
    let many = ruby_analysis(
        &(0..=cap)
            .map(|index| {
                format!("Given('step {index}') {{ alpha_{index} }}\nGiven('step {index}') {{ beta_{index} }}\n")
            })
            .collect::<String>(),
        ConfigOverrides::default(),
    );
    assert_eq!(many.result.findings.len(), cap + 1);
    let many_context = ReportContext::new(&many.result, &many.config.root, 0.0);
    let json: serde_json::Value =
        serde_json::from_str(&render_json(&many_context).unwrap()).unwrap();
    assert_eq!(json["findings"].as_array().unwrap().len(), cap);
    assert_eq!(json["findingsTruncated"], 1);
    let sarif: serde_json::Value =
        serde_json::from_str(&render_sarif(&many_context).unwrap()).unwrap();
    assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), cap);
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["properties"]["findingsTruncated"],
        1
    );
    let html = render_html(&many_context).unwrap();
    assert_eq!(html.matches("<article class=\"finding\"").count(), cap);
    assert!(html.contains("1 additional findings"));
    assert!(ruby_terminal(&many_context).contains("1 additional active findings omitted"));
    assert_eq!(
        render_jsonl(&many_context).unwrap().lines().count(),
        cap + 2
    );

    let mut warnings = (0..cap)
        .map(|index| {
            format!("Given('step {index}') {{ alpha_{index} }}\nGiven('step {index}') {{ beta_{index} }}\n")
        })
        .collect::<String>();
    warnings.push_str("Given('release one') { blocking }\nGiven('release two') { blocking }\n");
    let displaced = ruby_analysis(
        &warnings,
        ConfigOverrides {
            rules: [
                (Rule::DuplicateMatcher, Severity::Warning),
                (Rule::NearDuplicateStep, Severity::Off),
            ]
            .into_iter()
            .collect(),
            ..ConfigOverrides::default()
        },
    );
    let error_line = 2 * cap + 1;
    let errors = displaced
        .result
        .findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].primary.line, error_line);
    assert!(displaced.result.findings.len() > cap);
    let context = ReportContext::new(&displaced.result, &displaced.config.root, 0.0);
    let expected = format!("steps/steps.rb:{error_line}:1");
    let json = render_json(&context).unwrap();
    assert!(json.contains(&format!("\"line\": {error_line},")));
    assert!(render_html(&context).unwrap().contains(&expected));
    let sarif: serde_json::Value = serde_json::from_str(&render_sarif(&context).unwrap()).unwrap();
    assert!(sarif["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| result["level"] == "error"));
    assert!(ruby_terminal(&context).contains(&expected));
}
