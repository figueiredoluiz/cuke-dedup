use crate::config::{Config, SuppressionConfig};
use crate::model::{Rule, StepDefinition, Suppression};
use globset::Glob;

pub(super) fn unmatched_suppressions(
    config: &Config,
    definitions: &[StepDefinition],
) -> Vec<usize> {
    config
        .suppressions
        .iter()
        .enumerate()
        .filter_map(|(index, suppression)| {
            let path_matcher = compile_path_matcher(suppression);
            (!definitions.iter().any(|definition| {
                suppression_selects(suppression, path_matcher.as_ref(), definition, &config.root)
            }))
            .then_some(index)
        })
        .collect()
}

pub(super) fn find_suppression(
    config: &Config,
    rule: Rule,
    definitions: &[&StepDefinition],
) -> Option<Suppression> {
    config
        .suppressions
        .iter()
        .find_map(|suppression| {
            suppression_matches(suppression, rule, definitions, &config.root).then(|| Suppression {
                reason: suppression.reason.clone(),
            })
        })
        .or_else(|| {
            definitions.iter().find_map(|definition| {
                definition
                    .inline_suppressions
                    .iter()
                    .find(|suppression| suppression.rule == rule)
                    .map(|suppression| Suppression {
                        reason: suppression.reason.clone(),
                    })
            })
        })
}

fn suppression_matches(
    suppression: &SuppressionConfig,
    rule: Rule,
    definitions: &[&StepDefinition],
    root: &std::path::Path,
) -> bool {
    if suppression.rule != rule {
        return false;
    }
    let path_matcher = compile_path_matcher(suppression);
    if suppression.path.is_some() && path_matcher.is_none() {
        return false;
    }
    if suppression.path.is_some()
        && !definitions
            .iter()
            .all(|definition| path_selects(path_matcher.as_ref(), definition, root))
    {
        return false;
    }
    definitions.iter().any(|definition| {
        matcher_selects(suppression, definition)
            && path_selects(path_matcher.as_ref(), definition, root)
    })
}

fn suppression_selects(
    suppression: &SuppressionConfig,
    path_matcher: Option<&globset::GlobMatcher>,
    definition: &StepDefinition,
    root: &std::path::Path,
) -> bool {
    if suppression.path.is_some() && path_matcher.is_none() {
        return false;
    }
    path_selects(path_matcher, definition, root) && matcher_selects(suppression, definition)
}

fn path_selects(
    path_matcher: Option<&globset::GlobMatcher>,
    definition: &StepDefinition,
    root: &std::path::Path,
) -> bool {
    path_matcher.is_none_or(|matcher| {
        let path = definition
            .location
            .path
            .strip_prefix(root)
            .unwrap_or(&definition.location.path)
            .to_string_lossy()
            .replace('\\', "/");
        matcher.is_match(&path)
    })
}

fn matcher_selects(suppression: &SuppressionConfig, definition: &StepDefinition) -> bool {
    suppression
        .matcher
        .as_ref()
        .is_none_or(|matcher| definition.matcher == *matcher)
}

fn compile_path_matcher(suppression: &SuppressionConfig) -> Option<globset::GlobMatcher> {
    suppression.path.as_ref().and_then(|pattern| {
        let pattern = if pattern.ends_with('/') {
            format!("{pattern}**")
        } else {
            pattern.clone()
        };
        Glob::new(&pattern).ok().map(|glob| glob.compile_matcher())
    })
}
