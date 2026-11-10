use super::{Config, FeatureStep, MatcherIndex, MatchingBudget, PreparedMatcher, Rule, Severity};
use crate::resource_limits::{MAX_AMBIGUITY_MEMBERSHIPS, MAX_DIRECT_MATCH_CACHE_BYTES};
use std::collections::BTreeSet;

/// An index into a slice of non-zero-sized definitions is always smaller than this flag.
const MASK_TAG: usize = 1 << (usize::BITS - 1);
/// One ambiguity group needs both its finding members and its proof memberships.
pub(super) const MAX_ROW_MATCHES: usize = MAX_AMBIGUITY_MEMBERSHIPS / 2;

#[derive(Default)]
enum MatchStorage {
    #[default]
    Empty,
    One([usize; 2]),
    Many(Vec<usize>),
}

/// Complete authoritative matches for one distinct feature text.
#[derive(Default)]
pub(super) struct MatchRow {
    storage: MatchStorage,
    matches: usize,
    overflowed: bool,
}

impl MatchRow {
    /// Appends a strictly increasing definition index, keeping the first span inline.
    pub(super) fn append(&mut self, index: usize) -> usize {
        let old_capacity = self.backing_words();
        match &mut self.storage {
            MatchStorage::Empty => self.storage = MatchStorage::One([index, MASK_TAG]),
            MatchStorage::One(span) => {
                if let Some(bit) = following_bit(span[0], index) {
                    span[1] |= bit;
                } else {
                    let mut words = Vec::with_capacity(3);
                    words.push(span[0]);
                    if span[1] != MASK_TAG {
                        words.push(span[1]);
                    }
                    words.push(index);
                    self.storage = MatchStorage::Many(words);
                }
            }
            MatchStorage::Many(words) => {
                let last = words.len() - 1;
                let has_mask = words[last] & MASK_TAG != 0;
                let first = words[last - usize::from(has_mask)];
                match following_bit(first, index) {
                    Some(bit) if has_mask => words[last] |= bit,
                    Some(bit) => words.push(MASK_TAG | bit),
                    None => words.push(index),
                }
            }
        }
        self.matches += 1;
        self.backing_words()
            .saturating_sub(old_capacity)
            .saturating_mul(std::mem::size_of::<usize>())
    }

    /// Retains a match only while one complete group could still pass ambiguity admission.
    fn append_bounded(&mut self, index: usize, row_limit: usize) -> RowAppend {
        if self.overflowed {
            return RowAppend::Complete;
        }
        if self.matches == row_limit {
            self.overflowed = true;
            return RowAppend::RowOverflow;
        }
        RowAppend::CacheGrowth(self.append(index))
    }

    /// Visits the first index and set bits of each span in ascending definition order.
    pub(super) fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        let words: &[usize] = match &self.storage {
            MatchStorage::Empty => &[],
            MatchStorage::One(span) => span,
            MatchStorage::Many(words) => words,
        };
        let mut words = words.iter().copied().peekable();
        std::iter::from_fn(move || {
            let first = words.next()?;
            let mut mask = words.next_if(|word| word & MASK_TAG != 0).unwrap_or(0) & !MASK_TAG;
            Some(std::iter::once(first).chain(std::iter::from_fn(move || {
                if mask == 0 {
                    return None;
                }
                let index = first + 1 + mask.trailing_zeros() as usize;
                mask &= mask - 1;
                Some(index)
            })))
        })
        .flatten()
    }

    /// Returns the number of authoritative matches retained in this complete row.
    pub(super) const fn len(&self) -> usize {
        self.matches
    }

    /// Reports that the row exceeded the largest group ambiguity admission can retain.
    pub(super) const fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Returns heap-allocated words; the first span remains inline and contributes zero.
    fn backing_words(&self) -> usize {
        match &self.storage {
            MatchStorage::Many(words) => words.capacity(),
            _ => 0,
        }
    }

    #[cfg(test)]
    /// Reports that the row still uses only its inline first span.
    pub(super) fn is_inline(&self) -> bool {
        matches!(self.storage, MatchStorage::One(_))
    }

    #[cfg(test)]
    /// Returns test-visible heap capacity in bytes.
    pub(super) fn backing_bytes(&self) -> usize {
        self.backing_words() * std::mem::size_of::<usize>()
    }
}

enum RowAppend {
    Complete,
    RowOverflow,
    CacheGrowth(usize),
}

/// Returns the bit for an index following the first match within the same span.
fn following_bit(first: usize, index: usize) -> Option<usize> {
    let distance = index - first;
    (distance < usize::BITS as usize).then(|| 1 << (distance - 1))
}

/// Sorted rows and the owned allocation bytes they retain.
pub(super) struct AmbiguityRows<'a> {
    rows: Vec<(&'a str, MatchRow)>,
    retained_bytes: usize,
    cache_limit: usize,
    row_limit: usize,
}

impl<'a> AmbiguityRows<'a> {
    /// Allocates sorted empty rows and accounts their exact vector capacity.
    pub(super) fn new(
        texts: impl IntoIterator<Item = &'a str>,
        cache_limit: usize,
        row_limit: usize,
    ) -> Self {
        let mut texts: Vec<_> = texts.into_iter().collect();
        texts.sort_unstable();
        texts.dedup();
        let mut rows = Vec::with_capacity(texts.len());
        rows.extend(texts.into_iter().map(|text| (text, MatchRow::default())));
        let retained_bytes = rows
            .capacity()
            .saturating_mul(std::mem::size_of::<(&str, MatchRow)>());
        Self {
            rows,
            retained_bytes,
            cache_limit,
            row_limit,
        }
    }

    #[cfg(test)]
    /// Appends by text so cache and overlap unit tests can build complete rows.
    pub(super) fn append(&mut self, text: &str, index: usize) -> ScanStatus {
        let offset = self
            .rows
            .binary_search_by_key(&text, |(text, _)| *text)
            .expect("scanned text belongs to the current batch");
        self.append_at(offset, index)
    }

    /// Appends at a known sorted offset and checks retained capacity after any growth.
    fn append_at(&mut self, offset: usize, index: usize) -> ScanStatus {
        let row = &mut self.rows[offset].1;
        match row.append_bounded(index, self.row_limit) {
            RowAppend::Complete | RowAppend::RowOverflow => ScanStatus::Complete,
            RowAppend::CacheGrowth(bytes) => {
                self.retained_bytes = self.retained_bytes.saturating_add(bytes);
                if self.retained_bytes > self.cache_limit {
                    ScanStatus::CacheOverflow
                } else {
                    ScanStatus::Complete
                }
            }
        }
    }

    /// Looks up one complete row by its feature text.
    pub(super) fn get(&self, text: &str) -> Option<&MatchRow> {
        self.rows
            .binary_search_by_key(&text, |(text, _)| *text)
            .ok()
            .map(|offset| &self.rows[offset].1)
    }

    /// Reports that retained row capacity is already above the current batch limit.
    pub(super) fn cache_overflowed(&self) -> bool {
        self.retained_bytes > self.cache_limit
    }
}

/// The enabled consumer determines whether direct matches need to survive the current window.
pub(super) enum FeatureMatches<'a> {
    Inactive,
    Usage(BTreeSet<&'a str>),
    Ambiguity(AmbiguityRows<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScanStatus {
    Complete,
    CacheOverflow,
    CpuExhausted,
}

/// Charges and performs one text scan, then admits every emitted matcher identifier atomically.
fn match_text(
    index: &MatcherIndex,
    text: &str,
    local: &mut Vec<usize>,
    budget: &mut MatchingBudget,
) -> bool {
    if !budget.admit_scans(index.scan_work()) {
        return false;
    }
    index.matches(text, local);
    budget.admit_matches(local.len())
}

impl<'a> FeatureMatches<'a> {
    /// Deduplicates all feature texts only for rules that consume direct matches.
    pub(super) fn new(steps: &'a [FeatureStep], config: &Config) -> Self {
        Self::for_texts(
            steps.iter().map(|step| step.text.as_str()),
            config,
            MAX_DIRECT_MATCH_CACHE_BYTES,
            MAX_ROW_MATCHES,
        )
    }

    /// Builds a testable or recomputed text batch with explicit retained-cache capacity.
    pub(super) fn for_texts(
        texts: impl IntoIterator<Item = &'a str>,
        config: &Config,
        cache_limit: usize,
        row_limit: usize,
    ) -> Self {
        if config.severity(Rule::AmbiguousStep) != Severity::Off {
            Self::Ambiguity(AmbiguityRows::new(texts, cache_limit, row_limit))
        } else if config.severity(Rule::UnusedDefinition) != Severity::Off {
            let mut distinct = BTreeSet::new();
            distinct.extend(texts);
            Self::Usage(distinct)
        } else {
            Self::Inactive
        }
    }

    /// Updates usage and complete ambiguity rows for one compiled window.
    pub(super) fn scan(
        &mut self,
        start: usize,
        matchers: &[PreparedMatcher],
        index: &MatcherIndex,
        used: &mut [bool],
        local: &mut Vec<usize>,
        budget: &mut MatchingBudget,
    ) -> ScanStatus {
        match self {
            Self::Inactive => ScanStatus::Complete,
            Self::Usage(texts) => {
                let mut remaining = used.iter().filter(|&&hit| !hit).count();
                for &text in texts.iter() {
                    if remaining == 0 {
                        break;
                    }
                    if !match_text(index, text, local, budget) {
                        return ScanStatus::CpuExhausted;
                    }
                    for &id in local.iter() {
                        if !used[id] {
                            used[id] = true;
                            remaining -= 1;
                        }
                    }
                }
                ScanStatus::Complete
            }
            Self::Ambiguity(rows) => {
                if rows.cache_overflowed() {
                    return ScanStatus::CacheOverflow;
                }
                for offset in 0..rows.rows.len() {
                    let text = rows.rows[offset].0;
                    if !match_text(index, text, local, budget) {
                        return ScanStatus::CpuExhausted;
                    }
                    for &id in local.iter() {
                        used[id] = true;
                        if matchers[id].authoritative
                            && rows.append_at(offset, start + id) == ScanStatus::CacheOverflow
                        {
                            return ScanStatus::CacheOverflow;
                        }
                    }
                }
                ScanStatus::Complete
            }
        }
    }

    /// Exposes complete ambiguity rows.
    pub(super) fn rows(&self) -> Option<&AmbiguityRows<'a>> {
        match self {
            Self::Ambiguity(rows) => Some(rows),
            _ => None,
        }
    }

    /// Reports that an ambiguity batch cannot begin within its retained-cache limit.
    pub(super) fn cache_overflowed(&self) -> bool {
        matches!(self, Self::Ambiguity(rows) if rows.cache_overflowed())
    }
}
