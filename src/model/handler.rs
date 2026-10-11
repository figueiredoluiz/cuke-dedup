//! Shared completion of frontend representations and ordered semantic evidence.

use super::{BehaviorEvent, HandlerFingerprint};

pub(crate) struct HandlerSemantics {
    pub(crate) exact: String,
    pub(crate) normalized: String,
    pub(crate) alpha: String,
    pub(crate) structural: String,
    pub(crate) events: Vec<BehaviorEvent>,
    pub(crate) source_snippet: String,
    pub(crate) comparable: bool,
    pub(crate) trivial: bool,
}

impl HandlerSemantics {
    // Frontends retain syntax/value proofs and legacy identity encoding. The shared boundary
    // owns uncertainty removal and the lossless event protocol; it cannot manufacture provenance.
    pub(crate) fn finish(self, encode: fn(&str) -> String) -> HandlerFingerprint {
        HandlerFingerprint {
            exact: encode(&self.exact),
            normalized: encode(&self.normalized),
            alpha_normalized: encode(&self.alpha),
            structural: encode(&self.structural),
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
        }
    }
}

/// Normalizes report snippets and truncates them at a Unicode-safe character boundary. Frontends
/// whose work budgets predate the ellipsis can preserve the truncated prefix's UTF-8 byte charge.
pub(crate) fn bounded_source_snippet(
    source: &str,
    max_chars: usize,
    preserve_prefix_bytes: bool,
) -> String {
    let normalized = source.trim().replace("\r\n", "\n").replace('\r', "\n");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut snippet: String = normalized.chars().take(max_chars).collect();
    let previous_bytes = snippet.len();
    if preserve_prefix_bytes {
        while snippet.len() + '…'.len_utf8() > previous_bytes {
            snippet.pop();
        }
    } else {
        snippet.pop();
    }
    snippet.push('…');
    snippet
}
