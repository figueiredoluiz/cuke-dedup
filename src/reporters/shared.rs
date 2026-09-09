use super::{CorpusCensus, ReportContext, Summary, JSON_SCHEMA_VERSION};
use crate::model::{
    AnalysisResult, Finding, FindingEvidence, Rule, Severity, SourceLocation, Suppression,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::Path;

#[cfg(not(test))]
pub(super) const MAX_BUFFERED_REPORT_FINDINGS: usize = 10_000;
#[cfg(test)]
pub(super) const MAX_BUFFERED_REPORT_FINDINGS: usize = 100;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct JsonReport<'a> {
    schema_version: &'static str,
    summary: Summary,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<&'a super::ExecutionMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    corpus: Option<&'a CorpusCensus>,
    findings: Vec<ReportFinding>,
    findings_truncated: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportFinding {
    rule: Rule,
    severity: Severity,
    message: String,
    primary: ReportLocation,
    related: Vec<ReportLocation>,
    evidence: FindingEvidence,
    suggested_action: String,
    suppression: Option<Suppression>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReportLocation {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) column: usize,
    pub(super) end_line: usize,
    pub(super) end_column: usize,
}

pub(super) struct BoundedFindings<'a> {
    pub(super) findings: Vec<&'a Finding>,
    pub(super) truncated: usize,
}

pub(super) fn bounded_findings(result: &AnalysisResult, active_only: bool) -> BoundedFindings<'_> {
    let eligible = |finding: &&Finding| !active_only || finding.is_active();
    let total = result.findings.iter().filter(eligible).count();
    let mut findings = Vec::with_capacity(total.min(MAX_BUFFERED_REPORT_FINDINGS));
    for priority in 0..4 {
        for finding in result.findings.iter().filter(eligible) {
            if finding_priority(finding) == priority {
                findings.push(finding);
                if findings.len() == MAX_BUFFERED_REPORT_FINDINGS {
                    return BoundedFindings {
                        findings,
                        truncated: total.saturating_sub(MAX_BUFFERED_REPORT_FINDINGS),
                    };
                }
            }
        }
    }
    BoundedFindings {
        findings,
        truncated: 0,
    }
}

fn finding_priority(finding: &Finding) -> u8 {
    match (finding.is_active(), finding.severity) {
        (true, Severity::Error) => 0,
        (true, _) => 1,
        (false, Severity::Error) => 2,
        (false, _) => 3,
    }
}

pub(super) fn report_value<'a>(context: &ReportContext<'a>) -> JsonReport<'a> {
    report_value_with_census(context, None)
}

pub(super) fn report_value_with_census<'a>(
    context: &ReportContext<'a>,
    corpus: Option<&'a CorpusCensus>,
) -> JsonReport<'a> {
    let selected = bounded_findings(context.result, false);
    JsonReport {
        schema_version: JSON_SCHEMA_VERSION,
        summary: context.summary.clone(),
        metrics: context.metrics,
        corpus,
        findings: selected
            .findings
            .into_iter()
            .map(|finding| convert_finding(finding, context.root))
            .collect(),
        findings_truncated: selected.truncated,
    }
}

fn convert_finding(finding: &Finding, root: &Path) -> ReportFinding {
    let mut evidence = finding.evidence.clone();
    evidence.matcher_difference = report_safe(&evidence.matcher_difference);
    evidence.handler_evidence = report_safe(&evidence.handler_evidence);
    if let Some(comparison) = evidence.comparison.as_mut() {
        comparison.left_matcher = report_safe(&comparison.left_matcher);
        comparison.right_matcher = report_safe(&comparison.right_matcher);
        comparison.left_handler = report_safe(&comparison.left_handler);
        comparison.right_handler = report_safe(&comparison.right_handler);
        comparison.matcher_diff.prefix = report_safe(&comparison.matcher_diff.prefix);
        comparison.matcher_diff.left_change = report_safe(&comparison.matcher_diff.left_change);
        comparison.matcher_diff.right_change = report_safe(&comparison.matcher_diff.right_change);
        comparison.matcher_diff.suffix = report_safe(&comparison.matcher_diff.suffix);
    }
    ReportFinding {
        rule: finding.rule,
        severity: finding.severity,
        message: report_safe(&finding.message),
        primary: convert_location(&finding.primary, root),
        related: finding
            .related
            .iter()
            .map(|location| convert_location(location, root))
            .collect(),
        evidence,
        suggested_action: report_safe(&finding.suggested_action),
        suppression: finding.suppression.as_ref().map(|suppression| Suppression {
            reason: report_safe(&suppression.reason),
        }),
    }
}

pub(super) fn convert_location(location: &SourceLocation, root: &Path) -> ReportLocation {
    let relative_path = location
        .path
        .strip_prefix(root)
        .unwrap_or(&location.path)
        .to_string_lossy()
        .replace('\\', "/");
    ReportLocation {
        path: report_safe(&relative_path),
        line: location.line,
        column: location.column,
        end_line: location.end_line,
        end_column: location.end_column,
    }
}

pub(super) fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create report directory {}", parent.display()))?;
    }
    Ok(())
}

pub(super) fn count_label(count: usize, singular: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {singular}{suffix}")
}

pub(super) fn terminal_safe(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    *character,
                    '\u{00ad}'
                        | '\u{061c}'
                        | '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2060}'..='\u{2069}'
                        | '\u{feff}'
                        | '\u{e0000}'..='\u{e007f}'
                )
        })
        .collect()
}

pub(super) fn report_safe(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            (!character.is_control() || matches!(*character, '\n' | '\r' | '\t'))
                && !matches!(
                    *character,
                    '\u{00ad}'
                        | '\u{061c}'
                        | '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2060}'..='\u{2069}'
                        | '\u{feff}'
                        | '\u{e0000}'..='\u{e007f}'
                )
        })
        .collect()
}
