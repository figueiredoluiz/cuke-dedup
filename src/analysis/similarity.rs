use super::string_metrics::matcher_text_similarity;
#[cfg(test)]
use crate::model::BehaviorEventRef;
use crate::model::StepDefinition;

// Short labels need a slightly stricter composite gate because one changed token occupies a
// larger fraction of the matcher. The reported score is the stronger of normalized edit
// similarity and Jaro-Winkler, so the diagnostic and the gate cannot contradict each other.
const SHORT_MATCHER_MAX_CHARS: usize = 12;
const SHORT_SIMILARITY_GATE: f64 = 0.92;
const LONG_SIMILARITY_GATE: f64 = 0.90;

/// The normalized matcher text that every matcher comparison in this module reads.
fn matcher_comparison_text(definition: &StepDefinition) -> &str {
    &definition.normalized_matcher
}

/// The larger of Levenshtein similarity (normalized by the longer char count) and Jaro-Winkler
/// over the normalized matchers; two empty matchers score 1.0.
pub(super) fn matcher_similarity(left: &StepDefinition, right: &StepDefinition) -> f64 {
    matcher_text_similarity(
        matcher_comparison_text(left),
        matcher_comparison_text(right),
    )
}

/// Lowercased words of the normalized matcher, computed once per definition per analysis and
/// passed to the word-based helpers below.
pub(super) fn matcher_words(definition: &StepDefinition) -> Vec<String> {
    words(matcher_comparison_text(definition))
}

/// Whether compatible matchers reach the wording gate: 0.92 when the longer normalized matcher has
/// at most 12 chars, 0.90 otherwise. `similarity` is taken as given, not recomputed; the word
/// slices are each definition's [`matcher_words`].
pub(super) fn is_near_matcher(
    left: &StepDefinition,
    right: &StepDefinition,
    left_words: &[String],
    right_words: &[String],
    similarity: f64,
) -> bool {
    if !matchers_compatible(left, right, left_words, right_words) {
        return false;
    }
    let length = matcher_comparison_text(left)
        .chars()
        .count()
        .max(matcher_comparison_text(right).chars().count());
    let similarity_gate = if length <= SHORT_MATCHER_MAX_CHARS {
        SHORT_SIMILARITY_GATE
    } else {
        LONG_SIMILARITY_GATE
    };
    similarity >= similarity_gate
}

// Floor below which two matchers are too dissimilar to be worth proposing a shared parameter for.
const PARAMETERIZATION_SIMILARITY_GATE: f64 = 0.6;

// A parameterization candidate should differ in a small, value-like region — the token or two that
// would become the parameter — not in a whole multi-word phrase describing different behavior. Real
// corpus false positives paired matchers whose only shared part was a prefix ("the AI chatbot
// toggle button should be visible" vs "…window should be open"); legitimate ones differ in a single
// value ("switches to dark theme" vs "…light theme"). Handlers alone cannot tell these apart because
// literal erasure collapses both into one structural class, so the matcher difference is the signal.
const MAX_PARAMETERIZED_CHANGE_WORDS: usize = 3;

/// Whether two matchers may describe the same step parameterized by a value: same kind, no polarity
/// conflict, similar enough, and differing only in a bounded value-like region. The word slices
/// are each definition's [`matcher_words`].
pub(super) fn is_parameterizable_matcher(
    left: &StepDefinition,
    right: &StepDefinition,
    left_words: &[String],
    right_words: &[String],
    similarity: f64,
) -> bool {
    matchers_compatible(left, right, left_words, right_words)
        && similarity >= PARAMETERIZATION_SIMILARITY_GATE
        && matcher_change_word_span(left_words, right_words) <= MAX_PARAMETERIZED_CHANGE_WORDS
}

/// Same matcher syntax and no negation/opposite-word conflict between the matchers' words — the
/// shared prerequisite for treating two matchers as variants of one step.
fn matchers_compatible(
    left: &StepDefinition,
    right: &StepDefinition,
    left_words: &[String],
    right_words: &[String],
) -> bool {
    left.matcher_kind == right.matcher_kind
        && left
            .analysis_profile()
            .dialect()
            .similarity_flags_match(&left.matcher_flags, &right.matcher_flags)
        && !has_polarity_conflict(left_words, right_words)
}

/// The number of words that differ between two matchers' word lists after stripping their common
/// leading and trailing words — the size of the region a single parameter would have to stand in
/// for.
fn matcher_change_word_span(left: &[String], right: &[String]) -> usize {
    let min = left.len().min(right.len());
    let mut prefix = 0;
    while prefix < min && left[prefix] == right[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < min - prefix && left[left.len() - 1 - suffix] == right[right.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let left_change = &left[prefix..left.len() - suffix];
    let right_change = &right[prefix..right.len() - suffix];
    // One parameter can stand in for a single contiguous change. A word shared between the trimmed
    // change regions splits them into two separate edits only when it sits *strictly inside* a
    // region — a word both before and after it — as "button" does in "red button shown" vs "blue
    // button hidden". A shared word that only ever sits at a region edge is an overlap within one
    // shifted value ("New York" vs "York City"), which a single parameter still covers.
    if splits_into_separate_changes(left_change, right_change) {
        return usize::MAX;
    }
    left_change.len().max(right_change.len())
}

/// Whether a word shared between the two change regions is flanked (has both a preceding and a
/// following word) in either region, marking two disjoint edits rather than one shifted value.
fn splits_into_separate_changes(left: &[String], right: &[String]) -> bool {
    let flanked = |region: &[String], word: &String| {
        region
            .iter()
            .enumerate()
            .any(|(index, candidate)| candidate == word && index > 0 && index + 1 < region.len())
    };
    left.iter()
        .any(|word| right.contains(word) && (flanked(left, word) || flanked(right, word)))
}

/// Whether exactly one word list contains a negation word, or any word pair is a prefixed
/// opposite. Negation on both sides is not a conflict by itself; a prefixed opposite still is.
fn has_polarity_conflict(left: &[String], right: &[String]) -> bool {
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

/// Lowercased runs of alphanumeric chars; every other char separates words.
fn words(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether `negative` is an enable/disable inflection of `positive`, or `positive` (4+ bytes)
/// prefixed by `in`, `un`, `non`, `dis`, `il`, `im` or `ir`. Direction matters; callers check both.
fn prefixed_opposites(positive: &str, negative: &str) -> bool {
    matches!(
        (positive, negative),
        ("enable", "disable")
            | ("enables", "disables")
            | ("enabled", "disabled")
            | ("enabling", "disabling")
    ) || positive.len() >= 4
        && ["in", "un", "non", "dis", "il", "im", "ir"]
            .iter()
            .any(|prefix| negative.strip_prefix(prefix) == Some(positive))
}

/// Handler similarity of two definitions: 0.0 when runtime-incompatible, else the relationship
/// score over their behaviour events with method events removed.
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
            && left.handler.behavior_identity() == right.handler.behavior_identity(),
        event_overlap_available(left, right),
        &left_events,
        &right_events,
    )
}

// Partial streams preserve identity checks but cannot measure whole-handler overlap.
/// Whether both definitions' profiles allow event-overlap scoring.
pub(super) fn event_overlap_available(left: &StepDefinition, right: &StepDefinition) -> bool {
    left.analysis_profile().scores_event_overlap && right.analysis_profile().scores_event_overlap
}

/// Whether two handlers can run in the same runtime context. Rejects differing paths when either
/// captures lexical file context, differing handler domains, and differing runtime contexts when
/// both handlers have one; a handler without a runtime context does not gate.
pub(super) fn handler_runtime_compatible(left: &StepDefinition, right: &StepDefinition) -> bool {
    use crate::source_adapter::semantics::CaptureContext;
    if (left.capture_context() == CaptureContext::LexicalFile
        || right.capture_context() == CaptureContext::LexicalFile)
        && left.location.path != right.location.path
    {
        return false;
    }
    if left.analysis_profile().handler_scope != right.analysis_profile().handler_scope {
        return false;
    }
    match (left.runtime_context(), right.runtime_context()) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

/// The handler's behaviour events parsed from the legacy signature, without method events.
#[cfg(test)]
fn executable_behavior_events(definition: &StepDefinition) -> Vec<BehaviorEventRef<'_>> {
    definition
        .handler
        .behavior_signature
        .iter()
        .map(|event| BehaviorEventRef::from_legacy(event))
        .filter(|event| !event.is_method())
        .collect()
}

/// 1.0 for same alpha form with equal events, else 0.95 for same structure, else 0.0 when event
/// similarity is unavailable or both streams are empty, else ordered event overlap over the longer
/// stream. Same alpha form with differing events falls through to the later checks.
pub(super) fn handler_similarity_with_relationship<T: Eq>(
    same_alpha: bool,
    same_structural: bool,
    event_similarity: bool,
    left_events: &[T],
    right_events: &[T],
) -> f64 {
    if same_alpha && left_events == right_events {
        return 1.0;
    }
    if same_structural {
        return 0.95;
    }
    if !event_similarity {
        return 0.0;
    }
    let max_len = left_events.len().max(right_events.len());
    if max_len == 0 {
        return 0.0;
    }
    ordered_common_subsequence_len(left_events, right_events) as f64 / max_len as f64
}

/// Length of the longest common subsequence of the two slices.
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

/// Rounds to three decimals, halves away from zero.
pub(super) fn round_score(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests;
