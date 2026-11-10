use crate::resource_limits::{MAX_DIRECT_MATCHES, MAX_DIRECT_MATCH_SCANS};

/// Independent per-analysis allowances for automaton scans and emitted matcher identifiers.
#[derive(Clone, Copy)]
pub(super) struct MatchingBudget {
    limits: [usize; 2],
    remaining: [usize; 2],
}

impl Default for MatchingBudget {
    /// Creates the measured production allowances for one direct feature-matching analysis.
    fn default() -> Self {
        Self::new(MAX_DIRECT_MATCH_SCANS, MAX_DIRECT_MATCHES)
    }
}

impl MatchingBudget {
    /// Creates explicit allowances so final-outcome tests can exercise exact boundaries.
    pub(super) const fn new(scans: usize, matches: usize) -> Self {
        Self {
            limits: [scans, matches],
            remaining: [scans, matches],
        }
    }

    /// Admits automaton invocations before they run.
    pub(super) fn admit_scans(&mut self, scans: usize) -> bool {
        Self::admit(&mut self.remaining[0], scans)
    }

    /// Admits matcher identifiers before they enter usage or ambiguity state.
    pub(super) fn admit_matches(&mut self, matches: usize) -> bool {
        Self::admit(&mut self.remaining[1], matches)
    }

    /// Atomically deducts fitting work without changing the allowance after refusal.
    fn admit(remaining: &mut usize, amount: usize) -> bool {
        if amount > *remaining {
            return false;
        }
        *remaining -= amount;
        true
    }

    /// Describes a conservative cutoff without claiming omitted direct findings are absent.
    pub(super) fn diagnostic(&self) -> String {
        format!(
            "direct feature matching exceeded its safety limits ({} automaton scans or {} emitted matches); ambiguity findings are partial, unused-definition findings were disabled and static matcher overlap was skipped",
            self.limits[0], self.limits[1]
        )
    }
}
