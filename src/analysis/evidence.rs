use crate::model::{
    stable_fingerprint, DefinitionComparison, MatcherDiff, MatcherKind, StepDefinition,
};

pub(super) fn matcher_difference(left: &StepDefinition, right: &StepDefinition) -> String {
    format!("`{}` ↔ `{}`", left.matcher, right.matcher)
}

pub(super) fn definition_comparison(
    left: &StepDefinition,
    right: &StepDefinition,
) -> DefinitionComparison {
    DefinitionComparison {
        left_fingerprint: definition_semantic_fingerprint(left),
        right_fingerprint: definition_semantic_fingerprint(right),
        left_matcher: left.matcher.clone(),
        right_matcher: right.matcher.clone(),
        left_handler: left.handler.source_snippet.clone(),
        right_handler: right.handler.source_snippet.clone(),
        matcher_diff: matcher_diff(&left.matcher, &right.matcher),
    }
}

fn definition_semantic_fingerprint(definition: &StepDefinition) -> String {
    let matcher_kind = match definition.matcher_kind {
        MatcherKind::CucumberExpression => "cucumber-expression",
        MatcherKind::RegularExpression => "regular-expression",
    };
    stable_fingerprint(&format!(
        "{matcher_kind}\u{0}{}\u{0}{}\u{0}{}",
        definition.normalized_matcher,
        definition.matcher_flags,
        definition.handler.alpha_normalized
    ))
}

fn matcher_diff(left: &str, right: &str) -> MatcherDiff {
    let left_chars: Vec<_> = left.chars().collect();
    let right_chars: Vec<_> = right.chars().collect();
    let prefix_len = left_chars
        .iter()
        .zip(&right_chars)
        .take_while(|(left, right)| left == right)
        .count();
    let max_suffix = left_chars
        .len()
        .saturating_sub(prefix_len)
        .min(right_chars.len().saturating_sub(prefix_len));
    let suffix_len = left_chars
        .iter()
        .rev()
        .zip(right_chars.iter().rev())
        .take(max_suffix)
        .take_while(|(left, right)| left == right)
        .count();

    MatcherDiff {
        prefix: left_chars[..prefix_len].iter().collect(),
        left_change: left_chars[prefix_len..left_chars.len() - suffix_len]
            .iter()
            .collect(),
        right_change: right_chars[prefix_len..right_chars.len() - suffix_len]
            .iter()
            .collect(),
        suffix: left_chars[left_chars.len() - suffix_len..].iter().collect(),
    }
}

pub(super) fn handler_evidence(left: &StepDefinition, right: &StepDefinition) -> String {
    if left.handler.exact == right.handler.exact {
        "Handlers have the same exact syntax fingerprint".to_owned()
    } else if left.handler.normalized == right.handler.normalized {
        "Handlers differ only in comments or formatting".to_owned()
    } else if left.handler.alpha_normalized == right.handler.alpha_normalized {
        "Handlers differ only in parameter or local-variable names".to_owned()
    } else if left.handler.structural == right.handler.structural {
        "Handlers share the same structure after literal normalization".to_owned()
    } else {
        "Handler behavior signatures differ".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{SourceFile, SourceLanguage};
    use std::path::PathBuf;

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
            assert_eq!(fingerprint.len(), 16);
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
            pair[0].handler.structural =
                if name == "structural" { "same" } else { "left" }.to_owned();
            pair[1].handler.structural = if name == "structural" {
                "same"
            } else {
                "right"
            }
            .to_owned();
            assert_eq!(handler_evidence(&pair[0], &pair[1]), expected, "{name}");
        }
    }

    #[test]
    fn semantic_fingerprint_distinguishes_matcher_syntax() {
        let mut pair = definitions(
            "Given('same matcher', () => work()); Given(/same matcher/, () => work());",
        );
        pair[1].normalized_matcher = pair[0].normalized_matcher.clone();
        assert_ne!(
            definition_semantic_fingerprint(&pair[0]),
            definition_semantic_fingerprint(&pair[1])
        );
    }
}
