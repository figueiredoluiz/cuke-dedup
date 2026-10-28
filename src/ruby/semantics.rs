//! Ruby comparison policy and native regex execution contract.

use crate::matcher::{IdentityFlags, RegexKey, TranslatedRegex};
use crate::source_adapter::semantics::{
    DomainSpec, HandlerScope, RegexDialect, StepMatching, StreamEvidence,
};
use crate::source_adapter::AnalysisPolicy;

struct RubyRegex;
static REGEX: RubyRegex = RubyRegex;

impl RegexDialect for RubyRegex {
    /// Translates the supported Ruby regex subset, compile-checked under the shared limits.
    fn translate(&self, matcher: &str, flags: &str) -> Option<TranslatedRegex> {
        super::regex_translation(matcher, flags).ok()
    }
    /// Keys on the translated pattern; the raw Ruby flag string partitions the keys.
    fn regex_key(&self) -> RegexKey {
        RegexKey::TranslatedBody
    }
    /// Every native flag is identity: the encoding flags `n` and `u` keep two matchers apart.
    fn identity_flags(&self) -> IdentityFlags {
        IdentityFlags::All
    }
    /// Require identical Ruby flag strings before matcher similarity comparison.
    fn similarity_flags_match(&self, left: &str, right: &str) -> bool {
        left == right
    }
    /// Tries a translated Ruby scoped array first, as every built-in dialect does.
    fn parameter_sample(&self, pattern: &str) -> Option<String> {
        crate::source_adapter::semantics::builtin_parameter_sample(pattern)
    }
}

static DOMAIN: DomainSpec = DomainSpec::new("ruby", &REGEX);

/// The Ruby comparison policy: one shared handler scope, text-matched steps, step invocation by
/// text, and the effect-stream regime each handler declares.
pub(crate) const fn policy() -> AnalysisPolicy {
    AnalysisPolicy::new(
        &DOMAIN,
        HandlerScope::Shared,
        StepMatching::ByText,
        StreamEvidence::Declared,
        true,
    )
}

#[cfg(test)]
mod tests;
