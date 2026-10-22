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
            .dialect
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
            && left.handler.behavior_signature == right.handler.behavior_signature,
        event_similarity_available(left, right),
        &left_events,
        &right_events,
    )
}

// Partial streams preserve identity checks but cannot measure whole-handler overlap.
/// Whether both definitions' profiles allow event-overlap scoring.
pub(super) fn event_similarity_available(left: &StepDefinition, right: &StepDefinition) -> bool {
    left.analysis_profile().event_similarity && right.analysis_profile().event_similarity
}

/// Whether two handlers can run in the same runtime context. Rejects differing paths when either
/// captures lexical file context, differing handler domains, and differing method semantics when
/// both signatures start with a method event; otherwise method semantics do not gate.
pub(super) fn handler_runtime_compatible(left: &StepDefinition, right: &StepDefinition) -> bool {
    use crate::source_adapter::semantics::CaptureContext;
    if (left.capture_context() == CaptureContext::LexicalFile
        || right.capture_context() == CaptureContext::LexicalFile)
        && left.location.path != right.location.path
    {
        return false;
    }
    if left.analysis_profile().handler_domain != right.analysis_profile().handler_domain {
        return false;
    }
    match (left.method_semantics(), right.method_semantics()) {
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
mod tests {
    use super::*;
    use crate::model::{
        Framework, HandlerFingerprint, MatcherKind, SourceLocation, StepDefinition,
    };

    /// An `Unknown`-framework definition using `matcher` as raw and normalized text, empty handler.
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
                effect_stream: Default::default(),
                capture_scope: None,
            },
            framework: Framework::Unknown,
            registration: "Given".to_owned(),
            location: SourceLocation::new("steps.ts", 1, 1, 1, 1),
            inline_suppressions: Vec::new(),
        }
    }

    /// `is_near_matcher` over each definition's own cached words.
    fn near(left: &StepDefinition, right: &StepDefinition, similarity: f64) -> bool {
        is_near_matcher(
            left,
            right,
            &matcher_words(left),
            &matcher_words(right),
            similarity,
        )
    }

    /// Similarity 1.0 still fails the near gate across matcher kinds, prefixed opposites and
    /// one-sided negation.
    #[test]
    fn matcher_gate_rejects_mixed_syntax_and_polarity_conflicts() {
        let expression = definition("account is active", MatcherKind::CucumberExpression);
        let regex = definition("account is active", MatcherKind::RegularExpression);
        assert!(!near(&expression, &regex, 1.0));

        let inactive = definition("account is inactive", MatcherKind::CucumberExpression);
        assert!(!near(&expression, &inactive, 1.0));

        let not_active = definition("account is not active", MatcherKind::CucumberExpression);
        assert!(!near(&expression, &not_active, 1.0));
    }

    /// The word helpers fed cached word lists return the answers the uncached string API returned
    /// before the cache existed (values recorded from that API), identically in both argument
    /// orders: punctuation-only, mixed case, negation on either side or both, prefixed opposites,
    /// a short word that is no prefixed opposite, two separate edits and one shifted value.
    #[test]
    fn cached_word_helpers_keep_pre_cache_answers() {
        // (polarity conflict, change span, compatible, near, parameterizable)
        let answers = |left: &str, right: &str| {
            let ordered = |left: &str, right: &str| {
                let left = definition(left, MatcherKind::CucumberExpression);
                let right = definition(right, MatcherKind::CucumberExpression);
                let (left_words, right_words) = (matcher_words(&left), matcher_words(&right));
                let similarity = matcher_similarity(&left, &right);
                (
                    has_polarity_conflict(&left_words, &right_words),
                    matcher_change_word_span(&left_words, &right_words),
                    matchers_compatible(&left, &right, &left_words, &right_words),
                    is_near_matcher(&left, &right, &left_words, &right_words, similarity),
                    is_parameterizable_matcher(
                        &left,
                        &right,
                        &left_words,
                        &right_words,
                        similarity,
                    ),
                )
            };
            let forward = ordered(left, right);
            assert_eq!(forward, ordered(right, left), "{left:?} / {right:?}");
            forward
        };
        assert_eq!(answers("!!! ---", "..."), (false, 0, true, false, false));
        assert_eq!(
            answers("Account Is ACTIVE", "account is active"),
            (false, 0, true, false, true)
        );
        assert_eq!(
            answers("the user can not log in", "the user can log in"),
            (true, 1, false, false, false)
        );
        assert_eq!(
            answers("the user can log in", "the user CANNOT log in"),
            (true, 1, false, false, false)
        );
        assert_eq!(
            answers("user is not active", "user is never active"),
            (false, 1, true, true, true)
        );
        assert_eq!(
            answers("account is active", "account is Inactive"),
            (true, 1, false, false, false)
        );
        assert_eq!(
            answers("feature is Disabled", "feature is enabled"),
            (true, 1, false, false, false)
        );
        assert_eq!(
            answers("the log is on", "the log is non"),
            (false, 1, true, true, true)
        );
        assert_eq!(
            answers("red button shown", "blue button hidden"),
            (false, usize::MAX, true, false, false)
        );
        assert_eq!(
            answers("New York", "York City"),
            (false, 2, true, false, false)
        );
        assert_eq!(
            answers("switches to dark theme", "switches to light theme"),
            (false, 1, true, true, true)
        );
    }
    /// Empty matchers score 1.0; same alpha with equal events scores 1.0 and same structure 0.95
    /// with or without event similarity; otherwise LCS overlap only when available, 0.0 for empty
    /// streams; LCS length; three-decimal rounding.
    #[test]
    fn similarity_helpers_cover_empty_exact_structural_and_lcs_paths() {
        let empty = definition("", MatcherKind::CucumberExpression);
        assert_eq!(matcher_similarity(&empty, &empty), 1.0);
        assert_eq!(
            handler_similarity_with_relationship::<u8>(true, false, true, &[], &[]),
            1.0
        );
        assert_eq!(
            handler_similarity_with_relationship::<u8>(false, true, true, &[], &[]),
            0.95
        );
        assert_eq!(
            handler_similarity_with_relationship::<u8>(false, false, true, &[], &[]),
            0.0
        );
        for available in [false, true] {
            for (alpha, structural, expected) in [
                (true, false, 1.0),
                (false, true, 0.95),
                (false, false, if available { 0.5 } else { 0.0 }),
            ] {
                let right = if alpha { &[1, 2][..] } else { &[1, 3][..] };
                assert_eq!(
                    handler_similarity_with_relationship(
                        alpha,
                        structural,
                        available,
                        &[1, 2],
                        right
                    ),
                    expected
                );
            }
        }
        assert_eq!(ordered_common_subsequence_len(&[1, 2, 3], &[2, 3, 4]), 2);
        assert_eq!(round_score(0.123_6), 0.124);
    }

    /// Differing method events score 0.0; shared method events add no overlap; a handler without a
    /// method event scores 1.0 against one with it when the remaining events match.
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
