use super::shared::{convert_location, report_safe, ReportLocation};
use super::{
    CliReportMetadata, CorpusCensus, ExecutionMetrics, ReportContext, Summary, JSONL_SCHEMA_VERSION,
};
use crate::analysis::AnalysisCensus;
use crate::model::{Finding, Rule, Severity};
use anyhow::{Context, Result};
use serde::Serialize;
use std::io::{BufWriter, Write};

pub(super) const JSONL_TEXT_LIMIT_CHARS: usize = 2_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonlFinding {
    schema_version: &'static str,
    #[serde(rename = "type")]
    record_type: &'static str,
    tool_version: &'static str,
    fingerprint: String,
    active: bool,
    contributes_to_threshold: bool,
    rule: Rule,
    severity: Severity,
    message: String,
    primary: ReportLocation,
    related: Vec<ReportLocation>,
    evidence: JsonlEvidence,
    suggested_action: String,
    suppression: Option<JsonlSuppression>,
    truncated_fields: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonlEvidence {
    matcher_similarity: Option<f64>,
    handler_similarity: Option<f64>,
    matcher_difference: String,
    handler_evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    comparison: Option<JsonlComparison>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonlComparison {
    left_fingerprint: String,
    right_fingerprint: String,
    left_matcher: String,
    right_matcher: String,
    left_handler: String,
    right_handler: String,
    matcher_diff: JsonlMatcherDiff,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonlMatcherDiff {
    prefix: String,
    left_change: String,
    right_change: String,
    suffix: String,
}

#[derive(Serialize)]
struct JsonlSuppression {
    reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonlSummary<'a> {
    schema_version: &'static str,
    #[serde(rename = "type")]
    record_type: &'static str,
    tool_version: &'static str,
    summary: &'a Summary,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<&'a ExecutionMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    corpus: Option<CorpusCensus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    analysis: Option<AnalysisCensus>,
    record_count: usize,
    truncated: bool,
}

/// Renders one compact JSON object per finding followed by one summary object.
///
/// Records are newline-delimited and the returned string ends with a newline, making it
/// suitable for streaming consumers. Long free-text evidence is capped and named in each
/// finding's `truncatedFields` array. Unlike buffered reporters, JSONL retains every finding.
pub fn render_jsonl(context: &ReportContext<'_>) -> Result<String> {
    render_jsonl_context(context)
}

pub(super) fn write_jsonl_context(
    context: &ReportContext<'_>,
    metadata: Option<&CliReportMetadata<'_>>,
    writer: &mut dyn Write,
) -> Result<()> {
    let mut writer = BufWriter::new(writer);
    for finding in &context.result.findings {
        let bytes = serde_json::to_vec(&jsonl_finding(finding, context))
            .context("failed to serialize JSONL finding")?;
        writer.write_all(&bytes)?;
        writer.write_all(b"\n")?;
    }
    let summary = JsonlSummary {
        schema_version: JSONL_SCHEMA_VERSION,
        record_type: "summary",
        tool_version: context.tool_version,
        summary: &context.summary,
        metrics: context.metrics,
        corpus: metadata.map(|metadata| metadata.corpus.clone()),
        analysis: metadata.map(|metadata| metadata.analysis.clone()),
        record_count: context.result.findings.len() + 1,
        truncated: false,
    };
    let bytes = serde_json::to_vec(&summary).context("failed to serialize JSONL summary")?;
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn render_jsonl_context(context: &ReportContext<'_>) -> Result<String> {
    let mut output = String::new();
    for finding in &context.result.findings {
        let record = jsonl_finding(finding, context);
        output.push_str(
            &serde_json::to_string(&record).context("failed to serialize JSONL finding")?,
        );
        output.push('\n');
    }
    let summary = JsonlSummary {
        schema_version: JSONL_SCHEMA_VERSION,
        record_type: "summary",
        tool_version: context.tool_version,
        summary: &context.summary,
        metrics: context.metrics,
        corpus: None,
        analysis: None,
        record_count: context.result.findings.len() + 1,
        truncated: false,
    };
    output.push_str(&serde_json::to_string(&summary).context("failed to serialize JSONL summary")?);
    output.push('\n');
    Ok(output)
}

fn jsonl_finding(finding: &Finding, context: &ReportContext<'_>) -> JsonlFinding {
    let mut truncated_fields = Vec::new();
    let message = bounded_jsonl_text(&finding.message, "message", &mut truncated_fields);
    let matcher_difference = bounded_jsonl_text(
        &finding.evidence.matcher_difference,
        "evidence.matcherDifference",
        &mut truncated_fields,
    );
    let handler_evidence = bounded_jsonl_text(
        &finding.evidence.handler_evidence,
        "evidence.handlerEvidence",
        &mut truncated_fields,
    );
    let comparison = finding
        .evidence
        .comparison
        .as_ref()
        .map(|comparison| JsonlComparison {
            left_fingerprint: comparison.left_fingerprint.clone(),
            right_fingerprint: comparison.right_fingerprint.clone(),
            left_matcher: bounded_jsonl_text(
                &comparison.left_matcher,
                "evidence.comparison.leftMatcher",
                &mut truncated_fields,
            ),
            right_matcher: bounded_jsonl_text(
                &comparison.right_matcher,
                "evidence.comparison.rightMatcher",
                &mut truncated_fields,
            ),
            left_handler: bounded_jsonl_text(
                &comparison.left_handler,
                "evidence.comparison.leftHandler",
                &mut truncated_fields,
            ),
            right_handler: bounded_jsonl_text(
                &comparison.right_handler,
                "evidence.comparison.rightHandler",
                &mut truncated_fields,
            ),
            matcher_diff: JsonlMatcherDiff {
                prefix: bounded_jsonl_text(
                    &comparison.matcher_diff.prefix,
                    "evidence.comparison.matcherDiff.prefix",
                    &mut truncated_fields,
                ),
                left_change: bounded_jsonl_text(
                    &comparison.matcher_diff.left_change,
                    "evidence.comparison.matcherDiff.leftChange",
                    &mut truncated_fields,
                ),
                right_change: bounded_jsonl_text(
                    &comparison.matcher_diff.right_change,
                    "evidence.comparison.matcherDiff.rightChange",
                    &mut truncated_fields,
                ),
                suffix: bounded_jsonl_text(
                    &comparison.matcher_diff.suffix,
                    "evidence.comparison.matcherDiff.suffix",
                    &mut truncated_fields,
                ),
            },
        });
    let suggested_action = bounded_jsonl_text(
        &finding.suggested_action,
        "suggestedAction",
        &mut truncated_fields,
    );
    let suppression = finding
        .suppression
        .as_ref()
        .map(|suppression| JsonlSuppression {
            reason: bounded_jsonl_text(
                &suppression.reason,
                "suppression.reason",
                &mut truncated_fields,
            ),
        });
    JsonlFinding {
        schema_version: JSONL_SCHEMA_VERSION,
        record_type: "finding",
        tool_version: context.tool_version,
        fingerprint: crate::modes::finding_fingerprint(finding),
        active: finding.is_active(),
        contributes_to_threshold: finding.is_active()
            && finding.severity == Severity::Error
            && finding.rule.contributes_to_duplication_threshold(),
        rule: finding.rule,
        severity: finding.severity,
        message,
        primary: convert_location(&finding.primary, context.root),
        related: finding
            .related
            .iter()
            .map(|location| convert_location(location, context.root))
            .collect(),
        evidence: JsonlEvidence {
            matcher_similarity: finding.evidence.matcher_similarity,
            handler_similarity: finding.evidence.handler_similarity,
            matcher_difference,
            handler_evidence,
            comparison,
        },
        suggested_action,
        suppression,
        truncated_fields,
    }
}

fn bounded_jsonl_text(
    value: &str,
    field: &'static str,
    truncated_fields: &mut Vec<&'static str>,
) -> String {
    let value = report_safe(value);
    if value.chars().count() <= JSONL_TEXT_LIMIT_CHARS {
        return value;
    }
    truncated_fields.push(field);
    value
        .chars()
        .take(JSONL_TEXT_LIMIT_CHARS.saturating_sub(1))
        .chain(std::iter::once('\u{2026}'))
        .collect()
}
