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
/// Analysis data plus diagnostics describing how complete the result is.
///
/// Callers that need to render partial results before failing should use
/// [`analyze_with_diagnostics`]. Callers that only accept complete analysis can use [`analyze`].
#[non_exhaustive]
pub struct AnalysisOutcome {
    /// Findings and analyzed inputs available before completeness handling.
    pub result: AnalysisResult,
    /// Diagnostics describing analysis that was bounded before it could finish.
    ///
    /// A non-empty list means findings are a subset of what a complete run would report, so the
    /// absence of a finding proves nothing. Findings that are present remain valid.
    pub incomplete: Vec<String>,
}

/// Evaluates every configured rule against extracted definitions and feature steps.
pub fn analyze(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
) -> Result<AnalysisResult> {
    let outcome = analyze_with_diagnostics(definitions, feature_steps, config)?;
    if !outcome.incomplete.is_empty() {
        bail!(outcome.incomplete.join("; "));
    }
    Ok(outcome.result)
}

/// Runs analysis while preserving partial results when bounded work cannot complete.
///
/// A non-empty [`AnalysisOutcome::incomplete`] means findings are a subset of a complete run, so
/// the absence of a finding proves nothing. This form lets CLI and library consumers write
/// diagnostic reports and decide their own severity. Candidate-generation truncation and matcher
/// compilation limits are both surfaced through `incomplete`.
pub fn analyze_with_diagnostics(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
) -> Result<AnalysisOutcome> {
    Ok(analyze_internal(definitions, feature_steps, config, false)?.0)
}

pub(crate) fn analyze_for_cli(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
) -> Result<(
    AnalysisOutcome,
    AnalysisCensus,
    suppression::UnmatchedSuppressionOutcome,
)> {
    analyze_internal(definitions, feature_steps, config, true)
}

fn analyze_internal(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
    include_unmatched_suppressions: bool,
) -> Result<(
    AnalysisOutcome,
    AnalysisCensus,
    suppression::UnmatchedSuppressionOutcome,
)> {
    config.validate_analysis_limits()?;
    let mut findings = Vec::new();
    let suppressions = suppression::SuppressionIndex::new(config, &definitions);
    let pair_analysis =
        pairs::analyze_definition_pairs(&definitions, config, &suppressions, &mut findings);
    let overlap_budget = config
        .max_candidate_comparisons
        .saturating_sub(pair_analysis.census.candidate_comparisons_evaluated);
    let usage = usage::analyze_feature_usage(
        &definitions,
        &feature_steps,
        config,
        &suppressions,
        &mut findings,
        overlap_budget,
    );
    usage::analyze_unused(
        &definitions,
        &usage.used,
        config,
        &suppressions,
        &mut findings,
    );
    let unmatched_suppressions = if include_unmatched_suppressions {
        suppressions.unmatched()
    } else {
        suppression::UnmatchedSuppressionOutcome::default()
    };
    findings.sort_by(|left, right| {
        left.primary
            .path
            .cmp(&right.primary.path)
            .then(left.primary.line.cmp(&right.primary.line))
            .then(left.primary.column.cmp(&right.primary.column))
            .then(left.rule.cmp(&right.rule))
    });
    let mut census = pair_analysis.census;
    census.candidate_comparisons_evaluated = census
        .candidate_comparisons_evaluated
        .saturating_add(usage.overlap_census.evaluated);
    census.skipped_candidate_comparisons = census
        .skipped_candidate_comparisons
        .saturating_add(usage.overlap_census.skipped);
    census.truncated |= usage.overlap_census.skipped > 0;
    census
        .candidate_sources
        .insert("matcherOverlap".to_owned(), usage.overlap_census);
    let mut incomplete = usage.incomplete;
    incomplete.extend(pair_analysis.incomplete);
    Ok((
        AnalysisOutcome {
            result: AnalysisResult {
                definitions,
                feature_steps,
                findings,
            },
            incomplete,
        },
        census,
        unmatched_suppressions,
    ))
}

#[cfg(test)]
mod tests;
