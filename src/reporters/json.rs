use super::shared::{report_value, report_value_with_census};
use super::{CorpusCensus, ReportContext};
use crate::model::AnalysisResult;
use anyhow::{Context, Result};
use std::path::Path;

/// Renders the versioned JSON report without writing it to disk.
pub fn render_json(result: &AnalysisResult, root: &Path) -> Result<String> {
    render_json_with_threshold(result, root, 0.0)
}

/// Renders JSON using the configured duplication threshold.
pub fn render_json_with_threshold(
    result: &AnalysisResult,
    root: &Path,
    threshold: f64,
) -> Result<String> {
    render_json_context(&ReportContext::new(result, root, threshold))
}

pub(super) fn render_json_context(context: &ReportContext<'_>) -> Result<String> {
    let report = report_value(context);
    serde_json::to_string_pretty(&report).context("failed to serialize JSON report")
}

pub(super) fn render_json_context_with_census(
    context: &ReportContext<'_>,
    corpus: Option<&CorpusCensus>,
) -> Result<String> {
    let report = report_value_with_census(context, corpus);
    serde_json::to_string_pretty(&report).context("failed to serialize JSON report")
}
