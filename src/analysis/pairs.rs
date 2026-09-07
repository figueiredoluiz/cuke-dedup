use super::evidence::{definition_comparison, handler_evidence, matcher_difference};
use super::similarity::{handler_similarity, is_near_matcher, matcher_similarity, round_score};
use super::suppression::find_suppression;
use crate::config::Config;
use crate::model::{Finding, FindingEvidence, MatcherKind, Rule, Severity, StepDefinition};
use anyhow::{bail, Result};
use std::collections::{BTreeSet, HashMap};

#[cfg(not(test))]
const MAX_DEFINITION_PAIR_CANDIDATES: usize = 2_000_000;
#[cfg(test)]
const MAX_DEFINITION_PAIR_CANDIDATES: usize = 10_000;

pub(super) fn analyze_definition_pairs(
    definitions: &[StepDefinition],
    config: &Config,
    findings: &mut Vec<Finding>,
) -> Result<()> {
    let mut forests = FindingForests::new(definitions.len());
    for (left_index, right_index) in definition_pair_candidates(definitions)? {
        let left = &definitions[left_index];
        let right = &definitions[right_index];
        let exact_matcher = left.matcher_kind == right.matcher_kind
            && left.matcher == right.matcher
            && left.matcher_flags == right.matcher_flags;
        let normalized_matcher = left.matcher_kind == right.matcher_kind
            && left.normalized_matcher == right.normalized_matcher;
        let same_handler = left.handler.alpha_normalized == right.handler.alpha_normalized;
        let same_structure = left.handler.structural == right.handler.structural
            && left.handler.behavior_signature == right.handler.behavior_signature;
        let meaningful_handlers = left.handler.comparable
            && right.handler.comparable
            && !left.handler.trivial
            && !right.handler.trivial;

        // Equal normalized matchers have a known score. Every other candidate needs the
        // fuzzy score either to classify the finding or to retain report evidence.
        let matcher_similarity = if normalized_matcher {
            1.0
        } else {
            matcher_similarity(left, right)
        };
        let near_matcher = !normalized_matcher && is_near_matcher(left, right, matcher_similarity);

        if exact_matcher {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                Rule::DuplicateMatcher,
                left_index,
                right_index,
                left,
                right,
                "Two step definitions use the same effective matcher",
                matcher_similarity,
                handler_similarity(left, right),
                "Matchers are textually identical",
                handler_evidence(left, right),
                "Keep one definition or make the matchers intentionally distinct",
            );
        } else if normalized_matcher {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                Rule::NormalizedMatcher,
                left_index,
                right_index,
                left,
                right,
                "Two matchers are equivalent after normalization",
                matcher_similarity,
                handler_similarity(left, right),
                format!(
                    "`{}` normalizes to `{}`",
                    right.matcher, right.normalized_matcher
                ),
                handler_evidence(left, right),
                "Consolidate the definitions or use clearly distinct matchers",
            );
        }

        if !normalized_matcher && same_handler && meaningful_handlers {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                Rule::DuplicateHandler,
                left_index,
                right_index,
                left,
                right,
                "Different matchers use the same normalized implementation",
                matcher_similarity,
                handler_similarity(left, right),
                matcher_difference(left, right),
                "Handlers are identical after parameter and local-variable normalization",
                "Review whether one parameterized step can replace both definitions",
            );
        }

        if !normalized_matcher
            && !same_handler
            && same_structure
            && meaningful_handlers
            && matcher_similarity >= 0.6
        {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                Rule::ParameterizationCandidate,
                left_index,
                right_index,
                left,
                right,
                "Handler structures differ primarily in literal values",
                matcher_similarity,
                handler_similarity(left, right),
                matcher_difference(left, right),
                "Handlers have the same control flow and calls after literal normalization",
                "Consider replacing the literal differences with a step parameter",
            );
        } else if !normalized_matcher && near_matcher && same_structure && meaningful_handlers {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                Rule::NearDuplicateStep,
                left_index,
                right_index,
                left,
                right,
                "Matcher wording is very close and handler structure agrees",
                matcher_similarity,
                handler_similarity(left, right),
                matcher_difference(left, right),
                handler_evidence(left, right),
                "Check for wording drift and consolidate if the steps express one behavior",
            );
        }
    }
    Ok(())
}

pub(super) fn definition_pair_candidates(
    definitions: &[StepDefinition],
) -> Result<BTreeSet<(usize, usize)>> {
    let mut normalized_matchers: HashMap<(MatcherKind, &str), Vec<usize>> = HashMap::new();
    let mut handlers: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut structures: HashMap<(&str, &[String]), Vec<usize>> = HashMap::new();

    for (index, definition) in definitions.iter().enumerate() {
        normalized_matchers
            .entry((definition.matcher_kind, &definition.normalized_matcher))
            .or_default()
            .push(index);
        if definition.handler.comparable && !definition.handler.trivial {
            handlers
                .entry(&definition.handler.alpha_normalized)
                .or_default()
                .push(index);
            structures
                .entry((
                    &definition.handler.structural,
                    &definition.handler.behavior_signature,
                ))
                .or_default()
                .push(index);
        }
    }

    let mut candidates = BTreeSet::new();
    for group in normalized_matchers.values() {
        let mut exact_groups: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
        for index in group {
            let definition = &definitions[*index];
            exact_groups
                .entry((&definition.matcher, &definition.matcher_flags))
                .or_default()
                .push(*index);
        }
        let exact_groups = sorted_groups(exact_groups);
        for exact_group in &exact_groups {
            insert_spanning(exact_group, &mut candidates)?;
        }
        insert_multipartite_spanning(&exact_groups, &mut candidates)?;
    }

    for group in handlers.values() {
        let mut matcher_groups: HashMap<(MatcherKind, &str), Vec<usize>> = HashMap::new();
        for index in group {
            let definition = &definitions[*index];
            matcher_groups
                .entry((definition.matcher_kind, &definition.normalized_matcher))
                .or_default()
                .push(*index);
        }
        insert_multipartite_spanning(&sorted_groups(matcher_groups), &mut candidates)?;
    }

    for group in structures.values() {
        let mut handler_groups: HashMap<&str, Vec<usize>> = HashMap::new();
        for index in group {
            handler_groups
                .entry(&definitions[*index].handler.alpha_normalized)
                .or_default()
                .push(*index);
        }
        let handler_groups = sorted_groups(handler_groups);
        for left_group in 0..handler_groups.len() {
            for right_group in left_group + 1..handler_groups.len() {
                for left in &handler_groups[left_group] {
                    for right in &handler_groups[right_group] {
                        insert_candidate(*left, *right, &mut candidates)?;
                    }
                }
            }
        }
    }
    Ok(candidates)
}

fn sorted_groups<K>(groups: HashMap<K, Vec<usize>>) -> Vec<Vec<usize>> {
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by_key(|group| group.first().copied().unwrap_or(usize::MAX));
    groups
}

fn insert_spanning(group: &[usize], candidates: &mut BTreeSet<(usize, usize)>) -> Result<()> {
    for pair in group.windows(2) {
        insert_candidate(pair[0], pair[1], candidates)?;
    }
    Ok(())
}

fn insert_multipartite_spanning(
    groups: &[Vec<usize>],
    candidates: &mut BTreeSet<(usize, usize)>,
) -> Result<()> {
    if groups.len() < 2 {
        return Ok(());
    }
    let first_anchor = groups[0][0];
    let second_anchor = groups[1][0];
    for group in &groups[1..] {
        for index in group {
            insert_candidate(first_anchor, *index, candidates)?;
        }
    }
    for index in &groups[0][1..] {
        insert_candidate(second_anchor, *index, candidates)?;
    }
    Ok(())
}

fn insert_candidate(
    left: usize,
    right: usize,
    candidates: &mut BTreeSet<(usize, usize)>,
) -> Result<()> {
    let pair = if left < right {
        (left, right)
    } else {
        (right, left)
    };
    candidates.insert(pair);
    if candidates.len() > MAX_DEFINITION_PAIR_CANDIDATES {
        bail!(
            "analysis requires more than {MAX_DEFINITION_PAIR_CANDIDATES} candidate definition comparisons; split independent suites or narrow the analysis root"
        );
    }
    Ok(())
}

struct FindingForests {
    definition_count: usize,
    forests: HashMap<(Rule, bool), DisjointSet>,
}

impl FindingForests {
    fn new(definition_count: usize) -> Self {
        Self {
            definition_count,
            forests: HashMap::new(),
        }
    }

    fn retain_edge(&mut self, rule: Rule, suppressed: bool, left: usize, right: usize) -> bool {
        self.forests
            .entry((rule, suppressed))
            .or_insert_with(|| DisjointSet::new(self.definition_count))
            .union(left, right)
    }
}

struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl DisjointSet {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
            rank: vec![0; size],
        }
    }

    fn find(&mut self, mut node: usize) -> usize {
        let mut root = node;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        while self.parent[node] != node {
            let parent = self.parent[node];
            self.parent[node] = root;
            node = parent;
        }
        root
    }

    fn union(&mut self, left: usize, right: usize) -> bool {
        let mut left_root = self.find(left);
        let mut right_root = self.find(right);
        if left_root == right_root {
            return false;
        }
        if self.rank[left_root] < self.rank[right_root] {
            std::mem::swap(&mut left_root, &mut right_root);
        }
        self.parent[right_root] = left_root;
        if self.rank[left_root] == self.rank[right_root] {
            self.rank[left_root] += 1;
        }
        true
    }
}

#[allow(clippy::too_many_arguments)]
fn push_pair_finding(
    findings: &mut Vec<Finding>,
    forests: &mut FindingForests,
    config: &Config,
    rule: Rule,
    left_index: usize,
    right_index: usize,
    left: &StepDefinition,
    right: &StepDefinition,
    message: impl Into<String>,
    matcher_similarity: f64,
    handler_similarity: f64,
    matcher_difference: impl Into<String>,
    handler_evidence: impl Into<String>,
    suggested_action: impl Into<String>,
) {
    let severity = config.severity(rule);
    if severity == Severity::Off {
        return;
    }
    let suppression = find_suppression(config, rule, &[left, right]);
    if !forests.retain_edge(rule, suppression.is_some(), left_index, right_index) {
        return;
    }
    findings.push(Finding {
        rule,
        severity,
        message: message.into(),
        primary: left.location.clone(),
        related: vec![right.location.clone()],
        evidence: FindingEvidence {
            matcher_similarity: Some(round_score(matcher_similarity)),
            handler_similarity: Some(round_score(handler_similarity)),
            matcher_difference: matcher_difference.into(),
            handler_evidence: handler_evidence.into(),
            comparison: Some(definition_comparison(left, right)),
        },
        suggested_action: suggested_action.into(),
        suppression,
    });
}
