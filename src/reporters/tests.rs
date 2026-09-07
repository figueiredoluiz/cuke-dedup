use super::jsonl::JSONL_TEXT_LIMIT_CHARS;
use super::*;
use crate::model::{
    AnalysisResult, DefinitionComparison, Finding, FindingEvidence, Framework, HandlerFingerprint,
    MatcherDiff, MatcherKind, Rule, Severity, SourceLocation, StepDefinition, Suppression,
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
    let json = render_json(&analysis, &root).unwrap();
    assert!(json.contains("\"schemaVersion\": \"1\""));
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
fn jsonl_emits_self_contained_findings_and_a_final_summary() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    analysis.findings[0]
        .evidence
        .comparison
        .as_mut()
        .unwrap()
        .left_handler = "界".repeat(JSONL_TEXT_LIMIT_CHARS + 1);

    let jsonl = render_jsonl_with_threshold(&analysis, &root, 100.0).unwrap();
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
    assert_eq!(finding["fingerprint"].as_str().unwrap().len(), 16);
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
        serde_json::from_str(&render_sarif_with_threshold(&analysis, &root, 100.0).unwrap())
            .unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(sarif["runs"][0]["columnKind"], "unicodeCodePoints");
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
    assert!(
        sarif["runs"][0]["results"][0]["partialFingerprints"]["cukeDedupFingerprint/v1"]
            .as_str()
            .is_some()
    );
}

#[test]
fn html_is_self_contained_and_escapes_finding_text() {
    let root = PathBuf::from("/repo");
    let mut analysis = result(&root);
    analysis.findings[0]
        .message
        .push_str(" safe\u{1b}[31m\u{202e}spoof");
    let html = render_html(&analysis, &root).unwrap();
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
        serde_json::from_str(&render_json(&analysis, &root).unwrap()).unwrap();
    assert_eq!(
        json["findings"].as_array().unwrap().len(),
        super::shared::MAX_BUFFERED_REPORT_FINDINGS
    );
    assert_eq!(json["findingsTruncated"], 1);

    let html = render_html(&analysis, &root).unwrap();
    assert_eq!(
        html.matches("<article class=\"finding\"").count(),
        super::shared::MAX_BUFFERED_REPORT_FINDINGS
    );
    assert!(html.contains("1 additional findings"));

    let sarif: serde_json::Value =
        serde_json::from_str(&render_sarif(&analysis, &root).unwrap()).unwrap();
    assert_eq!(
        sarif["runs"][0]["results"].as_array().unwrap().len(),
        super::shared::MAX_BUFFERED_REPORT_FINDINGS
    );
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["properties"]["findingsTruncated"],
        1
    );

    let mut terminal = Vec::new();
    write_terminal(&analysis, &root, &mut terminal).unwrap();
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
        serde_json::from_str(&render_json(&analysis, &root).unwrap()).unwrap();
    assert!(json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["message"] == "release-blocking error"));

    let html = render_html(&analysis, &root).unwrap();
    assert!(html.contains("release-blocking error"));

    let sarif: serde_json::Value =
        serde_json::from_str(&render_sarif(&analysis, &root).unwrap()).unwrap();
    assert!(sarif["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["message"]["text"] == "release-blocking error"));

    let mut terminal = Vec::new();
    write_terminal(&analysis, &root, &mut terminal).unwrap();
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
    write_terminal(&analysis, &root, &mut terminal).unwrap();
    let terminal = String::from_utf8(terminal).unwrap();
    assert_eq!(terminal.matches("\nduplicate-matcher\n").count(), 1);
    assert_eq!(terminal.matches("\nnear-duplicate-step\n").count(), 1);
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
    write_terminal_with_threshold(&analysis, &root, 100.0, &mut terminal).unwrap();
    let terminal = String::from_utf8(terminal).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&render_json_with_threshold(&analysis, &root, 100.0).unwrap())
            .unwrap();
    let html = render_html_with_threshold(&analysis, &root, 100.0).unwrap();
    let sarif: serde_json::Value =
        serde_json::from_str(&render_sarif_with_threshold(&analysis, &root, 100.0).unwrap())
            .unwrap();
    let jsonl = render_jsonl_with_threshold(&analysis, &root, 100.0).unwrap();
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
