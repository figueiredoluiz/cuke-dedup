use super::{Config, FeatureStep, MatcherIndex, Rule, Severity};
use std::collections::{BTreeMap, BTreeSet};

/// An index into a slice of non-zero-sized definitions is always smaller than this flag.
const MASK_TAG: usize = 1 << (usize::BITS - 1);

/// A span starts with its first matched definition. An optional tagged word stores the next
/// word-width minus one positions as bits. Isolated matches need only their index.
#[derive(Default)]
pub(super) enum MatchRow {
    #[default]
    Empty,
    One([usize; 2]),
    Many(Vec<usize>),
}

impl MatchRow {
    /// Appends a strictly increasing definition index, keeping the first span inline.
    pub(super) fn append(&mut self, index: usize) {
        match self {
            Self::Empty => *self = Self::One([index, MASK_TAG]),
            Self::One(span) => {
                if let Some(bit) = following_bit(span[0], index) {
                    span[1] |= bit;
                } else {
                    let mut words = vec![span[0]];
                    if span[1] != MASK_TAG {
                        words.push(span[1]);
                    }
                    words.push(index);
                    *self = Self::Many(words);
                }
            }
            Self::Many(words) => {
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
    }

    /// Visits the first index and set bits of each span in ascending definition order.
    pub(super) fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        let words: &[usize] = match self {
            Self::Empty => &[],
            Self::One(span) => span,
            Self::Many(words) => words,
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
}

/// Returns the bit for an index following the first match within the same span.
fn following_bit(first: usize, index: usize) -> Option<usize> {
    let distance = index - first;
    (distance < usize::BITS as usize).then(|| 1 << (distance - 1))
}

/// The enabled consumer determines whether direct matches need to survive the current window.
pub(super) enum FeatureMatches<'a> {
    Inactive,
    Usage(BTreeSet<&'a str>),
    Ambiguity(BTreeMap<&'a str, MatchRow>),
}

impl<'a> FeatureMatches<'a> {
    /// Deduplicates feature texts only for rules that consume direct matches.
    pub(super) fn new(steps: &'a [FeatureStep], config: &Config) -> Self {
        let texts = steps.iter().map(|step| step.text.as_str());
        if config.severity(Rule::AmbiguousStep) != Severity::Off {
            let mut rows = BTreeMap::new();
            for text in texts {
                rows.entry(text).or_default();
            }
            Self::Ambiguity(rows)
        } else if config.severity(Rule::UnusedDefinition) != Severity::Off {
            let mut distinct = BTreeSet::new();
            distinct.extend(texts);
            Self::Usage(distinct)
        } else {
            Self::Inactive
        }
    }

    /// Updates usage for one compiled window and retains complete rows only for ambiguity.
    pub(super) fn scan(
        &mut self,
        start: usize,
        index: &MatcherIndex,
        used: &mut [bool],
        local: &mut Vec<usize>,
    ) {
        match self {
            Self::Inactive => {}
            Self::Usage(texts) => {
                let mut remaining = used.iter().filter(|&&hit| !hit).count();
                for &text in texts.iter() {
                    if remaining == 0 {
                        break;
                    }
                    index.matches(text, local);
                    for &id in local.iter() {
                        if !used[id] {
                            used[id] = true;
                            remaining -= 1;
                        }
                    }
                }
            }
            Self::Ambiguity(rows) => {
                for (&text, row) in rows {
                    index.matches(text, local);
                    for &id in local.iter() {
                        used[id] = true;
                        row.append(start + id);
                    }
                }
            }
        }
    }

    /// Exposes the cache to overlap only when every observed text has a complete row.
    pub(super) fn rows(&self) -> Option<&BTreeMap<&'a str, MatchRow>> {
        match self {
            Self::Ambiguity(rows) => Some(rows),
            _ => None,
        }
    }
}
