//! Rule evaluation over the shared step-definition representation.

mod evidence;
mod pairs;
mod similarity;
mod suppression;
mod usage;

use crate::config::Config;
use crate::model::{AnalysisResult, FeatureStep, StepDefinition};
use anyhow::{bail, Result};

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

/// Runs analysis while preserving partial results when bounded matcher compilation fails.
///
/// A non-empty [`AnalysisOutcome::operational_errors`] means the result is incomplete and must
/// not be treated as a passing gate. This form lets CLI and library consumers write diagnostic
/// reports before returning their operational-failure status.
pub fn analyze_with_diagnostics(
    definitions: Vec<StepDefinition>,
    feature_steps: Vec<FeatureStep>,
    config: &Config,
) -> Result<AnalysisOutcome> {
    let mut findings = Vec::new();
    pairs::analyze_definition_pairs(&definitions, config, &mut findings)?;
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
    Ok(AnalysisOutcome {
        result: AnalysisResult {
            definitions,
            feature_steps,
            findings,
        },
        operational_errors: usage.operational_errors,
    })
}

#[cfg(test)]
mod tests;
