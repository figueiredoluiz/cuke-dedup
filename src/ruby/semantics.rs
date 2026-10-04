//! Ruby comparison policy and native regex execution contract.

use crate::source_adapter::semantics::{
    AnalysisProfile, CaptureContext, ComparisonDomain, HandlerDomain, RegexDialect,
};

struct RubyRegex;
static REGEX: RubyRegex = RubyRegex;

impl RegexDialect for RubyRegex {
    fn expression(&self, matcher: &str, flags: &str) -> Option<String> {
        super::regex_expression(matcher, flags)
    }
    fn normalized_flags<'a>(&self, flags: &'a str) -> Option<&'a str> {
        Some(flags)
    }
    fn similarity_flags_match(&self, left: &str, right: &str) -> bool {
        left == right
    }
}

/// Enables event overlap only for complete Ruby effect streams.
pub(crate) fn profile(definition: &crate::model::StepDefinition) -> AnalysisProfile {
    let complete_events = definition
        .handler
        .behavior_signature
        .iter()
        .any(|event| event == "method:ruby:complete-events");
    AnalysisProfile {
        comparison_domain: ComparisonDomain::Ruby,
        handler_domain: HandlerDomain::Shared,
        global_matchers: true,
        near_requires_same_handler: !complete_events,
        event_similarity: complete_events,
        indirect_usage: true,
        dialect: &REGEX,
        capture_context: |definition| {
            if definition.method_semantics().is_some_and(|context| {
                context == "ruby:lexical-file" || context.starts_with("ruby:lexical-file:")
            }) {
                CaptureContext::LexicalFile
            } else {
                CaptureContext::Unrestricted
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rule;
    use crate::source_adapter::{SourceFile, SourceLanguage};

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
                assert!(!profile(definition).event_similarity);
                assert!(profile(definition).near_requires_same_handler);
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
                    Rule::DuplicateHandler
                        | Rule::NearDuplicateStep
                        | Rule::ParameterizationCandidate
                )),
                expected
            );
        }
    }
}
