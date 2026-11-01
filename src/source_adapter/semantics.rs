//! Comparison policies adapters attach to the definitions they extract.

use crate::matcher::{IdentityFlags, RegexKey, TranslatedRegex};
use crate::model::{BehaviorEventRef, EffectStream, MatcherKind, StepDefinition};

/// A set of definitions that may be compared with one another, with the regex dialect they share.
/// Definitions of different domains are never compared. Two specs are equal only when they are the
/// same object, so a reused label cannot merge domains: declare each domain once, in a `static`
/// (a `const` may be instantiated at several addresses).
pub struct DomainSpec {
    /// Stable label reported in `--emit-fingerprints` records.
    pub(crate) label: &'static str,
    /// Regex semantics of the domain's matchers.
    pub(crate) dialect: &'static dyn RegexDialect,
}

impl DomainSpec {
    /// A domain reported as `label` in `--emit-fingerprints` records, matching regexes with
    /// `dialect`.
    pub const fn new(label: &'static str, dialect: &'static dyn RegexDialect) -> Self {
        Self { label, dialect }
    }
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
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerScope {
    /// Registered in the common pool every other shared handler runs in.
    Shared,
    /// Bound to the scenario callback that declares it.
    Local,
    /// Registered in a global registry that no other scope shares.
    Isolated,
}

/// How a definition's matcher is selected for a feature step.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepMatching {
    /// Any feature step in the corpus can select the definition by its text.
    ByText,
    /// Only the step at the definition's own position selects it.
    Positional,
}

/// Where the evidence regime of a handler's event stream comes from.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEvidence {
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

/// The regex semantics of one comparison domain's matchers.
pub trait RegexDialect: Sync + std::panic::RefUnwindSafe {
    /// Translates a regex matcher and its flags into the Rust `regex` dialect with this dialect's
    /// semantics, or `None` when no faithful translation exists; usage analysis then treats the
    /// matcher as non-authoritative. Usage analysis matches feature steps with
    /// [`TranslatedRegex::expression`]; an expression that does not compile leaves the matcher's
    /// usage indeterminate with no diagnostic, so the adapter reports it at extraction, as the
    /// built-in adapters do. Pass the same translation to [`AnalysisPolicy::normalized_matcher`] so
    /// keying and matching agree.
    fn translate(&self, matcher: &str, flags: &str) -> Option<TranslatedRegex>;
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
    /// non-empty and made of alphanumerics, space, `_` or `-`; a group opener without a trailing
    /// `)` yields `None`.
    fn parameter_sample(&self, pattern: &str) -> Option<String> {
        alternative_sample(pattern)
    }
}

/// The first `|` alternative of `pattern` after stripping enclosing groups, accepted only when
/// non-empty and made of alphanumerics, space, `_` or `-`. A group opener without a trailing `)`
/// yields `None`. The witness must still match both compiled definitions.
pub(crate) fn alternative_sample(pattern: &str) -> Option<String> {
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

/// The built-in dialects' witness: the first element of a translated Ruby scoped array
/// (`(?:(?m:a)|(?m:b))`) when it qualifies, else [`alternative_sample`].
pub(crate) fn builtin_parameter_sample(pattern: &str) -> Option<String> {
    pattern
        .strip_prefix("(?:(?m:")
        .or_else(|| pattern.strip_prefix("(?:(?ms:"))
        .and_then(|value| value.strip_suffix("))"))
        .map(|array| {
            let end = [")|(?m:", ")|(?ms:"]
                .into_iter()
                .filter_map(|delimiter| array.find(delimiter))
                .min()
                .unwrap_or(array.len());
            &array[..end]
        })
        .filter(|first| {
            !first.is_empty()
                && first
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'))
        })
        .map(str::to_owned)
        .or_else(|| alternative_sample(pattern))
}

/// The dialect of a definition no adapter claims. Its regex semantics are unknown: nothing
/// translates, and matchers compare only as written.
struct UnattachedRegex;
static UNATTACHED_REGEX: UnattachedRegex = UnattachedRegex;

impl RegexDialect for UnattachedRegex {
    /// Declines: no translation is faithful to an unknown dialect.
    fn translate(&self, _: &str, _: &str) -> Option<TranslatedRegex> {
        None
    }
    /// Keys on the source pattern as written, without canonicalization.
    fn regex_key(&self) -> RegexKey {
        RegexKey::TranslatedBody
    }
    /// Every flag counts: which ones are inert depends on the dialect.
    fn identity_flags(&self) -> IdentityFlags {
        IdentityFlags::All
    }
    /// Requires identical flag strings, for the same reason.
    fn similarity_flags_match(&self, left: &str, right: &str) -> bool {
        left == right
    }
}

static UNATTACHED: DomainSpec = DomainSpec::new("unattached", &UNATTACHED_REGEX);

/// How the analyzer compares one definition with others, declared by the adapter that extracted
/// it. A [`StepDefinition`] built by hand names one: [`AnalysisPolicy::unattached`], an adapter's,
/// or one built with [`AnalysisPolicy::new`].
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
    /// Builds a policy from an adapter's declared capabilities. `step_invocation` declares that
    /// handlers may invoke other steps by text, so usage needs the invocation evidence.
    pub const fn new(
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

    /// The policy of a definition no adapter claims. Its domain is its own, so analysis refuses
    /// to mix it with an adapter's definitions, and registration refuses an adapter that declares
    /// it. No regex translates, so usage treats regex matchers as non-authoritative. Handlers may
    /// invoke steps by text as far as anyone knows, so without indirect usage evidence unused
    /// findings are disabled and the result is incomplete. Handlers keep the evidence regime they
    /// declare, in the shared scope, matched by text.
    pub const fn unattached() -> Self {
        Self::new(
            &UNATTACHED,
            HandlerScope::Shared,
            StepMatching::ByText,
            StreamEvidence::Declared,
            true,
        )
    }

    /// Whether this is [`AnalysisPolicy::unattached`]'s domain.
    pub(crate) fn is_unattached(self) -> bool {
        self.domain == &UNATTACHED
    }

    /// The comparison domain this policy declares.
    pub(crate) fn domain(self) -> &'static DomainSpec {
        self.domain
    }

    /// The comparison text of a matcher, keyed as this policy's regex dialect declares; the value
    /// an adapter stores in [`StepDefinition::normalized_matcher`].
    pub fn normalized_matcher(
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
