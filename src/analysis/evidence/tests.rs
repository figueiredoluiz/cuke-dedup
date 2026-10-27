use super::*;
use crate::discovery::{SourceFile, SourceLanguage};
use std::path::PathBuf;

/// Extracts TypeScript definitions from `source` as `steps.ts`; panics if extraction fails.
fn definitions(source: &str) -> Vec<StepDefinition> {
    crate::typescript::extract(
        source,
        &SourceFile {
            path: PathBuf::from("steps.ts"),
            language: SourceLanguage::TypeScript,
        },
    )
    .unwrap()
}

/// Pins prefix, change and suffix splits for empty, identical, disjoint, shared-suffix and
/// multi-byte Unicode matchers.
#[test]
fn matcher_diffs_cover_empty_identical_disjoint_and_unicode_inputs() {
    let cases = [
        ("empty", "", "", ("", "", "", "")),
        ("identical", "same", "same", ("same", "", "", "")),
        ("disjoint", "abc", "xyz", ("", "abc", "xyz", "")),
        (
            "shared suffix",
            "account enabled",
            "user enabled",
            ("", "account", "user", " enabled"),
        ),
        (
            "unicode scalar boundaries",
            "再生ボタン",
            "停止ボタン",
            ("", "再生", "停止", "ボタン"),
        ),
    ];

    for (name, left, right, expected) in cases {
        let diff = matcher_diff(left, right);
        assert_eq!(
            (
                diff.prefix.as_str(),
                diff.left_change.as_str(),
                diff.right_change.as_str(),
                diff.suffix.as_str(),
            ),
            expected,
            "{name}"
        );
    }
}

/// Pins that a clone keeps its fingerprint while moving bytes from the normalized matcher into
/// the flags changes it, so a NUL inside a component cannot forge a boundary.
#[test]
fn semantic_fingerprints_preserve_component_boundaries() {
    let mut accepted = definitions("Given('accepted', () => action());").remove(0);
    accepted.normalized_matcher = "a\0b".to_owned();
    let equivalent = accepted.clone();
    let mut unrelated = accepted.clone();
    unrelated.normalized_matcher = "a".to_owned();
    unrelated.matcher_flags = "b\0".to_owned();

    assert_eq!(
        definition_semantic_fingerprint(&accepted),
        definition_semantic_fingerprint(&equivalent)
    );
    assert_ne!(
        definition_semantic_fingerprint(&accepted),
        definition_semantic_fingerprint(&unrelated)
    );
}

/// Pins the `matcher_difference` rendering, the comparison's matchers, distinct 32-hex
/// fingerprints for different handlers, and every `handler_evidence` tier.
#[test]
fn pair_evidence_contract_includes_matchers_fingerprints_and_handler_classification() {
    let definitions = definitions(
        "Given('left matcher', () => first()); Given('right matcher', () => second());",
    );
    assert_eq!(
        matcher_difference(&definitions[0], &definitions[1]),
        "`left matcher` ↔ `right matcher`"
    );
    let comparison = definition_comparison(&definitions[0], &definitions[1]);
    assert_eq!(comparison.left_matcher, "left matcher");
    assert_eq!(comparison.right_matcher, "right matcher");
    assert_ne!(comparison.left_fingerprint, comparison.right_fingerprint);
    for fingerprint in [comparison.left_fingerprint, comparison.right_fingerprint] {
        assert_eq!(fingerprint.len(), 32);
        assert!(fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    let mut pair = definitions;
    let cases = [
        (
            "exact",
            "same",
            "same",
            "same",
            "same",
            "same",
            "same",
            "Handlers have the same exact syntax fingerprint",
        ),
        (
            "normalized",
            "left",
            "right",
            "same",
            "same",
            "same",
            "same",
            "Handlers differ only in comments or formatting",
        ),
        (
            "alpha",
            "left",
            "right",
            "left",
            "right",
            "same",
            "same",
            "Handlers differ only in parameter or local-variable names",
        ),
        (
            "structural",
            "left",
            "right",
            "left",
            "right",
            "left",
            "right",
            "Handlers share the same structure after literal normalization",
        ),
        (
            "different",
            "left",
            "right",
            "left",
            "right",
            "left",
            "right",
            "Handler behavior signatures differ",
        ),
    ];
    for (
        name,
        left_exact,
        right_exact,
        left_normalized,
        right_normalized,
        left_alpha,
        right_alpha,
        expected,
    ) in cases
    {
        pair[0].handler.exact = left_exact.to_owned();
        pair[1].handler.exact = right_exact.to_owned();
        pair[0].handler.normalized = left_normalized.to_owned();
        pair[1].handler.normalized = right_normalized.to_owned();
        pair[0].handler.alpha_normalized = left_alpha.to_owned();
        pair[1].handler.alpha_normalized = right_alpha.to_owned();
        pair[0].handler.structural = if name == "structural" { "same" } else { "left" }.to_owned();
        pair[1].handler.structural = if name == "structural" {
            "same"
        } else {
            "right"
        }
        .to_owned();
        assert_eq!(handler_evidence(&pair[0], &pair[1]), expected, "{name}");
    }
}

/// Pins that a string and a regex matcher with equal normalized text and identical handlers get
/// different fingerprints.
#[test]
fn semantic_fingerprint_distinguishes_matcher_syntax() {
    let mut pair =
        definitions("Given('same matcher', () => work()); Given(/same matcher/, () => work());");
    pair[1].normalized_matcher = pair[0].normalized_matcher.clone();
    assert_ne!(
        definition_semantic_fingerprint(&pair[0]),
        definition_semantic_fingerprint(&pair[1])
    );
}
