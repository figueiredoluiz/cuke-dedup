//! Framework policy and regex execution owned by the JS/TS frontend.

use crate::matcher::{IdentityFlags, RegexKey};
use crate::model::Framework;
use crate::source_adapter::semantics::{
    DomainSpec, HandlerScope, RegexDialect, StepMatching, StreamEvidence,
};
use crate::source_adapter::{AnalysisPolicy, FrameworkContract};
use crate::typescript::frameworks::FRAMEWORK_CONTRACTS;

struct JavaScriptRegex;
static REGEX: JavaScriptRegex = JavaScriptRegex;

impl RegexDialect for JavaScriptRegex {
    /// Builds the Rust-compatible expression used for JavaScript regex usage matching.
    fn expression(&self, matcher: &str, flags: &str) -> Option<String> {
        super::rust_regex_expression(matcher, flags)
    }
    /// Keys on the canonical source pattern, with the identity flags written into the key text.
    fn regex_key(&self) -> RegexKey {
        RegexKey::CanonicalSource
    }
    /// `i`, `m`, `s`, `u` and `v` are identity; `g`, `y` and `d` never separate two matchers.
    fn identity_flags(&self) -> IdentityFlags {
        IdentityFlags::Listed("imsuv")
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

/// An ECMAScript policy: every handler's event stream counts as complete.
const fn ecmascript(handler_scope: HandlerScope, step_matching: StepMatching) -> AnalysisPolicy {
    AnalysisPolicy::new(
        &DOMAIN,
        handler_scope,
        step_matching,
        StreamEvidence::Complete,
        false,
    )
}

/// Steps local to their scenario callback and matched by position.
pub(super) const SCENARIO_LOCAL: AnalysisPolicy =
    ecmascript(HandlerScope::Local, StepMatching::Positional);
/// Steps in a framework's own global registry, matched by text.
pub(super) const ISOLATED: AnalysisPolicy =
    ecmascript(HandlerScope::Isolated, StepMatching::ByText);

/// The comparison policy `framework`'s contract declares, or the shared text-matched pool.
pub(crate) fn policy(framework: Framework) -> AnalysisPolicy {
    FrameworkContract::policy_of(&FRAMEWORK_CONTRACTS, framework, default_policy())
}

/// The policy of a definition with no framework-specific scope: [`Framework::UNKNOWN`]'s.
pub(crate) const fn default_policy() -> AnalysisPolicy {
    ecmascript(HandlerScope::Shared, StepMatching::ByText)
}
