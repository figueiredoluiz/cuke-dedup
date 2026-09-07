//! Rule evaluation over the shared step-definition representation.

mod evidence;
mod pairs;
mod similarity;
mod suppression;
mod usage;

use crate::config::Config;
use crate::model::{AnalysisResult, FeatureStep, StepDefinition};
use anyhow::Result;

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
    let mut findings = Vec::new();
    pairs::analyze_definition_pairs(&definitions, config, &mut findings)?;
    let used = usage::analyze_feature_usage(&definitions, &feature_steps, config, &mut findings);
    usage::analyze_unused(&definitions, &used, config, &mut findings);
    findings.sort_by(|left, right| {
        left.primary
            .path
            .cmp(&right.primary.path)
            .then(left.primary.line.cmp(&right.primary.line))
            .then(left.primary.column.cmp(&right.primary.column))
            .then(left.rule.cmp(&right.rule))
    });
    Ok(AnalysisResult {
        definitions,
        feature_steps,
        findings,
    })
}

#[cfg(test)]
mod tests;
