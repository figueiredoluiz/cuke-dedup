use super::{CliReportMetadata, CorpusCensus, ReportContext, Summary, JSON_SCHEMA_VERSION};
use crate::analysis::AnalysisCensus;
use crate::model::{
    AnalysisResult, DefinitionCluster, DefinitionComparison, Finding, FindingEvidence, MatcherDiff,
    Rule, Severity, SourceLocation, Suppression,
};
use crate::resource_limits::{
    sample_text, LimitedWriter, MAX_BUFFERED_REPORT_BYTES, MAX_REPORT_FIELD_BYTES,
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

/// Selects up to `MAX_BUFFERED_REPORT_FINDINGS` findings by `finding_priority`, keeping input
/// order within each priority, stopping when the conservative output byte budget is exhausted.
///
/// `active_only` excludes inactive findings first; `truncated` counts the eligible findings left
/// out.
pub(super) fn bounded_findings<'a>(
    result: &'a AnalysisResult,
    root: &Path,
    active_only: bool,
) -> BoundedFindings<'a> {
    let eligible = |finding: &&Finding| !active_only || finding.is_active();
    let total = result.findings.iter().filter(eligible).count();
    let mut findings = Vec::with_capacity(total.min(MAX_BUFFERED_REPORT_FINDINGS));
    // Reserve space for the template, summaries, rule descriptors, and CLI metadata.
    let mut remaining = MAX_BUFFERED_REPORT_BYTES - 64 * 1024;
    for priority in 0..4 {
        for finding in result.findings.iter().filter(eligible) {
            if finding_priority(finding) == priority {
                // Count bounded, sanitized evidence with no serialized buffer. A conservative
                // multiplier covers HTML's visible and embedded copies (including escaping and
                // repeated diff segments); fixed overhead covers cards and SARIF locations.
                let converted = convert_finding(finding, root);
                let mut counter = LimitedWriter {
                    inner: std::io::sink(),
                    remaining: MAX_BUFFERED_REPORT_BYTES,
                };
                let fits = serde_json::to_writer(&mut counter, &converted).is_ok();
                let cost = (MAX_BUFFERED_REPORT_BYTES - counter.remaining)
                    .saturating_mul(18)
                    .saturating_add(4096);
                if !fits || cost > remaining || findings.len() == MAX_BUFFERED_REPORT_FINDINGS {
                    return BoundedFindings {
                        truncated: total - findings.len(),
                        findings,
                    };
                }
                remaining -= cost;
                findings.push(finding);
            }
        }
    }
    BoundedFindings {
        findings,
        truncated: 0,
    }
}

/// Ranks a finding for report order: 0 active error, 1 other active, 2 inactive error, 3 other.
fn finding_priority(finding: &Finding) -> u8 {
    match (finding.is_active(), finding.severity) {
        (true, Severity::Error) => 0,
        (true, _) => 1,
        (false, Severity::Error) => 2,
        (false, _) => 3,
    }
}

/// Builds the JSON report value without corpus or analysis censuses.
pub(super) fn report_value<'a>(context: &ReportContext<'a>) -> JsonReport<'a> {
    report_value_with_metadata(context, None)
}

/// Builds the JSON report value from up to `MAX_BUFFERED_REPORT_FINDINGS` findings, active before
/// inactive, adding corpus and analysis censuses when `metadata` is given.
pub(super) fn report_value_with_metadata<'a>(
    context: &ReportContext<'a>,
    metadata: Option<&CliReportMetadata<'_>>,
) -> JsonReport<'a> {
    let selected = bounded_findings(context.result, context.root, false);
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

/// Converts a finding for structured reports, sanitizing text with `report_safe`.
///
/// Keeps up to `MAX_REPORTED_CLUSTER_MEMBERS` cluster fingerprints and one fewer related
/// locations, flagging either cut. Every string is sampled before copying and sanitizing.
fn convert_finding(finding: &Finding, root: &Path) -> ReportFinding {
    let comparison = finding
        .evidence
        .comparison
        .as_ref()
        .map(|comparison| DefinitionComparison {
            left_fingerprint: report_safe(&comparison.left_fingerprint),
            right_fingerprint: report_safe(&comparison.right_fingerprint),
            left_matcher: report_safe(&comparison.left_matcher),
            right_matcher: report_safe(&comparison.right_matcher),
            left_handler: report_safe(&comparison.left_handler),
            right_handler: report_safe(&comparison.right_handler),
            matcher_diff: MatcherDiff {
                prefix: report_safe(&comparison.matcher_diff.prefix),
                left_change: report_safe(&comparison.matcher_diff.left_change),
                right_change: report_safe(&comparison.matcher_diff.right_change),
                suffix: report_safe(&comparison.matcher_diff.suffix),
            },
        });
    let cluster = finding.evidence.cluster.as_ref().map(|cluster| {
        let definition_fingerprints = cluster
            .definition_fingerprints
            .iter()
            .take(MAX_REPORTED_CLUSTER_MEMBERS)
            .map(|fingerprint| report_safe(fingerprint))
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

/// Converts a location with its path made relative to `root` when under it, `/`-separated, and
/// sanitized with `report_safe`.
pub(super) fn convert_location(location: &SourceLocation, root: &Path) -> ReportLocation {
    let relative_path = location
        .path
        .strip_prefix(root)
        .unwrap_or(&location.path)
        .to_string_lossy();
    let relative_path = sample_text(&relative_path, MAX_REPORT_FIELD_BYTES).replace('\\', "/");
    ReportLocation {
        path: report_safe(&relative_path),
        line: location.line,
        column: location.column,
        end_line: location.end_line,
        end_column: location.end_column,
    }
}

/// Formats a sampled root-relative path and its source position without copying the full path.
pub(super) fn display_location(location: &SourceLocation, root: &Path) -> String {
    let location = convert_location(location, root);
    format!("{}:{}:{}", location.path, location.line, location.column)
}

/// Creates the parent directories of `path`; does nothing when `path` has no parent.
pub(super) fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create report directory {}", parent.display()))?;
    }
    Ok(())
}

/// Formats `count` and `singular`, appending `s` unless `count` is 1.
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
    sample_text(value, MAX_REPORT_FIELD_BYTES)
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

/// Serializes through a capped writer so escape expansion cannot allocate an oversized output.
pub(super) fn bounded_json(value: &impl Serialize, pretty: bool) -> Result<String> {
    let mut writer = LimitedWriter {
        inner: Vec::new(),
        remaining: MAX_BUFFERED_REPORT_BYTES,
    };
    if pretty {
        serde_json::to_writer_pretty(&mut writer, value)?;
    } else {
        serde_json::to_writer(&mut writer, value)?;
    }
    Ok(String::from_utf8(writer.inner)?)
}
