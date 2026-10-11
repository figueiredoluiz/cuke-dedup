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
