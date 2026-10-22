//! Internal compatibility profiles for extracted and caller-constructed definitions.

use crate::model::{BehaviorEventRef, EffectStream, Framework, StepDefinition};

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

/// What pins a handler to one runtime context; two handlers with different contexts cannot run
/// together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeContext<'a> {
    /// The key of the handler's declared capture scope.
    Scope(&'a str),
    /// The payload of the handler's leading `method:` event.
    Method(&'a str),
    /// The modelled effect-stream regime of a handler that observed no events.
    Stream(EffectStream),
}

pub(crate) trait RegexDialect: Sync {
    /// Translates a regex matcher and its flags into a Rust `regex` expression with this dialect's
    /// semantics, or `None` when no faithful translation exists; usage analysis then treats the
    /// matcher as non-authoritative.
    fn expression(&self, matcher: &str, flags: &str) -> Option<String>;
    /// Flags that partition normalized-matcher classes, or `None` when flags never separate them.
    /// Ruby returns the raw flags; JavaScript returns `None`.
    fn normalized_flags<'a>(&self, flags: &'a str) -> Option<&'a str>;
    /// Whether two matchers' flags allow treating them as wording variants of one step. Ruby
    /// requires equal flag strings; JavaScript accepts any pair.
    fn similarity_flags_match(&self, left: &str, right: &str) -> bool;

    /// Projects flags onto those that count toward exact-duplicate identity; the default applies
    /// [`legacy_identity_flags`].
    fn identity_flags(&self, flags: &str) -> String {
        legacy_identity_flags(flags)
    }

    /// One text that parameter-type `pattern` matches, used as a usage witness, or `None`. The
    /// default takes the first `|` alternative after stripping enclosing groups, accepted only when
    /// non-empty and made of alphanumerics, space, `_` or `-`; a translated scoped array tries its
    /// first element first. A group opener without a trailing `)` yields `None`.
    fn parameter_sample(&self, pattern: &str) -> Option<String> {
        // Preserve the existing witness subset, including translated scoped patterns.
        // The resulting witness must still match both compiled definitions.
        if let Some(array) = pattern
            .strip_prefix("(?:(?m:")
            .or_else(|| pattern.strip_prefix("(?:(?ms:"))
            .and_then(|value| value.strip_suffix("))"))
        {
            let end = [")|(?m:", ")|(?ms:"]
                .into_iter()
                .filter_map(|delimiter| array.find(delimiter))
                .min()
                .unwrap_or(array.len());
            let first = &array[..end];
            if !first.is_empty()
                && first
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'))
            {
                return Some(first.to_owned());
            }
        }
        let mut trimmed = pattern.trim();
        loop {
            let inner = trimmed
                .strip_prefix("(?:")
                .or_else(|| trimmed.strip_prefix("(?m:"))
                .or_else(|| trimmed.strip_prefix("(?ms:"))
                .or_else(|| trimmed.strip_prefix("(?s:"))
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
/// Keeps the flags `i`, `m`, `s`, `u` and `v` present in `flags`, in that order; drops any other.
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
    // Identical event streams with different handlers differ only in values the events omit;
    // such pairs are parameterization material, never wording findings.
    pub(crate) near_requires_distinct_events: bool,
    pub(crate) indirect_usage: bool,
    pub(crate) dialect: &'static dyn RegexDialect,
}

impl StepDefinition {
    // Routing is the only place that interprets framework identity for core analysis. Keeping
    // this derived preserves exhaustive public structs and legacy serialized definitions.
    /// Routes each framework to its language-owned comparison policy.
    pub(crate) fn analysis_profile(&self) -> AnalysisProfile {
        match self.framework {
            Framework::CucumberRuby => crate::ruby::semantics::profile(self),
            framework => crate::typescript::semantics::profile(framework),
        }
    }

    /// `LexicalFile` when the handler declares a capture scope, else `Unrestricted`.
    pub(crate) fn capture_context(&self) -> CaptureContext {
        if self.handler.capture_scope.is_some() {
            CaptureContext::LexicalFile
        } else {
            CaptureContext::Unrestricted
        }
    }

    /// The context that pins the handler: its capture scope; else the payload of a leading
    /// `method:` event; else, for a handler without events, a modelled effect-stream regime. `None`
    /// when nothing pins it.
    pub(crate) fn runtime_context(&self) -> Option<RuntimeContext<'_>> {
        let handler = &self.handler;
        if let Some(key) = handler.capture_scope.as_deref() {
            return Some(RuntimeContext::Scope(key));
        }
        match handler.behavior_signature.first() {
            Some(event) => BehaviorEventRef::from_legacy(event)
                .method_semantics()
                .map(RuntimeContext::Method),
            None => (!handler.effect_stream.is_partial())
                .then_some(RuntimeContext::Stream(handler.effect_stream)),
        }
    }
}

impl super::SourceLanguage {
    /// Comparison domain of this language's definitions: Ruby alone, or ECMAScript shared by
    /// JavaScript, TypeScript and TSX.
    pub(crate) fn comparison_domain(self) -> ComparisonDomain {
        match self {
            Self::Ruby => ComparisonDomain::Ruby,
            Self::JavaScript | Self::TypeScript | Self::Tsx => ComparisonDomain::EcmaScript,
        }
    }
}
