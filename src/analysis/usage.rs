use super::suppression::SuppressionIndex;
use crate::config::Config;
use crate::model::{
    FeatureStep, Finding, FindingEvidence, MatcherKind, Rule, Severity, StepDefinition,
};
use crate::resource_limits::{compile_regex, MAX_REGEX_PATTERN_BYTES};
use cucumber_expressions::expand::IntoRegexCharIter;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

static FALLBACK_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^{}]+)\}").expect("static placeholder regex"));

struct CompiledMatcher {
    regex: Option<Regex>,
    authoritative: bool,
}

enum CucumberRegexExpression {
    Compiled(String),
    Unsupported,
    ResourceLimit,
}

pub(super) struct FeatureUsageOutcome {
    pub(super) used: BTreeSet<usize>,
    pub(super) operational_errors: Vec<String>,
}

pub(super) fn analyze_feature_usage(
    definitions: &[StepDefinition],
    steps: &[FeatureStep],
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
) -> FeatureUsageOutcome {
    let (compiled, operational_errors): (Vec<_>, Vec<_>) =
        definitions.iter().map(compile_matcher).unzip();
    let operational_errors = operational_errors.into_iter().flatten().collect();
    // Definitions with syntax unsupported by Rust's regex engine cannot be proven unused.
    // Treat them as indeterminate instead of emitting a guaranteed false positive.
    let mut used: BTreeSet<_> = compiled
        .iter()
        .enumerate()
        .filter_map(|(index, matcher)| matcher.regex.is_none().then_some(index))
        .collect();
    let mut ambiguities: BTreeMap<_, (&FeatureStep, BTreeSet<usize>, usize)> = BTreeMap::new();
    for step in steps {
        let matches: Vec<_> = compiled
            .iter()
            .enumerate()
            .filter_map(|(index, matcher)| {
                matcher
                    .regex
                    .as_ref()
                    .filter(|matcher| matcher.is_match(&step.text))
                    .map(|_| index)
            })
            .collect();
        used.extend(matches.iter().copied());
        let authoritative_matches: BTreeSet<_> = matches
            .iter()
            .copied()
            .filter(|index| compiled[*index].authoritative)
            .collect();
        if authoritative_matches.len() > 1 {
            let ambiguity_key = (
                step.location.path.clone(),
                step.location.line,
                step.location.column,
                authoritative_matches.clone(),
            );
            let entry = ambiguities
                .entry(ambiguity_key)
                .or_insert_with(|| (step, BTreeSet::new(), 0));
            entry.1.extend(authoritative_matches);
            entry.2 += 1;
        }
    }
    let severity = config.severity(Rule::AmbiguousStep);
    if severity != Severity::Off {
        for (_, (step, matches, expansion_count)) in ambiguities {
            let matched_indices = matches.iter().copied().collect::<Vec<_>>();
            let matched_definitions: Vec<_> = matched_indices
                .iter()
                .map(|index| &definitions[*index])
                .collect();
            let matcher_list = matched_definitions
                .iter()
                .map(|definition| format!("`{}`", definition.matcher))
                .collect::<Vec<_>>()
                .join(", ");
            let message = if expansion_count == 1 {
                format!(
                    "Feature step `{}` matches {} definitions",
                    step.text,
                    matches.len()
                )
            } else {
                format!(
                    "Scenario outline step has {expansion_count} ambiguous expansions matching {} definitions",
                    matches.len()
                )
            };
            findings.push(Finding {
                rule: Rule::AmbiguousStep,
                severity,
                message,
                primary: step.location.clone(),
                related: matched_definitions
                    .iter()
                    .map(|definition| definition.location.clone())
                    .collect(),
                evidence: FindingEvidence {
                    matcher_similarity: None,
                    handler_similarity: None,
                    matcher_difference: matcher_list,
                    handler_evidence:
                        "More than one definition accepts at least one concrete feature step"
                            .to_owned(),
                    comparison: None,
                },
                suggested_action: "Make the matchers mutually exclusive".to_owned(),
                suppression: suppressions
                    .find_reason(Rule::AmbiguousStep, &matched_indices)
                    .map(|reason| crate::model::Suppression {
                        reason: reason.to_owned(),
                    }),
            });
        }
    }
    FeatureUsageOutcome {
        used,
        operational_errors,
    }
}

pub(super) fn analyze_unused(
    definitions: &[StepDefinition],
    used: &BTreeSet<usize>,
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
) {
    let severity = config.severity(Rule::UnusedDefinition);
    if severity == Severity::Off {
        return;
    }
    for (index, definition) in definitions.iter().enumerate() {
        if used.contains(&index) {
            continue;
        }
        findings.push(Finding {
            rule: Rule::UnusedDefinition,
            severity,
            message: format!(
                "Step definition `{}` is not used by the feature corpus",
                definition.matcher
            ),
            primary: definition.location.clone(),
            related: Vec::new(),
            evidence: FindingEvidence {
                matcher_similarity: None,
                handler_similarity: None,
                matcher_difference: "No discovered feature step matched this definition".to_owned(),
                handler_evidence: String::new(),
                comparison: None,
            },
            suggested_action: "Remove the definition or add the missing feature usage".to_owned(),
            suppression: suppressions
                .find_reason(Rule::UnusedDefinition, &[index])
                .map(|reason| crate::model::Suppression {
                    reason: reason.to_owned(),
                }),
        });
    }
}

fn compile_matcher(definition: &StepDefinition) -> (CompiledMatcher, Option<String>) {
    match definition.matcher_kind {
        MatcherKind::RegularExpression => {
            if definition.matcher.len() > MAX_REGEX_PATTERN_BYTES {
                return (
                    CompiledMatcher {
                        regex: None,
                        authoritative: true,
                    },
                    Some(regex_limit_message(definition)),
                );
            }
            let flags: String = definition
                .matcher_flags
                .chars()
                .filter(|flag| matches!(flag, 'i' | 'm' | 's' | 'u'))
                .collect();
            let expression = if flags.is_empty() {
                definition.matcher.clone()
            } else {
                format!("(?{flags}:{})", definition.matcher)
            };
            let (regex, error) = compile_definition_regex(&expression, definition);
            (
                CompiledMatcher {
                    regex,
                    authoritative: true,
                },
                error,
            )
        }
        MatcherKind::CucumberExpression => {
            match cucumber_regex_expression(&definition.matcher) {
                CucumberRegexExpression::Compiled(expression) => {
                    let (regex, error) = compile_definition_regex(&expression, definition);
                    (
                        CompiledMatcher {
                            regex,
                            authoritative: true,
                        },
                        error,
                    )
                }
                CucumberRegexExpression::ResourceLimit => (
                    CompiledMatcher {
                        regex: None,
                        authoritative: true,
                    },
                    Some(regex_limit_message(definition)),
                ),
                CucumberRegexExpression::Unsupported => {
                    // Unknown project-defined parameter types cannot be resolved without loading
                    // runtime code. The permissive fallback is useful for avoiding false unused
                    // reports, but cannot prove runtime ambiguity. Oversized input has already
                    // returned ResourceLimit above, so fallback construction remains bounded.
                    let (regex, error) = compile_definition_regex(
                        &fallback_cucumber_expression_regex(&definition.matcher),
                        definition,
                    );
                    (
                        CompiledMatcher {
                            regex,
                            authoritative: false,
                        },
                        error,
                    )
                }
            }
        }
    }
}

fn cucumber_regex_expression(matcher: &str) -> CucumberRegexExpression {
    if matcher.len() > MAX_REGEX_PATTERN_BYTES {
        return CucumberRegexExpression::ResourceLimit;
    }
    let Ok(expression) = cucumber_expressions::Expression::parse(matcher) else {
        return CucumberRegexExpression::Unsupported;
    };
    let mut expanded = String::with_capacity(matcher.len().min(MAX_REGEX_PATTERN_BYTES));
    for character in expression.into_regex_char_iter() {
        let Ok(character) = character else {
            return CucumberRegexExpression::Unsupported;
        };
        if expanded.len().saturating_add(character.len_utf8()) > MAX_REGEX_PATTERN_BYTES {
            return CucumberRegexExpression::ResourceLimit;
        }
        expanded.push(character);
    }
    CucumberRegexExpression::Compiled(expanded)
}

fn compile_definition_regex(
    expression: &str,
    definition: &StepDefinition,
) -> (Option<Regex>, Option<String>) {
    if expression.len() > MAX_REGEX_PATTERN_BYTES {
        return (None, Some(regex_limit_message(definition)));
    }
    match compile_regex(expression) {
        Ok(regex) => (Some(regex), None),
        Err(regex::Error::CompiledTooBig(_)) => (None, Some(regex_limit_message(definition))),
        Err(_) => (None, None),
    }
}

fn regex_limit_message(definition: &StepDefinition) -> String {
    format!(
        "step matcher at {}:{}:{} exceeds the {}-byte regex resource limit; simplify the matcher or remove its source from definition discovery",
        definition.location.path.display(),
        definition.location.line,
        definition.location.column,
        MAX_REGEX_PATTERN_BYTES
    )
}

fn fallback_cucumber_expression_regex(expression: &str) -> String {
    let mut output = String::from("^");
    let mut offset = 0;
    for capture in FALLBACK_PLACEHOLDER.captures_iter(expression) {
        let whole = capture.get(0).expect("whole match");
        output.push_str(&fallback_cucumber_literal_regex(
            &expression[offset..whole.start()],
        ));
        output.push_str(match &capture[1] {
            "int" => r"-?\d+",
            "float" => r"-?(?:\d+\.)?\d+",
            "word" => r"\S+",
            "string" => r#"(?:"[^"]*"|'[^']*')"#,
            _ => r".+",
        });
        offset = whole.end();
    }
    output.push_str(&fallback_cucumber_literal_regex(&expression[offset..]));
    output.push('$');
    output
}

fn fallback_cucumber_literal_regex(literal: &str) -> String {
    let mut output = String::new();
    let characters: Vec<_> = literal.chars().collect();
    let mut offset = 0;
    while offset < characters.len() {
        if characters[offset].is_whitespace() {
            let start = offset;
            while offset < characters.len() && characters[offset].is_whitespace() {
                offset += 1;
            }
            output.push_str(&regex::escape(
                &characters[start..offset].iter().collect::<String>(),
            ));
            continue;
        }
        let start = offset;
        while offset < characters.len() && !characters[offset].is_whitespace() {
            offset += 1;
        }
        output.push_str(&fallback_cucumber_token_regex(
            &characters[start..offset].iter().collect::<String>(),
        ));
    }
    output
}

fn fallback_cucumber_token_regex(token: &str) -> String {
    let alternatives: Vec<_> = token.split('/').collect();
    if alternatives.len() > 1 {
        return format!(
            "(?:{})",
            alternatives
                .iter()
                .map(|alternative| fallback_cucumber_token_regex(alternative))
                .collect::<Vec<_>>()
                .join("|")
        );
    }

    let mut output = String::new();
    let characters: Vec<_> = token.chars().collect();
    let mut offset = 0;
    while offset < characters.len() {
        if characters[offset] == '(' {
            if let Some(end) = characters[offset + 1..]
                .iter()
                .position(|character| *character == ')')
                .map(|index| offset + index + 1)
            {
                let optional: String = characters[offset + 1..end].iter().collect();
                output.push_str("(?:");
                output.push_str(&regex::escape(&optional));
                output.push_str(")?");
                offset = end + 1;
                continue;
            }
            output.push_str(r"\(");
            offset += 1;
            continue;
        }
        let start = offset;
        while offset < characters.len() && characters[offset] != '(' {
            offset += 1;
        }
        output.push_str(&regex::escape(
            &characters[start..offset].iter().collect::<String>(),
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{SourceFile, SourceLanguage};
    use std::path::PathBuf;

    #[test]
    fn cucumber_expression_expansion_stops_at_the_regex_pattern_limit() {
        let matcher = ".".repeat(600_000);
        assert!(matches!(
            cucumber_regex_expression(&matcher),
            CucumberRegexExpression::ResourceLimit
        ));
    }

    #[test]
    fn regex_pattern_limit_is_inclusive() {
        let mut definition = crate::typescript::extract(
            "Given(/x/, () => work());",
            &SourceFile {
                path: PathBuf::from("steps.ts"),
                language: SourceLanguage::TypeScript,
            },
        )
        .unwrap()
        .remove(0);
        definition.matcher = format!(
            "(?x){}",
            " ".repeat(MAX_REGEX_PATTERN_BYTES.saturating_sub(4))
        );

        let (compiled, error) = compile_matcher(&definition);
        assert!(error.is_none());
        assert!(compiled.regex.is_some());
    }
}
