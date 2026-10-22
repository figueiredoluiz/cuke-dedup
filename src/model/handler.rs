//! Shared completion of frontend representations and ordered semantic evidence.

use super::{BehaviorEvent, HandlerFingerprint};
use serde::{Deserialize, Serialize};

/// Evidence regime of a handler's behaviour events; selects which event-overlap comparisons the
/// events can support. Non-exhaustive so that a later regime is not another breaking change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum EffectStream {
    /// Effects are not fully modelled: only identity comparisons apply, never event overlap.
    #[default]
    Partial,
    /// Every observable effect, assertions included, is in the events.
    Complete,
    /// Action calls are in the events but assertions are not.
    ActionOnly,
}

impl EffectStream {
    /// The regime's serialized name.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Partial => "partial",
            Self::Complete => "complete",
            Self::ActionOnly => "action-only",
        }
    }

    /// Whether this is the default regime; serialization omits it.
    pub(crate) fn is_partial(&self) -> bool {
        *self == Self::Partial
    }
}

/// What makes two handlers' behaviour equal: the ordered events, the effect-stream regime and the
/// capture scope. Events alone would equate handlers that the events cannot tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BehaviorIdentity<'a> {
    events: &'a [String],
    stream: EffectStream,
    scope: Option<&'a str>,
}

impl HandlerFingerprint {
    /// The key under which two handlers count as behaving identically.
    pub(crate) fn behavior_identity(&self) -> BehaviorIdentity<'_> {
        BehaviorIdentity {
            events: &self.behavior_signature,
            stream: self.effect_stream,
            scope: self.capture_scope.as_deref(),
        }
    }

    /// Stable digest of the behaviour identity. A handler with a partial stream and no capture
    /// scope digests as its events alone; any other handler digests its events together with the
    /// regime and scope under a distinct frame, so handlers that differ in either digest
    /// differently.
    pub(crate) fn behavior_digest(&self) -> String {
        let events =
            super::stable_fingerprint_parts(self.behavior_signature.iter().map(String::as_str));
        if self.effect_stream.is_partial() && self.capture_scope.is_none() {
            return events;
        }
        let scope = self.capture_scope.as_deref();
        super::stable_fingerprint_parts([
            "behavior-identity",
            &events,
            self.effect_stream.name(),
            // An absent scope and the bare scope (empty key) are different identities.
            if scope.is_some() { "scope" } else { "no-scope" },
            scope.unwrap_or_default(),
        ])
    }
}

/// How `HandlerSemantics::finish` turns the textual representations into comparison identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepresentationEncoding {
    /// Every representation becomes its stable digest; comparison work no longer depends on text
    /// length.
    Digest,
    /// Normalized, alpha and structural text stay as supplied so similarity can read it. The exact
    /// identity is the digest of the exact text repeated to `exact_len` characters, which keeps
    /// the comparison-work charge the frontend budgeted for.
    Verbatim { exact_len: usize },
}

impl RepresentationEncoding {
    /// Encodes the exact representation.
    fn exact(self, exact: &str) -> String {
        let digest = super::stable_fingerprint(exact);
        match self {
            Self::Digest => digest,
            Self::Verbatim { exact_len } => digest.chars().cycle().take(exact_len).collect(),
        }
    }

    /// Encodes a non-exact representation.
    fn other(self, text: String) -> String {
        match self {
            Self::Digest => super::stable_fingerprint(&text),
            Self::Verbatim { .. } => text,
        }
    }
}

pub(crate) struct HandlerSemantics {
    pub(crate) exact: String,
    pub(crate) normalized: String,
    pub(crate) alpha: String,
    /// `None` reuses the encoded exact identity: the frontend found no structure beyond spelling.
    pub(crate) structural: Option<String>,
    pub(crate) events: Vec<BehaviorEvent>,
    pub(crate) source_snippet: String,
    pub(crate) comparable: bool,
    pub(crate) trivial: bool,
    pub(crate) encoding: RepresentationEncoding,
    pub(crate) stream: EffectStream,
    /// Key of the file-bound capture scope, or `None` when captures are unrestricted.
    pub(crate) capture_scope: Option<String>,
}

impl HandlerSemantics {
    // Frontends retain syntax/value proofs. The shared boundary owns representation encoding,
    // uncertainty removal and the lossless event protocol; it cannot manufacture provenance.
    /// Encodes the four representations per `encoding` and the events as legacy signatures; the
    /// result is comparable only if the input is and no event is an unresolved assertion.
    pub(crate) fn finish(self) -> HandlerFingerprint {
        let encoding = self.encoding;
        let exact = encoding.exact(&self.exact);
        HandlerFingerprint {
            structural: self
                .structural
                .map_or_else(|| exact.clone(), |text| encoding.other(text)),
            exact,
            normalized: encoding.other(self.normalized),
            alpha_normalized: encoding.other(self.alpha),
            comparable: self.comparable
                && !self
                    .events
                    .iter()
                    .any(BehaviorEvent::is_unresolved_assertion),
            trivial: self.trivial,
            behavior_signature: self
                .events
                .into_iter()
                .map(BehaviorEvent::into_legacy)
                .collect(),
            source_snippet: self.source_snippet,
            effect_stream: self.stream,
            capture_scope: self.capture_scope,
        }
    }
}

/// Normalizes report snippets and truncates them at a Unicode-safe character boundary. Frontends
/// whose work budgets predate normalization can constrain the result to the raw prefix's UTF-8
/// byte charge.
pub(crate) fn bounded_source_snippet(
    source: &str,
    max_chars: usize,
    preserve_prefix_bytes: bool,
) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let normalized = source.trim().replace("\r\n", "\n").replace('\r', "\n");
    if preserve_prefix_bytes {
        let prefix_bytes = source
            .chars()
            .take(max_chars)
            .map(char::len_utf8)
            .sum::<usize>();
        if normalized.chars().count() <= max_chars && normalized.len() <= prefix_bytes {
            return normalized;
        }
        let mut snippet: String = normalized.chars().take(max_chars).collect();
        while snippet.len() + '…'.len_utf8() > prefix_bytes {
            if snippet.pop().is_none() {
                return String::new();
            }
        }
        snippet.push('…');
        return snippet;
    }
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut snippet: String = normalized.chars().take(max_chars).collect();
    snippet.pop();
    snippet.push('…');
    snippet
}
