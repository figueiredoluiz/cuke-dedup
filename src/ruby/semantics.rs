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

pub(crate) fn profile() -> AnalysisProfile {
    AnalysisProfile {
        comparison_domain: ComparisonDomain::Ruby,
        handler_domain: HandlerDomain::Shared,
        global_matchers: true,
        near_requires_same_handler: true,
        event_similarity: false,
        indirect_usage: true,
        dialect: &REGEX,
        capture_context: |definition| {
            if definition.method_semantics() == Some("ruby:lexical-file") {
                CaptureContext::LexicalFile
            } else {
                CaptureContext::Unrestricted
            }
        },
    }
}
