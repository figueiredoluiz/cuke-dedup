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

pub(super) fn handler_similarity(left: &StepDefinition, right: &StepDefinition) -> f64 {
    if left.handler.alpha_normalized == right.handler.alpha_normalized {
        return 1.0;
    }
    if left.handler.structural == right.handler.structural {
        return 0.95;
    }
    let left_events = &left.handler.behavior_signature;
    let right_events = &right.handler.behavior_signature;
    let max_len = left_events.len().max(right_events.len());
    if max_len == 0 {
        return 0.0;
    }
    ordered_common_subsequence_len(left_events, right_events) as f64 / max_len as f64
}

pub(super) fn ordered_common_subsequence_len(left: &[String], right: &[String]) -> usize {
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
