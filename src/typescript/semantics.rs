//! Framework policy and regex execution owned by the JS/TS frontend.

use crate::model::Framework;
use crate::source_adapter::semantics::{
    DomainSpec, HandlerScope, RegexDialect, StepMatching, StreamEvidence,
};
use crate::source_adapter::AnalysisPolicy;

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

static DOMAIN: DomainSpec = DomainSpec {
    label: "ecmascript",
    dialect: &REGEX,
};

/// The comparison policy of a JavaScript or TypeScript framework: Jest-Cucumber steps are local to
/// their scenario callback and matched by position, vitest-cucumber-plugin registers into its own
/// global registry, and every other framework shares one text-matched pool. Every handler's event
/// stream counts as complete.
pub(crate) const fn policy(framework: Framework) -> AnalysisPolicy {
    let (handler_scope, step_matching) = match framework {
        Framework::JestCucumber => (HandlerScope::Local, StepMatching::Positional),
        Framework::VitestCucumber => (HandlerScope::Isolated, StepMatching::ByText),
        _ => (HandlerScope::Shared, StepMatching::ByText),
    };
    AnalysisPolicy::new(
        &DOMAIN,
        handler_scope,
        step_matching,
        StreamEvidence::Complete,
        false,
    )
}

/// The policy of a definition with no framework-specific scope: [`Framework::Unknown`]'s.
pub(crate) const fn default_policy() -> AnalysisPolicy {
    policy(Framework::Unknown)
}
