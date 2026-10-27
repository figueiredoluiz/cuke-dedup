//! Bit-parallel replacements for `strsim::levenshtein` and `strsim::jaro_winkler` on the matcher
//! comparison path. ASCII inputs that fit one 128-bit word take the bit-parallel path; every other
//! input defers to strsim, so both paths return strsim's value bit for bit.

// One `u128` holds the pattern bits (Levenshtein) or the matched positions (Jaro).
const MAX_BIT_PARALLEL_LEN: usize = 128;

/// The larger of Levenshtein similarity (normalized by the longer char count) and Jaro-Winkler;
/// two empty texts score 1.0.
pub(super) fn matcher_text_similarity(left: &str, right: &str) -> f64 {
    combined_score(left, right, levenshtein, jaro_winkler)
}

/// The combined matcher score over the given edit-distance and Jaro-Winkler functions, so the tests
/// can apply the same formula to strsim's metrics as the reference.
fn combined_score(
    left: &str,
    right: &str,
    distance: fn(&str, &str) -> usize,
    jaro_winkler: fn(&str, &str) -> f64,
) -> f64 {
    let max_length = left.chars().count().max(right.chars().count());
    if max_length == 0 {
        return 1.0;
    }
    let edit_similarity = 1.0 - distance(left, right) as f64 / max_length as f64;
    edit_similarity.max(jaro_winkler(left, right))
}

/// Char-based edit distance, equal to `strsim::levenshtein`.
fn levenshtein(left: &str, right: &str) -> usize {
    bit_parallel_levenshtein(left, right).unwrap_or_else(|| strsim::levenshtein(left, right))
}

/// Edit distance for ASCII inputs whose shorter side fits one word once the common prefix and
/// suffix are trimmed; `None` for any other input.
fn bit_parallel_levenshtein(left: &str, right: &str) -> Option<usize> {
    if !(left.is_ascii() && right.is_ascii()) {
        return None;
    }
    let (left, right) = trim_common_affixes(left.as_bytes(), right.as_bytes());
    let (pattern, text) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    (pattern.len() <= MAX_BIT_PARALLEL_LEN).then(|| {
        if pattern.is_empty() {
            text.len()
        } else {
            myers_distance(pattern, text)
        }
    })
}

/// Both slices without their longest common prefix and then their longest common suffix; edit
/// distance is invariant under that removal.
fn trim_common_affixes<'a>(left: &'a [u8], right: &'a [u8]) -> (&'a [u8], &'a [u8]) {
    let prefix = left
        .iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count();
    let (left, right) = (&left[prefix..], &right[prefix..]);
    let suffix = left
        .iter()
        .rev()
        .zip(right.iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    (&left[..left.len() - suffix], &right[..right.len() - suffix])
}

/// Global edit distance by Hyyrö's form of Myers' bit-vector algorithm; `pattern` holds 1 to 128
/// ASCII bytes and `text` only ASCII bytes.
fn myers_distance(pattern: &[u8], text: &[u8]) -> usize {
    let mut pattern_positions = [0_u128; 128];
    for (index, &byte) in pattern.iter().enumerate() {
        pattern_positions[usize::from(byte)] |= 1 << index;
    }
    let last_row = 1_u128 << (pattern.len() - 1);
    // Bits above the pattern never reach the bits below it (carries and shifts only move up), so
    // the vertical deltas need no mask.
    let mut vertical_positive = u128::MAX;
    let mut vertical_negative = 0_u128;
    let mut distance = pattern.len();
    for &byte in text {
        let equal = pattern_positions[usize::from(byte)];
        let vertical_change = equal | vertical_negative;
        let horizontal_change = ((equal & vertical_positive).wrapping_add(vertical_positive)
            ^ vertical_positive)
            | equal;
        let horizontal_positive = vertical_negative | !(horizontal_change | vertical_positive);
        let horizontal_negative = vertical_positive & horizontal_change;
        distance = distance + usize::from(horizontal_positive & last_row != 0)
            - usize::from(horizontal_negative & last_row != 0);
        // Row 0 of the global distance matrix grows by one per text byte: shift in a +1.
        let horizontal_positive = (horizontal_positive << 1) | 1;
        let horizontal_negative = horizontal_negative << 1;
        vertical_positive = horizontal_negative | !(vertical_change | horizontal_positive);
        vertical_negative = horizontal_positive & vertical_change;
    }
    distance
}

/// Jaro-Winkler similarity, equal to `strsim::jaro_winkler`.
fn jaro_winkler(left: &str, right: &str) -> f64 {
    bit_parallel_jaro_winkler(left, right).unwrap_or_else(|| strsim::jaro_winkler(left, right))
}

/// strsim 0.11's Jaro-Winkler for ASCII inputs of at most 128 bytes each, `None` otherwise: the
/// Jaro score boosted by 0.1 per shared leading char (at most 4) only when it exceeds 0.7, in
/// strsim's float expression order.
fn bit_parallel_jaro_winkler(left: &str, right: &str) -> Option<f64> {
    let fits = |text: &str| text.is_ascii() && text.len() <= MAX_BIT_PARALLEL_LEN;
    (fits(left) && fits(right)).then(|| {
        let (left, right) = (left.as_bytes(), right.as_bytes());
        let similarity = bit_parallel_jaro(left, right);
        if similarity > 0.7 {
            let prefix_length = left
                .iter()
                .take(4)
                .zip(right)
                .take_while(|(left, right)| left == right)
                .count();
            similarity + 0.1 * prefix_length as f64 * (1.0 - similarity)
        } else {
            similarity
        }
    })
}

/// strsim 0.11's Jaro similarity over ASCII byte slices of at most 128 bytes each. Each left byte
/// takes the first unmatched equal right byte inside the window `[i - range, i + range]`, with
/// `range = max(len) / 2 - 1`; two empty inputs score 1.0 and no match scores 0.0.
fn bit_parallel_jaro(left: &[u8], right: &[u8]) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let search_range = (left.len().max(right.len()) / 2).saturating_sub(1);
    let mut right_positions = [0_u128; 128];
    for (index, &byte) in right.iter().enumerate() {
        right_positions[usize::from(byte)] |= 1 << index;
    }
    let mut left_matched = 0_u128;
    let mut right_matched = 0_u128;
    for (index, &byte) in left.iter().enumerate() {
        let window = low_bits(right.len().min(index + search_range + 1))
            & !low_bits(index.saturating_sub(search_range));
        let candidates = right_positions[usize::from(byte)] & window & !right_matched;
        // strsim scans the window upward, so the first match is the lowest candidate bit.
        right_matched |= candidates & candidates.wrapping_neg();
        left_matched |= u128::from(candidates != 0) << index;
    }
    // An empty side leaves both masks empty, which scores 0.0 as strsim does.
    let matches = left_matched.count_ones() as usize;
    if matches == 0 {
        return 0.0;
    }
    let mut transpositions = 0_usize;
    let (mut left_rest, mut right_rest) = (left_matched, right_matched);
    while left_rest != 0 {
        transpositions += usize::from(
            left[left_rest.trailing_zeros() as usize]
                != right[right_rest.trailing_zeros() as usize],
        );
        left_rest &= left_rest - 1;
        right_rest &= right_rest - 1;
    }
    transpositions /= 2;
    ((matches as f64 / left.len() as f64)
        + (matches as f64 / right.len() as f64)
        + ((matches - transpositions) as f64 / matches as f64))
        / 3.0
}

/// A mask of the lowest `count` bits, for `count` in `0..=128`.
fn low_bits(count: usize) -> u128 {
    u128::MAX
        .checked_shr((MAX_BIT_PARALLEL_LEN - count) as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
