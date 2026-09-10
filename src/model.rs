//! Shared intermediate representation and diagnostics.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Source span with one-based starts and one-based, exclusive end coordinates.
pub struct SourceLocation {
    /// Absolute or caller-provided source path.
    pub path: PathBuf,
    /// One-based start line.
    pub line: usize,
    /// One-based start column.
    pub column: usize,
    /// One-based line containing the exclusive end position.
    pub end_line: usize,
    /// One-based, exclusive end column in the original source text.
    pub end_column: usize,
}

impl SourceLocation {
    /// Creates a source location from a path and one-based span coordinates.
    pub fn new(
        path: impl Into<PathBuf>,
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
    ) -> Self {
        Self {
            path: path.into(),
            line,
            column,
            end_line,
            end_column,
        }
    }

    /// Formats the location as `path:line:column`, relative to `root` when possible.
    pub fn display(&self, root: &Path) -> String {
        let path = self.path.strip_prefix(root).unwrap_or(&self.path);
        let path = path.to_string_lossy().replace('\\', "/");
        format!("{path}:{}:{}", self.line, self.column)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Syntax used by a step definition matcher.
pub enum MatcherKind {
    /// A Cucumber Expression such as `I have {int} items`.
    CucumberExpression,
    /// A JavaScript regular expression.
    RegularExpression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Step-definition framework inferred from a source file.
pub enum Framework {
    /// Cucumber.js registration APIs.
    CucumberJs,
    /// Playwright BDD registration APIs.
    PlaywrightBdd,
    /// Cypress step definitions registered through the Badeball Cucumber preprocessor.
    CypressCucumber,
    /// No supported framework import could be inferred.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Comparable representations of a definition handler.
pub struct HandlerFingerprint {
    /// Exact syntax-tree representation.
    pub exact: String,
    /// Representation normalized for superficial syntax differences.
    pub normalized: String,
    /// Representation with parameter and local identifiers normalized.
    pub alpha_normalized: String,
    /// Control-flow and call structure with literals normalized.
    pub structural: String,
    /// Ordered behavioral operations observed in the handler.
    pub behavior_signature: Vec<String>,
    /// Bounded original source displayed in reports.
    pub source_snippet: String,
    /// Whether a concrete handler body was available for comparison.
    #[serde(default)]
    pub comparable: bool,
    #[serde(default)]
    /// Whether the handler lacks enough behavior for duplication analysis.
    pub trivial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Rule-scoped, reason-bearing suppression declared beside a definition.
pub struct InlineSuppression {
    /// Rule disabled for findings involving this definition.
    pub rule: Rule,
    /// Human-readable justification retained in reports.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// A step definition extracted into the analyzer's shared representation.
pub struct StepDefinition {
    /// Original matcher text without JavaScript delimiters.
    pub matcher: String,
    /// Canonical matcher used for equivalence checks.
    pub normalized_matcher: String,
    /// Matcher syntax.
    pub matcher_kind: MatcherKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    /// JavaScript regular-expression flags, or an empty string otherwise.
    pub matcher_flags: String,
    /// Comparable handler representations.
    pub handler: HandlerFingerprint,
    /// Framework that registered the definition.
    pub framework: Framework,
    /// Registration name resolved from direct, aliased, or namespaced usage.
    pub registration: String,
    /// Full registration call location.
    pub location: SourceLocation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Source-local suppression directives attached to this definition.
    pub inline_suppressions: Vec<InlineSuppression>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// One concrete step from a parsed Gherkin scenario.
pub struct FeatureStep {
    /// Localized Gherkin keyword.
    pub keyword: String,
    /// Concrete step text after Scenario Outline expansion.
    pub text: String,
    /// Source location of the originating step.
    pub location: SourceLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Stable identifier for an analyzer rule.
pub enum Rule {
    /// Two definitions have the same effective matcher.
    DuplicateMatcher,
    /// Two matchers become equivalent after normalization.
    NormalizedMatcher,
    /// A concrete feature step matches multiple definitions.
    AmbiguousStep,
    /// Two matchers can accept the same step text, independent of the feature corpus.
    OverlappingMatcher,
    /// Distinct matchers share an equivalent implementation.
    DuplicateHandler,
    /// Similar wording and handler structure indicate likely duplication.
    NearDuplicateStep,
    /// Similar definitions may be replaceable by a parameterized step.
    ParameterizationCandidate,
    /// No discovered feature step uses a definition.
    UnusedDefinition,
}

impl Rule {
    /// Every rule in deterministic report order.
    pub const ALL: [Rule; 8] = [
        Rule::DuplicateMatcher,
        Rule::NormalizedMatcher,
        Rule::AmbiguousStep,
        Rule::OverlappingMatcher,
        Rule::DuplicateHandler,
        Rule::NearDuplicateStep,
        Rule::ParameterizationCandidate,
        Rule::UnusedDefinition,
    ];

    /// Rules whose active error findings contribute to the duplication threshold.
    pub const DUPLICATION_THRESHOLD: [Rule; 5] = [
        Rule::DuplicateMatcher,
        Rule::NormalizedMatcher,
        Rule::DuplicateHandler,
        Rule::NearDuplicateStep,
        Rule::ParameterizationCandidate,
    ];

    /// Returns the stable kebab-case configuration and report name.
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::DuplicateMatcher => "duplicate-matcher",
            Rule::NormalizedMatcher => "normalized-matcher",
            Rule::AmbiguousStep => "ambiguous-step",
            Rule::OverlappingMatcher => "overlapping-matcher",
            Rule::DuplicateHandler => "duplicate-handler",
            Rule::NearDuplicateStep => "near-duplicate-step",
            Rule::ParameterizationCandidate => "parameterization-candidate",
            Rule::UnusedDefinition => "unused-definition",
        }
    }

    /// Returns whether error findings from this rule contribute to the duplication threshold.
    pub fn contributes_to_duplication_threshold(self) -> bool {
        Self::DUPLICATION_THRESHOLD.contains(&self)
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Rule {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "duplicate-matcher" => Ok(Self::DuplicateMatcher),
            "normalized-matcher" => Ok(Self::NormalizedMatcher),
            "ambiguous-step" => Ok(Self::AmbiguousStep),
            "overlapping-matcher" => Ok(Self::OverlappingMatcher),
            "duplicate-handler" => Ok(Self::DuplicateHandler),
            "near-duplicate-step" => Ok(Self::NearDuplicateStep),
            "parameterization-candidate" => Ok(Self::ParameterizationCandidate),
            "unused-definition" => Ok(Self::UnusedDefinition),
            _ => Err(format!("unknown rule `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Configured impact of a finding.
pub enum Severity {
    /// Rule evaluation is disabled.
    Off,
    /// Finding is reported without causing exit code 1.
    Warning,
    /// Active finding causes exit code 1.
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Off => f.write_str("off"),
            Severity::Warning => f.write_str("warning"),
            Severity::Error => f.write_str("error"),
        }
    }
}

impl FromStr for Severity {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Severity::Off),
            "warn" | "warning" => Ok(Severity::Warning),
            "error" => Ok(Severity::Error),
            _ => Err(format!(
                "invalid severity `{value}` (expected off, warning, or error)"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Human- and machine-readable evidence supporting a finding.
pub struct FindingEvidence {
    /// Matcher similarity score from zero to one, when applicable.
    pub matcher_similarity: Option<f64>,
    /// Handler similarity score from zero to one, when applicable.
    pub handler_similarity: Option<f64>,
    /// Concise matcher comparison for terminal output.
    pub matcher_difference: String,
    /// Concise handler comparison for terminal output.
    pub handler_evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Structured definition comparison for rich reports.
    pub comparison: Option<DefinitionComparison>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Side-by-side matcher and handler content for a pair finding.
pub struct DefinitionComparison {
    /// Location-independent semantic fingerprint of the left definition.
    pub left_fingerprint: String,
    /// Location-independent semantic fingerprint of the right definition.
    pub right_fingerprint: String,
    /// Left definition's original matcher.
    pub left_matcher: String,
    /// Right definition's original matcher.
    pub right_matcher: String,
    /// Left definition's bounded handler source.
    pub left_handler: String,
    /// Right definition's bounded handler source.
    pub right_handler: String,
    /// Structured common and changed matcher segments.
    pub matcher_diff: MatcherDiff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Common and differing segments of two matcher strings.
pub struct MatcherDiff {
    /// Shared matcher prefix.
    pub prefix: String,
    /// Segment unique to the left matcher.
    pub left_change: String,
    /// Segment unique to the right matcher.
    pub right_change: String,
    /// Shared matcher suffix.
    pub suffix: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Reason that a finding is excluded from active totals.
pub struct Suppression {
    /// Human-readable suppression justification.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// One rule diagnostic produced by analysis.
pub struct Finding {
    /// Rule that produced the finding.
    pub rule: Rule,
    /// Configured impact.
    pub severity: Severity,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Main source span.
    pub primary: SourceLocation,
    /// Additional spans involved in the finding.
    pub related: Vec<SourceLocation>,
    /// Comparison data supporting the diagnostic.
    pub evidence: FindingEvidence,
    /// Recommended remediation.
    pub suggested_action: String,
    /// Suppression metadata, when excluded from active totals.
    pub suppression: Option<Suppression>,
}

impl Finding {
    /// Returns whether the finding contributes to output totals and exit status.
    pub fn is_active(&self) -> bool {
        self.severity != Severity::Off && self.suppression.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// Extracted inputs and deterministically ordered analyzer findings.
pub struct AnalysisResult {
    /// Extracted definition corpus.
    pub definitions: Vec<StepDefinition>,
    /// Concrete feature-step corpus.
    pub feature_steps: Vec<FeatureStep>,
    /// Findings produced by configured rules.
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Result of applying a percentage-based tolerance to active duplication errors.
pub struct DuplicationThreshold {
    /// Maximum duplicated-definition percentage allowed by configuration.
    pub threshold: f64,
    /// Unique definitions involved in active error-level duplication findings.
    pub duplicated_definitions: usize,
    /// Total number of definitions discovered for the analysis.
    pub total_definitions: usize,
    /// Unrounded duplicated-definition percentage.
    pub percentage: f64,
    /// Whether the percentage is less than or equal to the configured threshold.
    pub passed: bool,
    /// Rules whose error-level findings contribute to the numerator.
    pub rules: Vec<Rule>,
}

impl DuplicationThreshold {
    /// Calculates the duplication rate after suppressions and analysis modes are applied.
    pub fn from_result(result: &AnalysisResult, threshold: f64) -> Self {
        let definition_locations: HashSet<_> = result
            .definitions
            .iter()
            .map(|definition| definition.location.clone())
            .collect();
        let mut duplicated_locations = HashSet::new();
        let mut contributing_rules = BTreeSet::new();
        for finding in result.findings.iter().filter(|finding| {
            finding.is_active()
                && finding.severity == Severity::Error
                && finding.rule.contributes_to_duplication_threshold()
        }) {
            contributing_rules.insert(finding.rule);
            for location in std::iter::once(&finding.primary).chain(&finding.related) {
                if definition_locations.contains(location) {
                    duplicated_locations.insert(location.clone());
                }
            }
        }

        let total_definitions = result.definitions.len();
        let duplicated_definitions = result
            .definitions
            .iter()
            .filter(|definition| duplicated_locations.contains(&definition.location))
            .count();
        let percentage = if total_definitions == 0 {
            0.0
        } else {
            duplicated_definitions as f64 / total_definitions as f64 * 100.0
        };
        Self {
            threshold,
            duplicated_definitions,
            total_definitions,
            percentage,
            passed: threshold.is_finite() && percentage <= threshold,
            rules: contributing_rules.into_iter().collect(),
        }
    }
}

/// Computes a deterministic FNV-1a fingerprint for report and comparison data.
pub fn stable_fingerprint(value: &str) -> String {
    // FNV-1a is deliberately simple and deterministic across platforms and Rust versions.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(line: usize) -> StepDefinition {
        StepDefinition {
            matcher: format!("step {line}"),
            normalized_matcher: format!("step {line}"),
            matcher_kind: MatcherKind::CucumberExpression,
            matcher_flags: String::new(),
            handler: HandlerFingerprint {
                exact: String::new(),
                normalized: String::new(),
                alpha_normalized: String::new(),
                structural: String::new(),
                behavior_signature: Vec::new(),
                source_snippet: String::new(),
                comparable: false,
                trivial: true,
            },
            framework: Framework::Unknown,
            registration: "Given".to_owned(),
            location: SourceLocation::new("steps.ts", line, 1, line, 10),
            inline_suppressions: Vec::new(),
        }
    }

    fn finding(
        definitions: &[StepDefinition],
        left: usize,
        right: usize,
        severity: Severity,
    ) -> Finding {
        Finding {
            rule: Rule::DuplicateMatcher,
            severity,
            message: "duplicate".to_owned(),
            primary: definitions[left].location.clone(),
            related: vec![definitions[right].location.clone()],
            evidence: FindingEvidence {
                matcher_similarity: Some(1.0),
                handler_similarity: None,
                matcher_difference: String::new(),
                handler_evidence: String::new(),
                comparison: None,
            },
            suggested_action: "consolidate".to_owned(),
            suppression: None,
        }
    }

    #[test]
    fn rule_names_are_stable_kebab_case() {
        for rule in Rule::ALL {
            assert_eq!(rule.to_string().parse::<Rule>(), Ok(rule));
        }
    }

    #[test]
    fn threshold_rules_exclude_correctness_and_usage_findings() {
        assert!(Rule::DuplicateMatcher.contributes_to_duplication_threshold());
        assert!(Rule::ParameterizationCandidate.contributes_to_duplication_threshold());
        assert!(!Rule::AmbiguousStep.contributes_to_duplication_threshold());
        assert!(!Rule::UnusedDefinition.contributes_to_duplication_threshold());
    }

    #[test]
    fn duplication_threshold_counts_unique_active_error_definitions() {
        let definitions: Vec<_> = (1..=4).map(definition).collect();
        let first = finding(&definitions, 0, 1, Severity::Error);
        let second = finding(&definitions, 0, 2, Severity::Error);
        let warning = finding(&definitions, 2, 3, Severity::Warning);
        let mut suppressed = finding(&definitions, 1, 3, Severity::Error);
        suppressed.suppression = Some(Suppression {
            reason: "accepted".to_owned(),
        });
        let result = AnalysisResult {
            definitions,
            feature_steps: Vec::new(),
            findings: vec![first, second, warning, suppressed],
        };

        let at_limit = DuplicationThreshold::from_result(&result, 75.0);
        assert_eq!(at_limit.duplicated_definitions, 3);
        assert_eq!(at_limit.total_definitions, 4);
        assert_eq!(at_limit.percentage, 75.0);
        assert_eq!(at_limit.rules, [Rule::DuplicateMatcher]);
        assert!(at_limit.passed);
        assert!(!DuplicationThreshold::from_result(&result, 74.99).passed);
    }

    #[test]
    fn empty_analysis_has_zero_percent_duplication() {
        let result = AnalysisResult::default();
        let threshold = DuplicationThreshold::from_result(&result, 0.0);
        assert_eq!(threshold.percentage, 0.0);
        assert!(threshold.passed);
    }

    #[test]
    fn duplication_threshold_counts_multiple_definitions_at_one_location() {
        let mut definitions = vec![definition(1), definition(2)];
        definitions[1].location = definitions[0].location.clone();
        let duplicate = finding(&definitions, 0, 1, Severity::Error);
        let result = AnalysisResult {
            definitions,
            feature_steps: Vec::new(),
            findings: vec![duplicate],
        };
        let threshold = DuplicationThreshold::from_result(&result, 100.0);
        assert_eq!(threshold.duplicated_definitions, 2);
        assert_eq!(threshold.percentage, 100.0);
    }

    #[test]
    fn fingerprint_is_stable_and_content_sensitive() {
        assert_eq!(stable_fingerprint("hello"), "a430d84680aabd0b");
        assert_ne!(stable_fingerprint("hello"), stable_fingerprint("Hello"));
    }

    #[test]
    fn displayed_locations_use_portable_path_separators() {
        let location = SourceLocation::new("steps\\checkout.ts", 3, 4, 3, 10);
        assert_eq!(location.display(Path::new(".")), "steps/checkout.ts:3:4");
    }
}
