use super::shared::{report_value, report_value_with_metadata};
use super::{CliReportMetadata, ReportContext};
use anyhow::{Context, Result};

/// Renders the versioned JSON report without writing it to disk.
pub fn render_json(context: &ReportContext<'_>) -> Result<String> {
    let report = report_value(context);
    serde_json::to_string_pretty(&report).context("failed to serialize JSON report")
}

pub(super) fn render_json_context_with_metadata(
    context: &ReportContext<'_>,
    metadata: Option<&CliReportMetadata<'_>>,
) -> Result<String> {
    let report = report_value_with_metadata(context, metadata);
    serde_json::to_string_pretty(&report).context("failed to serialize JSON report")
}
