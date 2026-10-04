//! Internal compatibility profiles for extracted and caller-constructed definitions.

use crate::model::{BehaviorEventRef, Framework, StepDefinition};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComparisonDomain {
    EcmaScript,
    Ruby,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandlerDomain {
    Shared,
    ScenarioLocal,
    PluginGlobal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureContext {
    Unrestricted,
    LexicalFile,
}

pub(crate) trait RegexDialect: Sync {
    fn expression(&self, matcher: &str, flags: &str) -> Option<String>;
    fn normalized_flags<'a>(&self, flags: &'a str) -> Option<&'a str>;
    fn similarity_flags_match(&self, left: &str, right: &str) -> bool;

    fn identity_flags(&self, flags: &str) -> String {
        legacy_identity_flags(flags)
    }

    fn parameter_sample(&self, pattern: &str) -> Option<String> {
        // Preserve the existing witness subset, including translated scoped patterns.
        // The resulting witness must still match both compiled definitions.
        let mut trimmed = pattern.trim();
        loop {
            let inner = trimmed
                .strip_prefix("(?:")
                .or_else(|| trimmed.strip_prefix("(?m:"))
                .or_else(|| trimmed.strip_prefix("(?ms:"))
                .or_else(|| trimmed.strip_prefix('('));
            let Some(inner) = inner else { break };
            trimmed = inner.strip_suffix(')')?;
        }
        let first = trimmed.split('|').next()?;
        if first.is_empty()
            || first
                .chars()
                .any(|c| !c.is_alphanumeric() && !matches!(c, ' ' | '_' | '-'))
        {
            return None;
        }
        Some(first.to_owned())
    }
}

// This is the existing exact-identity protocol, including its projection for legacy Ruby
// inputs. Dialect-specific corrections must be a separately contracted behavior change.
pub(crate) fn legacy_identity_flags(flags: &str) -> String {
    ['i', 'm', 's', 'u', 'v']
        .into_iter()
        .filter(|f| flags.contains(*f))
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) struct AnalysisProfile {
    pub(crate) comparison_domain: ComparisonDomain,
    pub(crate) handler_domain: HandlerDomain,
    pub(crate) global_matchers: bool,
    // Wording findings may require exact handler identity, even for structural matches.
    pub(crate) near_requires_same_handler: bool,
    // Partial event streams cannot measure overlap between distinct handler trees.
    pub(crate) event_similarity: bool,
    pub(crate) indirect_usage: bool,
    pub(crate) dialect: &'static dyn RegexDialect,
    pub(crate) capture_context: fn(&StepDefinition) -> CaptureContext,
}

impl StepDefinition {
    // Routing is the only place that interprets framework identity for core analysis. Keeping
    // this derived preserves exhaustive public structs and legacy serialized definitions.
    pub(crate) fn analysis_profile(&self) -> AnalysisProfile {
        match self.framework {
            Framework::CucumberRuby => crate::ruby::semantics::profile(self),
            framework => crate::typescript::semantics::profile(framework),
        }
    }

    pub(crate) fn capture_context(&self) -> CaptureContext {
        (self.analysis_profile().capture_context)(self)
    }

    pub(crate) fn method_semantics(&self) -> Option<&str> {
        BehaviorEventRef::from_legacy(self.handler.behavior_signature.first()?).method_semantics()
    }
}

impl super::SourceLanguage {
    pub(crate) fn comparison_domain(self) -> ComparisonDomain {
        match self {
            Self::Ruby => ComparisonDomain::Ruby,
            Self::JavaScript | Self::TypeScript | Self::Tsx => ComparisonDomain::EcmaScript,
        }
    }
}
