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
