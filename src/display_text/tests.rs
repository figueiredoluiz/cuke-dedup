use super::{diagnostic_safe, is_invisible_format};
use crate::tests::property_runner::generated;
use proptest::prelude::*;
use serde::Deserialize;
use std::borrow::Cow;
use std::cell::Cell;

#[path = "../../tests/common/diagnostic_oracle.rs"]
mod diagnostic_oracle;
use diagnostic_oracle::diagnostic_unsafe as oracle_unsafe;

/// One shared diagnostic escape case, also read by the JavaScript helper's tests.
#[derive(Deserialize)]
struct Case {
    name: String,
    input: String,
    expected: String,
}

/// Builds the expected escape text by hand, independent of `char::escape_unicode`.
fn oracle_escape(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            other if oracle_unsafe(other) => format!("\\u{{{:x}}}", u32::from(other)),
            other => other.to_string(),
        })
        .collect()
}

/// Code points at and next to every unsafe interval boundary.
const BOUNDARIES: &[char] = &[
    '\0',
    '\t',
    '\n',
    '\r',
    '\u{1b}',
    '\u{1f}',
    ' ',
    '~',
    '\u{7f}',
    '\u{80}',
    '\u{85}',
    '\u{9f}',
    '\u{a0}',
    '\u{ac}',
    '\u{ad}',
    '\u{ae}',
    '\u{61b}',
    '\u{61c}',
    '\u{61d}',
    '\u{200a}',
    '\u{200b}',
    '\u{200f}',
    '\u{2010}',
    '\u{2027}',
    '\u{2028}',
    '\u{2029}',
    '\u{202a}',
    '\u{202e}',
    '\u{202f}',
    '\u{205f}',
    '\u{2060}',
    '\u{2069}',
    '\u{206a}',
    '\u{fefe}',
    '\u{feff}',
    '\u{ff00}',
    '\u{dffff}',
    '\u{e0000}',
    '\u{e007f}',
    '\u{e0080}',
    '\\',
    ':',
    '#',
];

/// Generated text matches the hand-built escape, keeps safe text borrowed, and leaves no unsafe
/// character or line break.
#[test]
fn generated_diagnostics_match_the_escape_oracle() {
    let unsafe_cases = Cell::new(0usize);
    let safe_cases = Cell::new(0usize);
    let piece = prop_oneof![
        3 => proptest::sample::select(BOUNDARIES),
        1 => any::<char>(),
    ];
    generated(proptest::collection::vec(piece, 0..24), |characters| {
        let input: String = characters.iter().collect();
        let actual = diagnostic_safe(&input);
        prop_assert_eq!(actual.as_ref(), oracle_escape(&input));
        let changed = input.chars().any(oracle_unsafe);
        prop_assert_eq!(matches!(actual, Cow::Owned(_)), changed);
        prop_assert!(!actual.chars().any(oracle_unsafe));
        if changed {
            unsafe_cases.set(unsafe_cases.get() + 1);
        } else {
            safe_cases.set(safe_cases.get() + 1);
        }
        Ok(())
    });
    assert!(
        unsafe_cases.get() >= 64,
        "unsafe stratum: {}",
        unsafe_cases.get()
    );
    assert!(safe_cases.get() >= 16, "safe stratum: {}", safe_cases.get());
}

/// The shared case file escapes the same way in Rust as in the Action helper.
#[test]
fn shared_escape_cases_match() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../fixtures/diagnostics/escapes.json")).unwrap();
    assert!(cases.len() >= 20);
    for case in cases {
        assert_eq!(diagnostic_safe(&case.input), case.expected, "{}", case.name);
    }
}

/// The invisible-format predicate shared with report sanitizing covers exactly the report ranges.
#[test]
fn invisible_format_matches_the_report_ranges() {
    for point in (0..=0x10ffff).filter_map(char::from_u32) {
        let expected = !point.is_control()
            && oracle_unsafe(point)
            && !matches!(point, '\u{2028}' | '\u{2029}');
        assert_eq!(
            is_invisible_format(point),
            expected,
            "U+{:04X}",
            u32::from(point)
        );
    }
}

/// Counts escaped bytes and keeps a single marker at the first refusal.
#[test]
fn retained_output_diagnostic_byte_limit_counts_escapes_and_line_overhead() {
    use super::{DiagnosticDetails, DIAGNOSTIC_LINE_OVERHEAD, DIAGNOSTIC_OMISSION};

    let text = "café\n";
    let bytes = "café\\n".len() + DIAGNOSTIC_LINE_OVERHEAD;
    let mut accepted = DiagnosticDetails::with_limits(2, bytes);
    accepted.push(text.to_owned());
    assert_eq!(accepted.iter().collect::<Vec<_>>(), [text]);

    let mut refused = DiagnosticDetails::with_limits(2, bytes - 1);
    refused.extend([text.to_owned(), "short".to_owned()]);
    assert_eq!(refused.into_lines(), [DIAGNOSTIC_OMISSION]);

    let mut sampled = DiagnosticDetails::with_limits(1, 64 * 1024);
    sampled.push("界".repeat(super::MAX_REPORT_FIELD_BYTES));
    let lines = sampled.into_lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("界"));
    assert!(lines[0].ends_with(" bytes omitted]"));
    assert!(lines[0].len() <= super::MAX_REPORT_FIELD_BYTES);
}
