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
mod tests {
    use super::*;

    /// strsim's combined matcher score, the contract the fast path must reproduce.
    fn strsim_score(left: &str, right: &str) -> f64 {
        combined_score(left, right, strsim::levenshtein, strsim::jaro_winkler)
    }

    /// Asserts distance, Jaro-Winkler and combined score equal strsim bit for bit.
    fn assert_matches_strsim(left: &str, right: &str) {
        assert_eq!(
            levenshtein(left, right),
            strsim::levenshtein(left, right),
            "levenshtein {left:?} {right:?}"
        );
        assert_eq!(
            jaro_winkler(left, right).to_bits(),
            strsim::jaro_winkler(left, right).to_bits(),
            "jaro_winkler {left:?} {right:?}"
        );
        assert_eq!(
            matcher_text_similarity(left, right).to_bits(),
            strsim_score(left, right).to_bits(),
            "combined {left:?} {right:?}"
        );
    }

    /// `length` ASCII bytes framed by `first` and `last`, so a partner with other frame bytes
    /// trims nothing; the body cycles with `stride` to vary which interior bytes coincide.
    fn framed(length: usize, first: char, last: char, stride: usize) -> String {
        let body = (0..length - 2).map(|index| char::from(b'a' + ((index * stride) % 26) as u8));
        std::iter::once(first)
            .chain(body)
            .chain(std::iter::once(last))
            .collect()
    }

    /// Texts of the given lengths whose first `matched` bytes coincide with distinct values and
    /// whose remaining bytes never match, so Jaro is `(m / left + m / right + 1) / 3`.
    fn jaro_fixture(left_length: usize, right_length: usize, matched: usize) -> (String, String) {
        let shared = (0..matched).map(|index| char::from(b'!' + index as u8));
        let left = shared
            .clone()
            .chain(std::iter::repeat_n('~', left_length - matched));
        let right = shared.chain(std::iter::repeat_n('|', right_length - matched));
        (left.collect(), right.collect())
    }

    struct Case {
        name: &'static str,
        left: String,
        right: String,
        bit_parallel_levenshtein: bool,
        bit_parallel_jaro_winkler: bool,
    }

    /// Builds a [`Case`] from borrowed or owned texts.
    fn case(
        name: &'static str,
        left: impl Into<String>,
        right: impl Into<String>,
        bit_parallel_levenshtein: bool,
        bit_parallel_jaro_winkler: bool,
    ) -> Case {
        Case {
            name,
            left: left.into(),
            right: right.into(),
            bit_parallel_levenshtein,
            bit_parallel_jaro_winkler,
        }
    }

    /// Every edge row equals strsim for distance, Jaro-Winkler and the combined score in both
    /// argument orders, and takes the bit-parallel or fallback path the row names.
    #[test]
    fn edge_cases_equal_strsim_and_take_the_expected_path() {
        // No pair within 128 bytes scores exactly 0.7, so these one-ULP neighbours are the
        // tightest probes of the Winkler threshold. Both share a leading byte, so a threshold that
        // boosts the lower one or skips the upper one changes the score.
        let (below_left, below_right) = jaro_fixture(60, 120, 44);
        let (above_left, above_right) = jaro_fixture(1, 10, 1);
        assert_eq!(
            strsim::jaro(&below_left, &below_right).to_bits(),
            0.7_f64.next_down().to_bits()
        );
        assert_eq!(
            strsim::jaro(&above_left, &above_right).to_bits(),
            0.7_f64.next_up().to_bits()
        );
        let shared_prefix = "p".repeat(40);
        let mut cases = vec![
            case("both empty", "", "", true, true),
            case("one empty", "", "abc", true, true),
            case(
                "identical",
                "the user logs in",
                "the user logs in",
                true,
                true,
            ),
            case("disjoint", "abc", "xyz", true, true),
            case("single equal char", "a", "a", true, true),
            case("single differing char", "a", "b", true, true),
            case("shared prefix", "prefix abc", "prefix xyz", true, true),
            case("shared suffix", "abc suffix", "xyz suffix", true, true),
            case(
                "shared prefix and suffix",
                "pre abc suf",
                "pre xy suf",
                true,
                true,
            ),
            case("overlapping affixes", "aaa", "aa", true, true),
            case("overlapping repeated unit", "abab", "ab", true, true),
            case("adjacent transposition", "martha", "marhta", true, true),
            case(
                "transpositions and length gap",
                "dixon",
                "dicksonx",
                true,
                true,
            ),
            case("pairwise swaps", "abcdef", "badcfe", true, true),
            case(
                "prefix longer than four",
                "abcdefgh one",
                "abcdefgh two",
                true,
                true,
            ),
            case(
                "shared bytes outside the window",
                "abcdef",
                "fxxxxa",
                true,
                true,
            ),
            case("jaro just below 0.7", below_left, below_right, true, true),
            case("jaro just above 0.7", above_left, above_right, true, true),
            case(
                "long shared prefix trims into a word",
                format!("{shared_prefix}{}", framed(100, 'L', 'l', 7)),
                format!("{shared_prefix}{}", framed(100, 'R', 'r', 11)),
                true,
                false,
            ),
            case("non-ASCII left", "café", "cafe", false, false),
            case("non-ASCII right", "strasse", "straße", false, false),
            case("emoji", "😀 smile", "smile 😀", false, false),
            case("non-ASCII against empty", "é", "", false, false),
        ];
        for length in [63, 64, 65, 127, 128, 129] {
            let fits = length <= MAX_BIT_PARALLEL_LEN;
            cases.push(case(
                "length after trimming",
                framed(length, 'L', 'l', 7),
                framed(length, 'R', 'r', 11),
                fits,
                fits,
            ));
        }
        for case in &cases {
            for (left, right) in [(&case.left, &case.right), (&case.right, &case.left)] {
                assert_matches_strsim(left, right);
                assert_eq!(
                    bit_parallel_levenshtein(left, right).is_some(),
                    case.bit_parallel_levenshtein,
                    "levenshtein path: {}",
                    case.name
                );
                assert_eq!(
                    bit_parallel_jaro_winkler(left, right).is_some(),
                    case.bit_parallel_jaro_winkler,
                    "jaro-winkler path: {}",
                    case.name
                );
            }
        }
    }

    /// Seeded pairs over ASCII and non-ASCII alphabets, lengths 0 to 140, independent or derived by
    /// edits, equal strsim exactly for distance, Jaro-Winkler and the combined score.
    #[test]
    fn seeded_pairs_equal_strsim() {
        let alphabets: [&[char]; 4] = [
            &['a', 'b', 'c', ' '],
            &[
                'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', ' ', '1', '{', '}',
            ],
            &['a', 'b', 'é', 'ß', ' ', 'x', '😀'],
            &[
                't', 'h', 'e', ' ', 'u', 's', 'r', 'i', 'n', 'o', 'g', 'l', 'd',
            ],
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for round in 0..10_000 {
            let alphabet = alphabets[round % alphabets.len()];
            let max_length = [8, 40, 140][(round / alphabets.len()) % 3];
            let text = |next: &mut dyn FnMut(usize) -> usize| {
                (0..next(max_length + 1))
                    .map(|_| alphabet[next(alphabet.len())])
                    .collect::<Vec<_>>()
            };
            let left = text(&mut next);
            let right = if next(2) == 0 {
                let mut edited = left.clone();
                for _ in 0..next(5) {
                    let at = next(edited.len() + 1);
                    let replacement = alphabet[next(alphabet.len())];
                    if at == edited.len() || next(3) == 0 {
                        edited.insert(at, replacement);
                    } else if next(2) == 0 {
                        edited.remove(at);
                    } else {
                        edited[at] = replacement;
                    }
                }
                edited
            } else {
                text(&mut next)
            };
            let left = left.into_iter().collect::<String>();
            let right = right.into_iter().collect::<String>();
            assert_matches_strsim(&left, &right);
        }
    }
}
