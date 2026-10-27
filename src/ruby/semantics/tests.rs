use crate::model::{EffectStream, Rule};
use crate::source_adapter::semantics::CaptureContext;
use crate::source_adapter::{SourceFile, SourceLanguage};

/// One Ruby definition whose typed handler facts are reset to the defaults.
fn plain_definition() -> crate::model::StepDefinition {
    let file = SourceFile {
        path: "steps.rb".into(),
        language: SourceLanguage::Ruby,
    };
    let (mut extracted, _) = super::super::extract("Given('a') { work() }", &file).unwrap();
    let mut definition = extracted.definitions.remove(0);
    definition.handler.behavior_signature.clear();
    definition.handler.effect_stream = EffectStream::Partial;
    definition.handler.capture_scope = None;
    definition
}

/// Each stream regime selects its own flag combination.
#[test]
fn profile_flags_follow_the_typed_effect_stream() {
    for (stream, same_handler, event_similarity, distinct_events) in [
        (EffectStream::Partial, true, false, false),
        (EffectStream::Complete, false, true, false),
        (EffectStream::ActionOnly, false, true, true),
    ] {
        let mut definition = plain_definition();
        definition.handler.effect_stream = stream;
        let profile = definition.analysis_profile();
        assert_eq!(
            (
                profile.requires_identical_handler,
                profile.scores_event_overlap,
                profile.excludes_identical_event_streams
            ),
            (same_handler, event_similarity, distinct_events),
            "{stream:?}"
        );
    }
}

/// The capture context is lexical-file exactly when a capture scope is declared, whatever the
/// key.
#[test]
fn capture_context_follows_the_typed_capture_scope() {
    for (scope, lexical) in [(None, false), (Some(""), true), (Some("4:9"), true)] {
        let mut definition = plain_definition();
        definition.handler.capture_scope = scope.map(str::to_owned);
        assert_eq!(
            definition.capture_context() == CaptureContext::LexicalFile,
            lexical,
            "{scope:?}"
        );
    }
}

/// Checks exact-handler fallback for legacy streams without losing positive findings.
#[test]
fn legacy_partial_events_keep_exact_policy_and_positive_controls() {
    let root = tempfile::tempdir().unwrap();
    let config = crate::config::Config::load(root.path(), Default::default()).unwrap();
    let file = SourceFile {
        path: root.path().join("steps.rb"),
        language: SourceLanguage::Ruby,
    };
    for (second, expected) in [("first", true), ("second", false)] {
        let source = format!("Given('the parcel status is verified') {{ first() }}; Then('the parcel status is now verified') {{ {second}() }}");
        let (mut extracted, _) = super::super::extract(&source, &file).unwrap();
        for definition in &mut extracted.definitions {
            definition.handler.behavior_signature = vec!["call:shared".into()];
            definition.handler.effect_stream = EffectStream::Partial;
            assert!(!definition.analysis_profile().scores_event_overlap);
            assert!(definition.analysis_profile().requires_identical_handler);
        }
        let outcome = crate::analysis::analyze_with_step_usage(
            extracted.definitions,
            vec![],
            &config,
            &Default::default(),
        )
        .unwrap();
        assert!(outcome.incomplete.is_empty());
        let result = outcome.result;
        assert_eq!(
            result.findings.iter().any(|finding| matches!(
                finding.rule,
                Rule::DuplicateHandler | Rule::NearDuplicateStep | Rule::ParameterizationCandidate
            )),
            expected
        );
    }
}
