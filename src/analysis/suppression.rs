use crate::config::{Config, SuppressionConfig};
use crate::model::{Rule, StepDefinition};
use crate::resource_limits::MAX_SUPPRESSION_REASON_CHARS;
use globset::{Glob, GlobMatcher};
use std::collections::BTreeMap;

const MAX_UNMATCHED_SUPPRESSION_WORK: u64 = 10_000_000;

#[derive(Default)]
pub(crate) struct UnmatchedSuppressionOutcome {
    pub(crate) indices: Vec<usize>,
    pub(crate) truncated: bool,
}

struct CompiledSuppression<'a> {
    config: &'a SuppressionConfig,
    path_matcher: Option<GlobMatcher>,
}

pub(super) struct SuppressionIndex<'a> {
    definitions: &'a [StepDefinition],
    relative_paths: Vec<String>,
    compiled: Vec<CompiledSuppression<'a>>,
    by_rule: BTreeMap<Rule, Vec<usize>>,
    inline_by_definition: Vec<BTreeMap<Rule, &'a str>>,
}

impl<'a> SuppressionIndex<'a> {
    pub(super) fn new(config: &'a Config, definitions: &'a [StepDefinition]) -> Self {
        let relative_paths = definitions
            .iter()
            .map(|definition| {
                definition
                    .location
                    .path
                    .strip_prefix(&config.root)
                    .unwrap_or(&definition.location.path)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        let mut compiled = Vec::with_capacity(config.suppressions.len());
        let mut by_rule: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for suppression in &config.suppressions {
            let index = compiled.len();
            compiled.push(CompiledSuppression {
                config: suppression,
                path_matcher: compile_path_matcher(suppression),
            });
            by_rule.entry(suppression.rule).or_default().push(index);
        }
        let inline_by_definition = definitions
            .iter()
            .map(|definition| {
                definition.inline_suppressions.iter().fold(
                    BTreeMap::new(),
                    |mut rules, suppression| {
                        rules
                            .entry(suppression.rule)
                            .or_insert(suppression.reason.as_str());
                        rules
                    },
                )
            })
            .collect();
        Self {
            definitions,
            relative_paths,
            compiled,
            by_rule,
            inline_by_definition,
        }
    }

    pub(super) fn find_reason(&self, rule: Rule, definition_indices: &[usize]) -> Option<&'a str> {
        self.by_rule
            .get(&rule)
            .into_iter()
            .flatten()
            .filter_map(|&index| self.compiled.get(index))
            .find_map(|suppression| {
                suppression_matches(suppression, definition_indices, self)
                    .then(|| bounded_reason(&suppression.config.reason))
            })
            .or_else(|| {
                definition_indices.iter().find_map(|&index| {
                    self.inline_by_definition[index]
                        .get(&rule)
                        .map(|reason| bounded_reason(reason))
                })
            })
    }

    pub(super) fn lookup_work(&self, rule: Rule, definition_indices: &[usize]) -> u64 {
        let suppression_count = self.by_rule.get(&rule).map_or(0, Vec::len);
        let input_bytes = definition_indices.iter().fold(1_u64, |total, &index| {
            total.saturating_add(self.selection_work(index))
        });
        u64::try_from(suppression_count)
            .unwrap_or(u64::MAX)
            .saturating_mul(input_bytes)
            // A path-selected entry can inspect every path once for its all-in-scope check and
            // again while requiring path and matcher selectors to choose the same definition.
            .saturating_mul(2)
    }

    pub(super) fn unmatched(&self) -> UnmatchedSuppressionOutcome {
        let mut outcome = UnmatchedSuppressionOutcome::default();
        let mut work = 0_u64;
        for (index, compiled) in self.compiled.iter().enumerate() {
            let mut matched = false;
            for definition_index in 0..self.definitions.len() {
                let next_work = work.saturating_add(self.selection_work(definition_index));
                if next_work > MAX_UNMATCHED_SUPPRESSION_WORK {
                    outcome.truncated = true;
                    return outcome;
                }
                work = next_work;
                if suppression_selects(compiled, definition_index, self) {
                    matched = true;
                    break;
                }
            }
            if !matched {
                outcome.indices.push(index);
            }
        }
        outcome
    }

    #[cfg(test)]
    pub(super) fn compiled_path_count(&self) -> usize {
        self.compiled
            .iter()
            .filter(|suppression| suppression.config.path.is_some())
            .count()
    }

    fn selection_work(&self, definition_index: usize) -> u64 {
        let path_bytes =
            u64::try_from(self.relative_paths[definition_index].len()).unwrap_or(u64::MAX);
        let matcher_bytes =
            u64::try_from(self.definitions[definition_index].matcher.len()).unwrap_or(u64::MAX);
        1_u64
            .saturating_add(path_bytes)
            .saturating_add(matcher_bytes)
    }
}

fn suppression_matches(
    suppression: &CompiledSuppression<'_>,
    definition_indices: &[usize],
    index: &SuppressionIndex<'_>,
) -> bool {
    if suppression.config.path.is_some() && suppression.path_matcher.is_none() {
        return false;
    }
    if suppression.config.path.is_some()
        && !definition_indices
            .iter()
            .all(|&definition_index| path_selects(suppression, definition_index, index))
    {
        return false;
    }
    definition_indices.iter().any(|&definition_index| {
        matcher_selects(suppression.config, &index.definitions[definition_index])
            && path_selects(suppression, definition_index, index)
    })
}

fn suppression_selects(
    suppression: &CompiledSuppression<'_>,
    definition_index: usize,
    index: &SuppressionIndex<'_>,
) -> bool {
    if suppression.config.path.is_some() && suppression.path_matcher.is_none() {
        return false;
    }
    path_selects(suppression, definition_index, index)
        && matcher_selects(suppression.config, &index.definitions[definition_index])
}

fn path_selects(
    suppression: &CompiledSuppression<'_>,
    definition_index: usize,
    index: &SuppressionIndex<'_>,
) -> bool {
    suppression
        .path_matcher
        .as_ref()
        .is_none_or(|matcher| matcher.is_match(&index.relative_paths[definition_index]))
}

fn matcher_selects(suppression: &SuppressionConfig, definition: &StepDefinition) -> bool {
    suppression
        .matcher
        .as_ref()
        .is_none_or(|matcher| definition.matcher == *matcher)
}

fn compile_path_matcher(suppression: &SuppressionConfig) -> Option<GlobMatcher> {
    suppression.path.as_ref().and_then(|pattern| {
        let pattern = if pattern.ends_with('/') {
            format!("{pattern}**")
        } else {
            pattern.clone()
        };
        Glob::new(&pattern).ok().map(|glob| glob.compile_matcher())
    })
}

fn bounded_reason(reason: &str) -> &str {
    reason
        .char_indices()
        .nth(MAX_SUPPRESSION_REASON_CHARS)
        .map_or(reason, |(boundary, _)| &reason[..boundary])
}
