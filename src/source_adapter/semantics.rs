//! Internal compatibility profiles for extracted and caller-constructed definitions.

use crate::matcher::{IdentityFlags, RegexKey, TranslatedRegex};
use crate::model::{BehaviorEventRef, EffectStream, MatcherKind, StepDefinition};

/// A set of definitions that may be compared with one another, with the regex dialect they share.
/// Definitions of different domains are never compared; an adapter owns one static spec, and two
/// specs are equal only when they are the same static, so a reused label cannot merge domains.
pub(crate) struct DomainSpec {
    /// Stable label reported in `--emit-fingerprints` records.
    pub(crate) label: &'static str,
    /// Regex semantics of the domain's matchers.
    pub(crate) dialect: &'static dyn RegexDialect,
}

impl PartialEq for DomainSpec {
    /// Domains are equal when they are the same static spec.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for DomainSpec {}

impl std::fmt::Debug for DomainSpec {
    /// Prints the domain label.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DomainSpec({})", self.label)
    }
}

/// Which handlers can run together: handlers of different scopes never share a runtime context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandlerScope {
    /// Registered in the common pool every other shared handler runs in.
    Shared,
    /// Bound to the scenario callback that declares it.
    Local,
    /// Registered in a global registry that no other scope shares.
    Isolated,
}

/// How a definition's matcher is selected for a feature step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepMatching {
    /// Any feature step in the corpus can select the definition by its text.
    ByText,
    /// Only the step at the definition's own position selects it.
    Positional,
}

/// Where the evidence regime of a handler's event stream comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamEvidence {
    /// Read the regime from the handler's declared [`EffectStream`].
    Declared,
    /// Treat every handler's events as a complete stream, whatever it declares.
    Complete,
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
    /// How this dialect's regex matchers are keyed for normalized-matcher comparison.
    fn regex_key(&self) -> RegexKey;
    /// Which flags count toward a matcher's identity, for exact-duplicate classes and canonical
    /// keys alike.
    fn identity_flags(&self) -> IdentityFlags;
    /// Whether two matchers' flags allow treating them as wording variants of one step. Ruby
    /// requires equal flag strings; JavaScript accepts any pair.
    fn similarity_flags_match(&self, left: &str, right: &str) -> bool;

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

/// How the analyzer compares one definition with others, declared by the adapter that extracted
/// it. The fields are opaque: only an adapter chooses them, and an embedder that builds a
/// [`StepDefinition`] by hand gets [`AnalysisPolicy::default`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisPolicy {
    domain: &'static DomainSpec,
    handler_scope: HandlerScope,
    step_matching: StepMatching,
    stream_evidence: StreamEvidence,
    step_invocation: bool,
}

impl AnalysisPolicy {
    /// Builds a policy from an adapter's declared capabilities.
    pub(crate) const fn new(
        domain: &'static DomainSpec,
        handler_scope: HandlerScope,
        step_matching: StepMatching,
        stream_evidence: StreamEvidence,
        step_invocation: bool,
    ) -> Self {
        Self {
            domain,
            handler_scope,
            step_matching,
            stream_evidence,
            step_invocation,
        }
    }

    /// The comparison domain this policy declares.
    pub(crate) fn domain(self) -> &'static DomainSpec {
        self.domain
    }

    /// The comparison text of a matcher, keyed as this policy's regex dialect declares.
    pub(crate) fn normalized_matcher(
        self,
        kind: MatcherKind,
        matcher: &str,
        flags: &str,
        translated: Option<&TranslatedRegex>,
    ) -> String {
        crate::matcher::normalized_matcher(
            kind,
            matcher,
            flags,
            translated,
            self.domain.dialect.regex_key(),
            self.domain.dialect.identity_flags(),
        )
    }

    /// Combines the declared capabilities with the handler's own stream regime.
    pub(crate) fn profile(self, stream: EffectStream) -> AnalysisProfile {
        let stream = match self.stream_evidence {
            StreamEvidence::Declared => stream,
            StreamEvidence::Complete => EffectStream::Complete,
        };
        let (requires_identical_handler, scores_event_overlap, excludes_identical_event_streams) =
            match stream {
                EffectStream::Partial => (true, false, false),
                EffectStream::Complete => (false, true, false),
                EffectStream::ActionOnly => (false, true, true),
            };
        AnalysisProfile {
            domain: self.domain,
            handler_scope: self.handler_scope,
            step_matching: self.step_matching,
            step_invocation: self.step_invocation,
            requires_identical_handler,
            scores_event_overlap,
            excludes_identical_event_streams,
        }
    }
}

impl Default for AnalysisPolicy {
    /// The policy of a definition nobody attached one to: the shared-scope, text-matched,
    /// complete-stream policy of the JavaScript/TypeScript adapter.
    fn default() -> Self {
        crate::typescript::semantics::default_policy()
    }
}

/// An [`AnalysisPolicy`] resolved against one handler's effect stream.
#[derive(Clone, Copy)]
pub(crate) struct AnalysisProfile {
    /// Definitions compare only within one domain.
    pub(crate) domain: &'static DomainSpec,
    /// Handlers of different scopes never run together.
    pub(crate) handler_scope: HandlerScope,
    /// How feature steps select the definition.
    pub(crate) step_matching: StepMatching,
    /// Handlers may invoke other steps by text, so usage needs the invocation evidence.
    pub(crate) step_invocation: bool,
    /// Wording findings need identical handlers, even for structural matches.
    pub(crate) requires_identical_handler: bool,
    /// Event overlap between distinct handler trees can be measured; partial streams cannot.
    pub(crate) scores_event_overlap: bool,
    /// Identical event streams behind different handlers differ only in values the events omit;
    /// such pairs are parameterization material, never wording findings.
    pub(crate) excludes_identical_event_streams: bool,
}

impl AnalysisProfile {
    /// The regex dialect of the definition's domain.
    pub(crate) fn dialect(&self) -> &'static dyn RegexDialect {
        self.domain.dialect
    }

    /// Whether any feature step in the corpus can select the definition by its text.
    pub(crate) fn matches_by_text(&self) -> bool {
        self.step_matching == StepMatching::ByText
    }
}

impl StepDefinition {
    /// The policy the adapter attached, resolved against this handler's effect stream.
    pub(crate) fn analysis_profile(&self) -> AnalysisProfile {
        self.analysis_policy.profile(self.handler.effect_stream)
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
