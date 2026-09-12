use crate::model::StepDefinition;

// Short labels need a slightly stricter composite gate because one changed token occupies a
// larger fraction of the matcher. The reported score is the stronger of normalized edit
// similarity and Jaro-Winkler, so the diagnostic and the gate cannot contradict each other.
const SHORT_MATCHER_MAX_CHARS: usize = 12;
const SHORT_SIMILARITY_GATE: f64 = 0.92;
const LONG_SIMILARITY_GATE: f64 = 0.90;

pub(super) fn matcher_similarity(left: &StepDefinition, right: &StepDefinition) -> f64 {
    let max_length = left
        .normalized_matcher
        .chars()
        .count()
        .max(right.normalized_matcher.chars().count());
    if max_length == 0 {
        return 1.0;
    }
    let distance = strsim::levenshtein(&left.normalized_matcher, &right.normalized_matcher);
    let edit_similarity = 1.0 - distance as f64 / max_length as f64;
    edit_similarity.max(strsim::jaro_winkler(
        &left.normalized_matcher,
        &right.normalized_matcher,
    ))
}

pub(super) fn is_near_matcher(
    left: &StepDefinition,
    right: &StepDefinition,
    similarity: f64,
) -> bool {
    if left.matcher_kind != right.matcher_kind {
        return false;
    }
    let length = left
        .normalized_matcher
        .chars()
        .count()
        .max(right.normalized_matcher.chars().count());
    let similarity_gate = if length <= SHORT_MATCHER_MAX_CHARS {
        SHORT_SIMILARITY_GATE
    } else {
        LONG_SIMILARITY_GATE
    };
    !has_polarity_conflict(&left.normalized_matcher, &right.normalized_matcher)
        && similarity >= similarity_gate
}

fn has_polarity_conflict(left: &str, right: &str) -> bool {
    let left = words(left);
    let right = words(right);
    const NEGATIONS: [&str; 8] = [
        "not", "no", "never", "cannot", "cant", "without", "unable", "neither",
    ];
    let left_negative = left.iter().any(|word| NEGATIONS.contains(&word.as_str()));
    let right_negative = right.iter().any(|word| NEGATIONS.contains(&word.as_str()));
    if left_negative != right_negative {
        return true;
    }
    left.iter().any(|left_word| {
        right.iter().any(|right_word| {
            prefixed_opposites(left_word, right_word) || prefixed_opposites(right_word, left_word)
        })
    })
}

fn words(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn prefixed_opposites(positive: &str, negative: &str) -> bool {
    positive.len() >= 4
        && ["in", "un", "non"]
            .iter()
            .any(|prefix| negative.strip_prefix(prefix) == Some(positive))
}

#[cfg(test)]
pub(super) fn handler_similarity(left: &StepDefinition, right: &StepDefinition) -> f64 {
    if !handler_runtime_compatible(left, right) {
        return 0.0;
    }
    let left_events = executable_behavior_events(left);
    let right_events = executable_behavior_events(right);
    handler_similarity_with_relationship(
        left.handler.alpha_normalized == right.handler.alpha_normalized,
        left.handler.structural == right.handler.structural
            && left.handler.behavior_signature == right.handler.behavior_signature,
        &left_events,
        &right_events,
    )
}

pub(super) fn handler_runtime_compatible(left: &StepDefinition, right: &StepDefinition) -> bool {
    match (method_semantics(left), method_semantics(right)) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn method_semantics(definition: &StepDefinition) -> Option<&str> {
    definition
        .handler
        .behavior_signature
        .first()
        .and_then(|event| event.strip_prefix("method:"))
}

#[cfg(test)]
fn executable_behavior_events(definition: &StepDefinition) -> Vec<&str> {
    definition
        .handler
        .behavior_signature
        .iter()
        .filter(|event| !event.starts_with("method:"))
        .map(String::as_str)
        .collect()
}

pub(super) fn handler_similarity_with_relationship<T: Eq>(
    same_alpha: bool,
    same_structural: bool,
    left_events: &[T],
    right_events: &[T],
) -> f64 {
    if same_alpha && left_events == right_events {
        return 1.0;
    }
    if same_structural {
        return 0.95;
    }
    let max_len = left_events.len().max(right_events.len());
    if max_len == 0 {
        return 0.0;
    }
    ordered_common_subsequence_len(left_events, right_events) as f64 / max_len as f64
}

pub(super) fn ordered_common_subsequence_len<T: Eq>(left: &[T], right: &[T]) -> usize {
    let mut previous = vec![0; right.len() + 1];
    for left_event in left {
        let mut current = vec![0; right.len() + 1];
        for (index, right_event) in right.iter().enumerate() {
            current[index + 1] = if left_event == right_event {
                previous[index] + 1
            } else {
                current[index].max(previous[index + 1])
            };
        }
        previous = current;
    }
    previous[right.len()]
}

pub(super) fn round_score(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Framework, HandlerFingerprint, MatcherKind, SourceLocation, StepDefinition,
    };

    fn definition(matcher: &str, kind: MatcherKind) -> StepDefinition {
        StepDefinition {
            matcher: matcher.to_owned(),
            normalized_matcher: matcher.to_owned(),
            matcher_kind: kind,
            matcher_flags: String::new(),
            handler: HandlerFingerprint {
                exact: String::new(),
                normalized: String::new(),
                alpha_normalized: String::new(),
                structural: String::new(),
                behavior_signature: Vec::new(),
                source_snippet: String::new(),
                comparable: true,
                trivial: false,
            },
            framework: Framework::Unknown,
            registration: "Given".to_owned(),
            location: SourceLocation::new("steps.ts", 1, 1, 1, 1),
            inline_suppressions: Vec::new(),
        }
    }

    #[test]
    fn matcher_gate_rejects_mixed_syntax_and_polarity_conflicts() {
        let expression = definition("account is active", MatcherKind::CucumberExpression);
        let regex = definition("account is active", MatcherKind::RegularExpression);
        assert!(!is_near_matcher(&expression, &regex, 1.0));

        let inactive = definition("account is inactive", MatcherKind::CucumberExpression);
        assert!(!is_near_matcher(&expression, &inactive, 1.0));

        let not_active = definition("account is not active", MatcherKind::CucumberExpression);
        assert!(!is_near_matcher(&expression, &not_active, 1.0));
    }

    #[test]
    fn similarity_helpers_cover_empty_exact_structural_and_lcs_paths() {
        let empty = definition("", MatcherKind::CucumberExpression);
        assert_eq!(matcher_similarity(&empty, &empty), 1.0);
        assert_eq!(
            handler_similarity_with_relationship::<u8>(true, false, &[], &[]),
            1.0
        );
        assert_eq!(
            handler_similarity_with_relationship::<u8>(false, true, &[], &[]),
            0.95
        );
        assert_eq!(
            handler_similarity_with_relationship::<u8>(false, false, &[], &[]),
            0.0
        );
        assert_eq!(ordered_common_subsequence_len(&[1, 2, 3], &[2, 3, 4]), 2);
        assert_eq!(round_score(0.123_6), 0.124);
    }

    #[test]
    fn method_metadata_is_a_compatibility_gate_not_similarity_evidence() {
        let mut left = definition("left", MatcherKind::CucumberExpression);
        left.handler.alpha_normalized = "left-alpha".to_owned();
        left.handler.structural = "left-structure".to_owned();
        left.handler.behavior_signature = vec![
            "method:instance sync".to_owned(),
            "assert:expect#toBe:subject:ready".to_owned(),
        ];

        let mut conflicting_assertion = left.clone();
        conflicting_assertion.handler.alpha_normalized = "right-alpha".to_owned();
        conflicting_assertion.handler.structural = "right-structure".to_owned();
        conflicting_assertion.handler.behavior_signature[1] =
            "assert:expect#toBe:subject:idle".to_owned();
        assert_eq!(handler_similarity(&left, &conflicting_assertion), 0.0);

        let mut incompatible_method = left.clone();
        incompatible_method.handler.alpha_normalized = "async-alpha".to_owned();
        incompatible_method.handler.structural = "async-structure".to_owned();
        incompatible_method.handler.behavior_signature[0] = "method:async".to_owned();
        assert_eq!(handler_similarity(&left, &incompatible_method), 0.0);

        let mut function_handler = left.clone();
        function_handler.handler.alpha_normalized = "function-alpha".to_owned();
        function_handler.handler.structural = "function-structure".to_owned();
        function_handler.handler.behavior_signature.remove(0);
        assert_eq!(handler_similarity(&left, &function_handler), 1.0);
    }
}
