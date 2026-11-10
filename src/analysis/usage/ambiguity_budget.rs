use crate::resource_limits::{
    MAX_AMBIGUITY_GROUPS, MAX_AMBIGUITY_MEMBERSHIPS, MAX_AMBIGUITY_PAYLOAD_BYTES,
};

/// Cumulative admission for owned ambiguity indices, complete findings and native text/path bytes.
pub(super) struct AmbiguityBudget {
    limits: [usize; 3],
    remaining: [usize; 3],
}

impl Default for AmbiguityBudget {
    /// Creates a fresh per-analysis allowance, independent of matcher and reporter budgets.
    fn default() -> Self {
        Self::new(
            MAX_AMBIGUITY_MEMBERSHIPS,
            MAX_AMBIGUITY_GROUPS,
            MAX_AMBIGUITY_PAYLOAD_BYTES,
        )
    }
}

impl AmbiguityBudget {
    /// Creates explicit allowances so boundary tests do not change process-wide settings.
    pub(super) fn new(memberships: usize, groups: usize, bytes: usize) -> Self {
        let limits = [memberships, groups, bytes];
        Self {
            limits,
            remaining: limits,
        }
    }

    /// Rejects even temporary member collection when no group of this size could ever fit.
    pub(super) fn can_hold_group(&self, members: usize) -> bool {
        members <= self.limits[0] / 2
    }

    /// Reserves a key and its future related locations, plus each new proof key/membership copy.
    pub(super) fn admit_group(&mut self, members: usize, new_proof_members: usize) -> bool {
        let Some(slots) = members
            .checked_add(new_proof_members)
            .and_then(|n| n.checked_mul(2))
        else {
            return false;
        };
        if slots > self.remaining[0] || self.remaining[1] == 0 {
            return false;
        }
        self.remaining[0] -= slots;
        self.remaining[1] -= 1;
        true
    }

    /// Counts every retained text/path field before locations are cloned; overflow refuses atomically.
    pub(super) fn admit_payload(&mut self, sizes: impl IntoIterator<Item = usize>) -> bool {
        let Some(bytes) = sizes.into_iter().try_fold(0usize, usize::checked_add) else {
            return false;
        };
        if bytes > self.remaining[2] {
            return false;
        }
        self.remaining[2] -= bytes;
        true
    }

    /// Describes the cut without implying that omitted ambiguity or overlap was analyzed completely.
    pub(super) fn diagnostic(&self) -> String {
        format!("ambiguity retention exceeded its safety limits ({} owned memberships, {} groups/findings or {} payload bytes); ambiguity findings are partial and static matcher overlap was skipped", self.limits[0], self.limits[1], self.limits[2])
    }
}

#[cfg(test)]
mod tests;
