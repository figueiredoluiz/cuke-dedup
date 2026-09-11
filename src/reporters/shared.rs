use super::{CliReportMetadata, CorpusCensus, ReportContext, Summary, JSON_SCHEMA_VERSION};
use crate::analysis::AnalysisCensus;
use crate::model::{
    AnalysisResult, DefinitionCluster, Finding, FindingEvidence, Rule, Severity, SourceLocation,
    Suppression,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::Path;

#[cfg(not(test))]
pub(super) const MAX_BUFFERED_REPORT_FINDINGS: usize = 10_000;
#[cfg(test)]
pub(super) const MAX_BUFFERED_REPORT_FINDINGS: usize = 100;
#[cfg(not(test))]
pub(super) const MAX_REPORTED_CLUSTER_MEMBERS: usize = 256;
#[cfg(test)]
pub(super) const MAX_REPORTED_CLUSTER_MEMBERS: usize = 16;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct JsonReport<'a> {
    schema_version: &'static str,
    summary: Summary,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<&'a super::ExecutionMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    corpus: Option<CorpusCensus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    analysis: Option<AnalysisCensus>,
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
    related_locations_truncated: bool,
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
    report_value_with_metadata(context, None)
}

pub(super) fn report_value_with_metadata<'a>(
    context: &ReportContext<'a>,
    metadata: Option<&CliReportMetadata<'_>>,
) -> JsonReport<'a> {
    let selected = bounded_findings(context.result, false);
    JsonReport {
        schema_version: JSON_SCHEMA_VERSION,
        summary: context.summary.clone(),
        metrics: context.metrics,
        corpus: metadata.map(|metadata| metadata.corpus.clone()),
        analysis: metadata.map(|metadata| metadata.analysis.clone()),
        findings: selected
            .findings
            .into_iter()
            .map(|finding| convert_finding(finding, context.root))
            .collect(),
        findings_truncated: selected.truncated,
    }
}

fn convert_finding(finding: &Finding, root: &Path) -> ReportFinding {
    let mut comparison = finding.evidence.comparison.clone();
    if let Some(comparison) = comparison.as_mut() {
        comparison.left_matcher = report_safe(&comparison.left_matcher);
        comparison.right_matcher = report_safe(&comparison.right_matcher);
        comparison.left_handler = report_safe(&comparison.left_handler);
        comparison.right_handler = report_safe(&comparison.right_handler);
        comparison.matcher_diff.prefix = report_safe(&comparison.matcher_diff.prefix);
        comparison.matcher_diff.left_change = report_safe(&comparison.matcher_diff.left_change);
        comparison.matcher_diff.right_change = report_safe(&comparison.matcher_diff.right_change);
        comparison.matcher_diff.suffix = report_safe(&comparison.matcher_diff.suffix);
    }
    let cluster = finding.evidence.cluster.as_ref().map(|cluster| {
        let definition_fingerprints = cluster
            .definition_fingerprints
            .iter()
            .take(MAX_REPORTED_CLUSTER_MEMBERS)
            .cloned()
            .collect::<Vec<_>>();
        DefinitionCluster {
            member_count: cluster.member_count,
            members_truncated: cluster.members_truncated
                || definition_fingerprints.len() < cluster.member_count,
            definition_fingerprints,
            pair_findings_collapsed: cluster.pair_findings_collapsed,
        }
    });
    let evidence = FindingEvidence {
        matcher_similarity: finding.evidence.matcher_similarity,
        handler_similarity: finding.evidence.handler_similarity,
        matcher_difference: report_safe(&finding.evidence.matcher_difference),
        handler_evidence: report_safe(&finding.evidence.handler_evidence),
        comparison,
        cluster,
    };
    let retained_related = MAX_REPORTED_CLUSTER_MEMBERS.saturating_sub(1);
    ReportFinding {
        rule: finding.rule,
        severity: finding.severity,
        message: report_safe(&finding.message),
        primary: convert_location(&finding.primary, root),
        related: finding
            .related
            .iter()
            .take(retained_related)
            .map(|location| convert_location(location, root))
            .collect(),
        related_locations_truncated: finding.related.len() > retained_related,
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

/// Removes characters that could rewrite or misrepresent output when rendered.
///
/// Report text is attacker-controlled: it comes from matchers, handlers, and paths in the
/// analyzed repository. Control characters can rewrite a terminal line, and bidirectional or
/// invisible-format characters can make one string display as another.
fn sanitize(value: &str, keep_whitespace: bool) -> String {
    value
        .chars()
        .filter(|character| {
            let allowed_control = keep_whitespace && matches!(*character, '\n' | '\r' | '\t');
            (!character.is_control() || allowed_control)
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

/// Sanitizes text for a terminal, where even newlines and tabs would break line-oriented output.
pub(super) fn terminal_safe(value: &str) -> String {
    sanitize(value, false)
}

/// Sanitizes text for structured reports, which preserve intentional line breaks and tabs.
pub(super) fn report_safe(value: &str) -> String {
    sanitize(value, true)
}
