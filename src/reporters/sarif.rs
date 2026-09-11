use super::shared::{
    bounded_findings, convert_location, report_safe, MAX_REPORTED_CLUSTER_MEMBERS,
};
use super::{CliReportMetadata, ReportContext};
use crate::model::{Severity, SourceLocation};
use anyhow::{Context, Result};
use std::path::Path;

/// Renders active findings as a SARIF 2.1.0 log without writing it to disk.
pub fn render_sarif(context: &ReportContext<'_>) -> Result<String> {
    render_sarif_context_with_metadata(context, None)
}

pub(super) fn render_sarif_context_with_metadata(
    context: &ReportContext<'_>,
    metadata: Option<&CliReportMetadata<'_>>,
) -> Result<String> {
    let result = context.result;
    let root = context.root;
    let selected = bounded_findings(result, true);
    let findings_truncated = selected.truncated;
    let rules = result
        .findings
        .iter()
        .filter(|finding| finding.is_active())
        .map(|finding| finding.rule)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|rule| {
            serde_json::json!({
                "id": rule.to_string(),
                "name": rule.to_string(),
                "shortDescription": { "text": rule.to_string() },
                "helpUri": rule.documentation_url(),
            })
        })
        .collect::<Vec<_>>();
    let findings = selected
        .findings
        .into_iter()
        .map(|finding| {
            let primary = sarif_location(&finding.primary, root, None);
            let related = finding
                .related
                .iter()
                .take(MAX_REPORTED_CLUSTER_MEMBERS.saturating_sub(1))
                .enumerate()
                .map(|(index, location)| sarif_location(location, root, Some(index + 1)))
                .collect::<Vec<_>>();
            serde_json::json!({
                "ruleId": finding.rule.to_string(),
                "level": match finding.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Off => "note",
                },
                "message": { "text": report_safe(&finding.message) },
                "locations": [primary],
                "relatedLocations": related,
                "partialFingerprints": {
                    "cukeDedupFingerprint/v2": crate::modes::finding_fingerprint(finding)
                },
                "properties": {
                    "suggestedAction": report_safe(&finding.suggested_action),
                    "matcherSimilarity": finding.evidence.matcher_similarity,
                    "handlerSimilarity": finding.evidence.handler_similarity,
                    "clusterSize": finding.evidence.cluster.as_ref().map(|cluster| cluster.member_count),
                    "pairFindingsCollapsed": finding.evidence.cluster.as_ref().map(|cluster| cluster.pair_findings_collapsed),
                    "clusterMembersTruncated": finding.evidence.cluster.as_ref().is_some_and(|cluster| cluster.member_count > MAX_REPORTED_CLUSTER_MEMBERS),
                    "relatedLocationsTruncated": finding.related.len() > MAX_REPORTED_CLUSTER_MEMBERS.saturating_sub(1),
                }
            })
        })
        .collect::<Vec<_>>();
    let retained_findings = findings.len();
    let mut sarif = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "automationDetails": { "id": "cuke-dedup/" },
            "columnKind": "unicodeCodePoints",
            "tool": {
                "driver": {
                    "name": "CukeDedup",
                    "semanticVersion": context.tool_version,
                    "informationUri": "https://github.com/figueiredoluiz/cuke-dedup",
                    "rules": rules,
                }
            },
            "invocations": [{
                "executionSuccessful": metadata.is_none_or(|metadata| metadata.execution_successful),
                "properties": {
                    "threshold": context.threshold,
                    "duplicatedDefinitions": context.summary.duplication.duplicated_definitions,
                    "totalDefinitions": context.summary.duplication.total_definitions,
                    "duplicationPercentage": context.summary.duplication.percentage,
                    "thresholdPassed": context.summary.duplication.passed,
                    "contributingRules": context.summary.duplication.rules,
                    "activeFindings": context.summary.findings,
                    "suppressedFindings": context.summary.suppressed,
                    "retainedFindings": retained_findings,
                    "findingsTruncated": findings_truncated,
                }
            }],
            "results": findings,
        }]
    });
    if let Some(metadata) = metadata {
        sarif["runs"][0]["invocations"][0]["properties"]["corpus"] =
            serde_json::to_value(metadata.corpus)
                .context("failed to serialize SARIF corpus census")?;
        sarif["runs"][0]["invocations"][0]["properties"]["analysis"] =
            serde_json::to_value(metadata.analysis)
                .context("failed to serialize SARIF analysis census")?;
    }
    serde_json::to_string_pretty(&sarif).context("failed to serialize SARIF report")
}

fn sarif_location(location: &SourceLocation, root: &Path, id: Option<usize>) -> serde_json::Value {
    let converted = convert_location(location, root);
    let mut value = serde_json::json!({
        "physicalLocation": {
            "artifactLocation": { "uri": converted.path },
            "region": {
                "startLine": converted.line,
                "startColumn": converted.column,
                "endLine": converted.end_line,
                "endColumn": converted.end_column,
            }
        }
    });
    if let Some(id) = id {
        value["id"] = serde_json::json!(id);
    }
    value
}
