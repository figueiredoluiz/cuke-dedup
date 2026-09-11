use crate::analysis::AnalysisCensus;
use crate::model::{AnalysisResult, DuplicationThreshold, Severity};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Measured execution phases and discovered input counts for one CLI run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionMetrics {
    /// Number of definition source files selected by discovery.
    pub definition_files: usize,
    /// Number of feature files selected by discovery.
    pub feature_files: usize,
    /// Total number of definition and feature files selected by discovery.
    pub files_discovered: usize,
    /// Time spent walking and classifying input files.
    pub discovery_ms: f64,
    /// Time spent parsing definition and feature sources.
    pub parsing_ms: f64,
    /// Time spent evaluating analysis rules.
    pub analysis_ms: f64,
}

/// Deterministic input and extraction counts for one CLI run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CorpusCensus {
    /// Number of definition source files selected by discovery.
    pub(crate) definition_files: usize,
    /// Number of selected definition files that yielded at least one definition.
    pub(crate) definition_files_with_definitions: usize,
    /// Number of step definitions extracted before analysis and mode filtering.
    pub(crate) definitions_extracted: usize,
    /// Number of feature files selected by discovery.
    pub(crate) feature_files: usize,
    /// Number of selected feature files parsed successfully.
    pub(crate) feature_files_parsed: usize,
    /// Number of converted Gherkin Markdown files that parsed but yielded no concrete steps.
    pub(crate) feature_files_without_steps: usize,
    /// Whether definitions or feature steps are known to be missing from the extracted corpus.
    ///
    /// Set when a source imported a known registration through a module that could not be
    /// resolved statically, or when Gherkin Markdown conversion yielded no steps. Those inputs
    /// are absent from one or more rules, so the corpus — not just the comparison set — is a
    /// subset of what a complete run would analyze.
    pub(crate) incomplete: bool,
}

pub(crate) struct CliReportMetadata<'a> {
    pub(crate) corpus: &'a CorpusCensus,
    pub(crate) analysis: &'a AnalysisCensus,
    pub(crate) execution_successful: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Aggregate counts shared by every report format.
pub struct Summary {
    /// Number of extracted step definitions.
    pub definitions_analyzed: usize,
    /// Number of concrete feature steps.
    pub feature_steps_analyzed: usize,
    /// Number of active findings.
    pub findings: usize,
    /// Number of active error findings.
    pub errors: usize,
    /// Number of active warning findings.
    pub warnings: usize,
    /// Number of suppressed findings.
    pub suppressed: usize,
    /// Active findings grouped by stable rule name.
    pub by_rule: BTreeMap<String, usize>,
    /// Percentage-based duplication quality-gate result.
    pub duplication: DuplicationThreshold,
}

impl Summary {
    /// Computes report totals using the strict default duplication threshold.
    pub fn from_result(result: &AnalysisResult) -> Self {
        Self::from_result_with_threshold(result, 0.0)
    }

    /// Computes report totals using `threshold` as the allowed duplication percentage.
    pub fn from_result_with_threshold(result: &AnalysisResult, threshold: f64) -> Self {
        let active: Vec<_> = result
            .findings
            .iter()
            .filter(|finding| finding.is_active())
            .collect();
        let mut by_rule = BTreeMap::new();
        for finding in &active {
            *by_rule.entry(finding.rule.to_string()).or_insert(0) += 1;
        }
        Self {
            definitions_analyzed: result.definitions.len(),
            feature_steps_analyzed: result.feature_steps.len(),
            findings: active.len(),
            errors: active
                .iter()
                .filter(|finding| finding.severity == Severity::Error)
                .count(),
            warnings: active
                .iter()
                .filter(|finding| finding.severity == Severity::Warning)
                .count(),
            suppressed: result
                .findings
                .iter()
                .filter(|finding| finding.suppression.is_some())
                .count(),
            by_rule,
            duplication: DuplicationThreshold::from_result(result, threshold),
        }
    }
}

/// Shared, precomputed input passed to every report renderer.
pub struct ReportContext<'a> {
    /// Complete analysis result, including suppressed findings.
    pub result: &'a AnalysisResult,
    /// Repository root used to relativize source locations.
    pub root: &'a Path,
    /// Configured duplication percentage threshold.
    pub threshold: f64,
    /// Aggregate counts shared across report formats.
    pub summary: Summary,
    /// Version of CukeDedup that produced the report.
    pub tool_version: &'static str,
    /// Optional CLI execution measurements.
    pub metrics: Option<&'a ExecutionMetrics>,
}

impl<'a> ReportContext<'a> {
    /// Creates a context and computes its shared summary once.
    pub fn new(result: &'a AnalysisResult, root: &'a Path, threshold: f64) -> Self {
        Self {
            result,
            root,
            threshold,
            summary: Summary::from_result_with_threshold(result, threshold),
            tool_version: env!("CARGO_PKG_VERSION"),
            metrics: None,
        }
    }

    /// Creates a report context containing measured CLI execution phases.
    pub fn with_metrics(
        result: &'a AnalysisResult,
        root: &'a Path,
        threshold: f64,
        metrics: &'a ExecutionMetrics,
    ) -> Self {
        Self {
            metrics: Some(metrics),
            ..Self::new(result, root, threshold)
        }
    }
}
