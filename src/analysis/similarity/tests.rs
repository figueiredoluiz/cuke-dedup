use super::*;
use crate::model::{Framework, HandlerFingerprint, MatcherKind, SourceLocation, StepDefinition};
use crate::typescript::semantics::default_policy;

/// A `Framework::UNKNOWN` definition using `matcher` as raw and normalized text, empty handler.
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
        framework: Framework::UNKNOWN,
        analysis_policy: default_policy(),
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
                is_parameterizable_matcher(&left, &right, &left_words, &right_words, similarity),
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
                handler_similarity_with_relationship(alpha, structural, available, &[1, 2], right),
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

/// Equal structure and events score 0.95 only while the effect stream and capture scope also
/// agree; with either differing, the pair falls through to event overlap (1.0 here).
#[test]
fn structural_handler_similarity_requires_equal_behavior_identity() {
    let mut left = definition("left", MatcherKind::CucumberExpression);
    left.handler.alpha_normalized = "left-alpha".to_owned();
    left.handler.structural = "structure".to_owned();
    left.handler.behavior_signature = vec!["call:work".to_owned()];
    let mut same = left.clone();
    same.handler.alpha_normalized = "right-alpha".to_owned();
    assert_eq!(handler_similarity(&left, &same), 0.95);
    let mut other_stream = same.clone();
    other_stream.handler.effect_stream = crate::model::EffectStream::ActionOnly;
    let mut other_scope = same.clone();
    other_scope.handler.capture_scope = Some("4:9".to_owned());
    for other in [other_stream, other_scope] {
        assert_eq!(handler_similarity(&left, &other), 1.0);
    }
}

/// Handlers are runtime-compatible unless both are pinned to different contexts: capture
/// scopes, leading method events, or an effect stream that observed no events. A handler
/// without a pin, including an unmodelled one, never gates.
#[test]
fn runtime_context_gates_handler_compatibility() {
    use crate::model::EffectStream::{self, ActionOnly, Complete, Partial};
    /// Scope key, events and effect stream of one handler.
    type Handler = (Option<&'static str>, &'static [&'static str], EffectStream);
    let called: &[&str] = &["call:work"];
    let (bare, bound, unscoped) = (
        (Some(""), called, ActionOnly),
        (Some("4:9"), called, ActionOnly),
        (None, called, ActionOnly),
    );
    let (empty, unmodelled): (Handler, Handler) = ((None, &[], ActionOnly), (None, &[], Partial));
    for (name, left, right, compatible) in [
        ("one bare scope", bare, bare, true),
        ("bare against bound scope", bare, bound, false),
        ("one bound scope", bound, bound, true),
        (
            "two bound scopes",
            bound,
            (Some("7:12"), called, ActionOnly),
            false,
        ),
        ("bound against unscoped", bound, unscoped, true),
        ("two unscoped", unscoped, unscoped, true),
        ("scope against modelled empty stream", bound, empty, false),
        ("two modelled empty streams", empty, empty, true),
        (
            "modelled against unmodelled empty stream",
            empty,
            unmodelled,
            true,
        ),
        (
            "scope against unmodelled empty stream",
            bound,
            unmodelled,
            true,
        ),
        (
            "modelled empty stream against events",
            empty,
            unscoped,
            true,
        ),
        (
            "complete against action-only empty stream",
            (None, &[], Complete),
            empty,
            false,
        ),
        (
            "method against other method",
            (None, &["method:async"], Partial),
            (None, &["method:sync"], Partial),
            false,
        ),
        (
            "method against itself",
            (None, &["method:async"], Partial),
            (None, &["method:async"], Partial),
            true,
        ),
    ] {
        let build = |(scope, events, stream): Handler| {
            let mut definition = definition("matcher", MatcherKind::CucumberExpression);
            definition.handler.capture_scope = scope.map(str::to_owned);
            definition.handler.behavior_signature =
                events.iter().map(|event| (*event).to_owned()).collect();
            definition.handler.effect_stream = stream;
            definition
        };
        let (left, right) = (build(left), build(right));
        assert_eq!(
            handler_runtime_compatible(&left, &right),
            compatible,
            "{name}"
        );
        assert_eq!(
            handler_runtime_compatible(&right, &left),
            compatible,
            "{name}, reversed"
        );
    }
}
