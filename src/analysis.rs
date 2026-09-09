//! Rule evaluation over the shared step-definition representation.

mod evidence;
mod pairs;
mod similarity;
mod suppression;
mod usage;

use crate::config::Config;
use crate::model::{AnalysisResult, FeatureStep, StepDefinition};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CandidateSourceCensus {
    pub(crate) evaluated: usize,
    pub(crate) skipped: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalysisCensus {
    pub(crate) truncated: bool,
    pub(crate) candidate_comparisons_evaluated: usize,
    pub(crate) skipped_candidate_comparisons: u64,
    pub(crate) truncated_structural_classes: usize,
    pub(crate) candidate_sources: BTreeMap<String, CandidateSourceCensus>,
}

#[derive(Debug)]
/// Analysis data plus operational diagnostics that make the result incomplete.
///
/// Callers that need to render partial results before failing should use
/// [`analyze_with_diagnostics`]. Callers that only accept complete analysis can use [`analyze`].
pub struct AnalysisOutcome {
    /// Findings and analyzed inputs available before operational failure handling.
    pub result: AnalysisResult,
    /// Diagnostics that require the run to be treated as an operational failure.
    pub operational_errors: Vec<String>,
}

pub(crate) fn unmatched_suppressions(
    config: &Config,
    definitions: &[StepDefinition],
) -> Vec<usize> {
    suppression::unmatched_suppressions(config, definitions)
}

/// Evaluates every configured rule against extracted definitions and feature steps.
pub fn analyze(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
) -> Result<AnalysisResult> {
    let outcome = analyze_with_diagnostics(definitions, feature_steps, config)?;
    if !outcome.operational_errors.is_empty() {
        bail!(outcome.operational_errors.join("; "));
    }
    Ok(outcome.result)
}

/// Runs analysis while preserving partial results when bounded work cannot complete.
///
/// A non-empty [`AnalysisOutcome::operational_errors`] means the result is incomplete and must
/// not be treated as a passing gate. This form lets CLI and library consumers write diagnostic
/// reports before returning their operational-failure status. Candidate-generation truncation and
/// matcher compilation limits are both surfaced through `operational_errors`.
pub fn analyze_with_diagnostics(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
) -> Result<AnalysisOutcome> {
    Ok(analyze_internal(definitions, feature_steps, config)?.0)
}

pub(crate) fn analyze_for_cli(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
) -> Result<(AnalysisOutcome, AnalysisCensus)> {
    analyze_internal(definitions, feature_steps, config)
}

fn analyze_internal(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
) -> Result<(AnalysisOutcome, AnalysisCensus)> {
    let mut findings = Vec::new();
    let pair_analysis = pairs::analyze_definition_pairs(&definitions, config, &mut findings);
    let usage = usage::analyze_feature_usage(&definitions, &feature_steps, config, &mut findings);
    usage::analyze_unused(&definitions, &usage.used, config, &mut findings);
    findings.sort_by(|left, right| {
        left.primary
            .path
            .cmp(&right.primary.path)
            .then(left.primary.line.cmp(&right.primary.line))
            .then(left.primary.column.cmp(&right.primary.column))
            .then(left.rule.cmp(&right.rule))
    });
    let mut operational_errors = usage.operational_errors;
    operational_errors.extend(pair_analysis.operational_error);
    Ok((
        AnalysisOutcome {
            result: AnalysisResult {
                definitions,
                feature_steps,
                findings,
            },
            operational_errors,
        },
        pair_analysis.census,
    ))
}

#[cfg(test)]
mod tests;
