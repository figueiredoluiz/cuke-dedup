//! Framework policy and regex execution owned by the JS/TS frontend.

use crate::model::Framework;
use crate::source_adapter::semantics::{
    AnalysisProfile, ComparisonDomain, HandlerDomain, RegexDialect,
};

struct JavaScriptRegex;
static REGEX: JavaScriptRegex = JavaScriptRegex;

impl RegexDialect for JavaScriptRegex {
    /// Builds the Rust-compatible expression used for JavaScript regex usage matching.
    fn expression(&self, matcher: &str, flags: &str) -> Option<String> {
        super::rust_regex_expression(matcher, flags)
    }
    /// Returns no separate flag normalization for the JavaScript comparison profile.
    fn normalized_flags<'a>(&self, _: &'a str) -> Option<&'a str> {
        None
    }
    /// Allows similarity comparisons regardless of the supplied flag strings.
    fn similarity_flags_match(&self, _: &str, _: &str) -> bool {
        true
    }
}

/// Comparison policy of a JavaScript or TypeScript framework.
pub(crate) fn profile(framework: Framework) -> AnalysisProfile {
    AnalysisProfile {
        comparison_domain: ComparisonDomain::EcmaScript,
        handler_domain: match framework {
            Framework::JestCucumber => HandlerDomain::ScenarioLocal,
            Framework::VitestCucumber => HandlerDomain::PluginGlobal,
            _ => HandlerDomain::Shared,
        },
        global_matchers: framework != Framework::JestCucumber,
        near_requires_same_handler: false,
        event_similarity: true,
        near_requires_distinct_events: false,
        indirect_usage: false,
        dialect: &REGEX,
    }
}
