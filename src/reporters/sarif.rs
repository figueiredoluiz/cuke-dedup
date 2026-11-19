use super::shared::{
    bounded_findings, bounded_json, pretty_json_cost, report_safe, MAX_REPORTED_CLUSTER_MEMBERS,
};
use super::{CliReportMetadata, ReportContext};
use crate::model::{Finding, Severity, SourceLocation};
use crate::resource_limits::{omission_cut, MAX_REPORT_FIELD_BYTES};
use anyhow::{Context, Result};
use std::ffi::OsStr;
#[cfg(windows)]
use std::path::Prefix;
use std::path::{Component, Path};

/// Renders active findings as a SARIF 2.1.0 log without writing it to disk.
pub fn render_sarif(context: &ReportContext<'_>) -> Result<String> {
    render_sarif_context_with_metadata(context, None)
}

/// Renders a SARIF 2.1.0 log of up to `MAX_BUFFERED_REPORT_FINDINGS` active findings.
///
/// Rule descriptors cover the rules of all active findings, including truncated ones. With
/// `metadata`, `executionSuccessful` comes from it and the corpus and analysis censuses are added;
/// without it, `executionSuccessful` is true.
pub(super) fn render_sarif_context_with_metadata(
    context: &ReportContext<'_>,
    metadata: Option<&CliReportMetadata<'_>>,
) -> Result<String> {
    let result = context.result;
    let root = context.root;
    let selected = bounded_findings(result, true, |finding| {
        pretty_json_cost(&sarif_finding(finding, root), 4)
    });
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
        .map(|finding| sarif_finding(finding, root))
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
    bounded_json(&sarif, true).context("failed to serialize SARIF report")
}

/// Builds the exact result object used by both byte accounting and final SARIF output.
fn sarif_finding(finding: &Finding, root: &Path) -> serde_json::Value {
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
            "cukeDedupFingerprint/v3": crate::modes::finding_fingerprint(finding)
        },
        "properties": {
            "suggestedAction": report_safe(&finding.suggested_action),
            "matcherSimilarity": finding.evidence.matcher_similarity,
            "handlerSimilarity": finding.evidence.handler_similarity,
            "clusterSize": finding.evidence.cluster.as_ref().map(|cluster| cluster.member_count),
            "pairFindingsCollapsed": finding.evidence.cluster.as_ref().map(|cluster| cluster.pair_findings_collapsed),
            "clusterMembersTruncated": finding.evidence.cluster.as_ref().is_some_and(|cluster| {
                cluster.members_truncated
                    || cluster.member_count > MAX_REPORTED_CLUSTER_MEMBERS
            }),
            "relatedLocationsTruncated": finding.related.len() > MAX_REPORTED_CLUSTER_MEMBERS.saturating_sub(1),
        }
    })
}

/// Builds a SARIF location with a root-relative URI and region; sets `id` only when one is given.
fn sarif_location(location: &SourceLocation, root: &Path, id: Option<usize>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "physicalLocation": {
            "artifactLocation": { "uri": artifact_uri(&location.path, root) },
            "region": {
                "startLine": location.line,
                "startColumn": location.column,
                "endLine": location.end_line,
                "endColumn": location.end_column,
            }
        }
    });
    if let Some(id) = id {
        value["id"] = serde_json::json!(id);
    }
    value
}

/// Encodes `path` as an RFC 3986 reference: root-relative when under `root`, else an RFC 8089
/// `file:` URI. Segments keep exact bytes so consumers resolve the same file; separators come
/// from path components, so a Unix file name containing `\` stays one segment.
fn artifact_uri(path: &Path, root: &Path) -> String {
    let (relative, absolute) = match path.strip_prefix(root) {
        Ok(relative) => (relative, false),
        Err(_) => (path, path.is_absolute()),
    };
    let (authority, lead) = windows_prefix(relative);
    // Copy at most one byte past the field limit; `total` keeps the native length for the marker.
    let mut segments = Vec::new();
    let (mut kept, mut total) = (0usize, 0usize);
    for component in relative.components() {
        let name = match component {
            // `windows_prefix` already consumed a prefix; root and `.` add no segment.
            Component::Prefix(_) | Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => OsStr::new(".."),
            Component::Normal(name) => name,
        };
        let separator = usize::from(total > 0);
        total += separator + name.len();
        let budget = (MAX_REPORT_FIELD_BYTES + 1).saturating_sub(kept + separator);
        if budget > 0 {
            let bytes = component_bytes(name, budget);
            kept += separator + bytes.len();
            segments.push(bytes);
        }
    }
    // Oversized paths are cut on native bytes so the marker counts source bytes and raw bytes
    // survive; segments never contain `/`.
    let mut joined = segments.join(&b'/');
    let marker = omission_cut(&joined, total, MAX_REPORT_FIELD_BYTES).map(|(end, marker)| {
        joined.truncate(end);
        marker
    });
    let mut encoded = joined
        .split(|&byte| byte == b'/')
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/");
    if let Some(marker) = marker {
        encoded.push_str(&percent_encode(marker.as_bytes()));
    }
    // A prefix makes the path non-relative even when `is_absolute` is false (`C:foo`).
    if absolute || !lead.is_empty() {
        format!("file://{}/{lead}{encoded}", percent_encode(&authority))
    } else {
        encoded
    }
}

/// RFC 8089 authority and leading path text for a Windows prefix: `C:/` for a drive (the colon
/// stays raw), the server and `share/` for UNC, an encoded `name/` for other verbatim forms.
#[cfg(windows)]
fn windows_prefix(path: &Path) -> (Vec<u8>, String) {
    // fail-closed: a path without a prefix has no authority and no drive text.
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return (Vec::new(), String::new());
    };
    match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
            (Vec::new(), format!("{}:/", char::from(drive)))
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => (
            component_bytes(server, MAX_REPORT_FIELD_BYTES),
            format!(
                "{}/",
                percent_encode(&component_bytes(share, MAX_REPORT_FIELD_BYTES))
            ),
        ),
        Prefix::Verbatim(name) | Prefix::DeviceNS(name) => (
            Vec::new(),
            format!(
                "{}/",
                percent_encode(&component_bytes(name, MAX_REPORT_FIELD_BYTES))
            ),
        ),
    }
}

/// Paths outside Windows have no prefix component.
#[cfg(not(windows))]
fn windows_prefix(_path: &Path) -> (Vec<u8>, String) {
    (Vec::new(), String::new())
}

/// Up to `limit` native bytes of a path component, copied without converting the whole name:
/// raw bytes on Unix, UTF-8 elsewhere (lone surrogates and a split final character are lossy).
fn component_bytes(component: &OsStr, limit: usize) -> Vec<u8> {
    let bytes = component.as_encoded_bytes();
    let bytes = bytes.get(..limit).unwrap_or(bytes);
    #[cfg(unix)]
    {
        bytes.to_vec()
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(bytes).into_owned().into_bytes()
    }
}

/// Percent-encodes every byte outside RFC 3986 `unreserved`, so no consumer reads a byte as a
/// delimiter, scheme separator, escape or form-encoded space.
fn percent_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests;
