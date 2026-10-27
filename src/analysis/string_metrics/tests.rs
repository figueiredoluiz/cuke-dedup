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
