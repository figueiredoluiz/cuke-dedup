use super::evidence::{definition_comparison, handler_evidence, matcher_difference};
use super::similarity::{handler_similarity, is_near_matcher, matcher_similarity, round_score};
use super::suppression::find_suppression;
use super::{AnalysisCensus, CandidateSourceCensus};
use crate::config::Config;
use crate::model::{
    Finding, FindingEvidence, MatcherKind, Rule, Severity, SourceLocation, StepDefinition,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const NORMALIZED_MATCHER_SOURCE: &str = "normalizedMatcher";
const IDENTICAL_HANDLER_SOURCE: &str = "identicalHandler";
const STRUCTURAL_HANDLER_SOURCE: &str = "structuralHandler";

pub(super) struct PairAnalysis {
    pub(super) census: AnalysisCensus,
    pub(super) operational_error: Option<String>,
}

pub(super) fn analyze_definition_pairs(
    definitions: &[StepDefinition],
    config: &Config,
    findings: &mut Vec<Finding>,
) -> PairAnalysis {
    let generated = definition_pair_candidates(definitions, config);
    let mut forests = FindingForests::new(definitions.len());
    for &(left_index, right_index) in &generated.candidates {
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
    let operational_error = generated.operational_error(config);
    PairAnalysis {
        census: generated.census,
        operational_error,
    }
}

pub(super) fn definition_pair_candidates(
    definitions: &[StepDefinition],
    config: &Config,
) -> CandidateGeneration {
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

    let mut builder = CandidateBuilder::new(config.max_candidate_comparisons);
    for group in sorted_groups(normalized_matchers) {
        let mut exact_groups: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
        for index in group {
            let definition = &definitions[index];
            exact_groups
                .entry((&definition.matcher, &definition.matcher_flags))
                .or_default()
                .push(index);
        }
        let exact_groups = sorted_groups(exact_groups);
        for exact_group in &exact_groups {
            insert_spanning(exact_group, NORMALIZED_MATCHER_SOURCE, &mut builder);
        }
        insert_multipartite_spanning(&exact_groups, NORMALIZED_MATCHER_SOURCE, &mut builder);
    }

    for group in sorted_groups(handlers) {
        let mut matcher_groups: HashMap<(MatcherKind, &str), Vec<usize>> = HashMap::new();
        for index in group {
            let definition = &definitions[index];
            matcher_groups
                .entry((definition.matcher_kind, &definition.normalized_matcher))
                .or_default()
                .push(index);
        }
        insert_multipartite_spanning(
            &sorted_groups(matcher_groups),
            IDENTICAL_HANDLER_SOURCE,
            &mut builder,
        );
    }

    let mut structural_classes = Vec::new();
    let mut structural_class_by_definition = vec![None; definitions.len()];
    let mut handler_group_by_definition = vec![0_usize; definitions.len()];
    for (class_index, group) in sorted_groups(structures).into_iter().enumerate() {
        let mut handler_groups: HashMap<&str, Vec<usize>> = HashMap::new();
        for &index in &group {
            handler_groups
                .entry(&definitions[index].handler.alpha_normalized)
                .or_default()
                .push(index);
        }
        let handler_groups = sorted_groups(handler_groups);
        for (handler_group, indexes) in handler_groups.iter().enumerate() {
            for &index in indexes {
                structural_class_by_definition[index] = Some(class_index);
                handler_group_by_definition[index] = handler_group;
            }
        }
        structural_classes.push(StructuralClass {
            anchor: definitions[group[0]].location.clone(),
            unique_proposals: multipartite_pair_count(&handler_groups),
            handler_groups,
        });
    }
    for &(left, right) in &builder.proposals {
        let Some(class_index) = structural_class_by_definition[left] else {
            continue;
        };
        if structural_class_by_definition[right] == Some(class_index)
            && handler_group_by_definition[left] != handler_group_by_definition[right]
        {
            structural_classes[class_index].unique_proposals = structural_classes[class_index]
                .unique_proposals
                .saturating_sub(1);
        }
    }

    let mut truncated_structural_classes = Vec::new();
    for structural_class in structural_classes {
        let mut inserted = 0_u64;
        let per_class_limit =
            u64::try_from(config.max_structural_class_comparisons).unwrap_or(u64::MAX);
        'groups: for left_group in 0..structural_class.handler_groups.len() {
            for right_group in left_group + 1..structural_class.handler_groups.len() {
                for left in &structural_class.handler_groups[left_group] {
                    for right in &structural_class.handler_groups[right_group] {
                        if inserted >= per_class_limit {
                            let remaining =
                                structural_class.unique_proposals.saturating_sub(inserted);
                            if remaining > 0 {
                                builder.skip(STRUCTURAL_HANDLER_SOURCE, remaining);
                                truncated_structural_classes.push(structural_class.anchor.clone());
                            }
                            break 'groups;
                        }
                        match builder.insert(STRUCTURAL_HANDLER_SOURCE, *left, *right) {
                            InsertOutcome::Inserted => inserted = inserted.saturating_add(1),
                            InsertOutcome::Existing => {}
                            InsertOutcome::Limit => {
                                builder.skip(
                                    STRUCTURAL_HANDLER_SOURCE,
                                    structural_class
                                        .unique_proposals
                                        .saturating_sub(inserted)
                                        .saturating_sub(1),
                                );
                                truncated_structural_classes.push(structural_class.anchor.clone());
                                break 'groups;
                            }
                        }
                    }
                }
            }
        }
    }
    let census = builder.census(truncated_structural_classes.len());
    CandidateGeneration {
        candidates: builder.candidates,
        census,
        truncated_structural_classes,
    }
}

struct StructuralClass {
    anchor: SourceLocation,
    unique_proposals: u64,
    handler_groups: Vec<Vec<usize>>,
}

fn sorted_groups<K>(groups: HashMap<K, Vec<usize>>) -> Vec<Vec<usize>> {
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by_key(|group| group.first().copied().unwrap_or(usize::MAX));
    groups
}

fn insert_spanning(group: &[usize], source: &'static str, builder: &mut CandidateBuilder) {
    for pair in group.windows(2) {
        builder.insert(source, pair[0], pair[1]);
    }
}

fn insert_multipartite_spanning(
    groups: &[Vec<usize>],
    source: &'static str,
    builder: &mut CandidateBuilder,
) {
    if groups.len() < 2 {
        return;
    }
    let first_anchor = groups[0][0];
    let second_anchor = groups[1][0];
    for group in &groups[1..] {
        for index in group {
            builder.insert(source, first_anchor, *index);
        }
    }
    for index in &groups[0][1..] {
        builder.insert(source, second_anchor, *index);
    }
}

fn multipartite_pair_count(groups: &[Vec<usize>]) -> u64 {
    let mut prior = 0_u64;
    let mut total = 0_u64;
    for group in groups {
        let size = u64::try_from(group.len()).unwrap_or(u64::MAX);
        total = total.saturating_add(prior.saturating_mul(size));
        prior = prior.saturating_add(size);
    }
    total
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InsertOutcome {
    Existing,
    Inserted,
    Limit,
}

struct CandidateBuilder {
    candidates: BTreeSet<(usize, usize)>,
    proposals: BTreeSet<(usize, usize)>,
    limit: usize,
    sources: BTreeMap<&'static str, CandidateSourceCensus>,
}

impl CandidateBuilder {
    fn new(limit: usize) -> Self {
        Self {
            candidates: BTreeSet::new(),
            proposals: BTreeSet::new(),
            limit,
            sources: [
                (NORMALIZED_MATCHER_SOURCE, CandidateSourceCensus::default()),
                (IDENTICAL_HANDLER_SOURCE, CandidateSourceCensus::default()),
                (STRUCTURAL_HANDLER_SOURCE, CandidateSourceCensus::default()),
            ]
            .into_iter()
            .collect(),
        }
    }

    fn insert(&mut self, source: &'static str, left: usize, right: usize) -> InsertOutcome {
        let pair = candidate_pair(left, right);
        if !self.proposals.insert(pair) {
            return InsertOutcome::Existing;
        }
        if self.candidates.len() >= self.limit {
            self.skip(source, 1);
            return InsertOutcome::Limit;
        }
        self.candidates.insert(pair);
        self.sources.entry(source).or_default().evaluated += 1;
        InsertOutcome::Inserted
    }

    fn skip(&mut self, source: &'static str, count: u64) {
        let metrics = self.sources.entry(source).or_default();
        metrics.skipped = metrics.skipped.saturating_add(count);
    }

    fn census(&self, truncated_structural_classes: usize) -> AnalysisCensus {
        let skipped_candidate_comparisons = self
            .sources
            .values()
            .fold(0_u64, |total, source| total.saturating_add(source.skipped));
        AnalysisCensus {
            truncated: skipped_candidate_comparisons > 0,
            candidate_comparisons_evaluated: self.candidates.len(),
            skipped_candidate_comparisons,
            truncated_structural_classes,
            candidate_sources: self
                .sources
                .iter()
                .map(|(name, census)| ((*name).to_owned(), census.clone()))
                .collect(),
        }
    }
}

fn candidate_pair(left: usize, right: usize) -> (usize, usize) {
    (left.min(right), left.max(right))
}

pub(super) struct CandidateGeneration {
    pub(super) candidates: BTreeSet<(usize, usize)>,
    pub(super) census: AnalysisCensus,
    truncated_structural_classes: Vec<SourceLocation>,
}

impl CandidateGeneration {
    fn operational_error(&self, config: &Config) -> Option<String> {
        self.census.truncated.then(|| {
            let mut affected = self
                .truncated_structural_classes
                .iter()
                .take(3)
                .map(|location| location.display(&config.root))
                .collect::<Vec<_>>()
                .join(", ");
            let additional = self.truncated_structural_classes.len().saturating_sub(3);
            if additional > 0 {
                affected.push_str(&format!(", and {additional} more"));
            }
            let class_detail = if affected.is_empty() {
                String::new()
            } else {
                format!("; affected structural classes start at {affected}")
            };
            format!(
                "analysis is incomplete: evaluated {} candidate definition comparisons and skipped {} after configured limits{}; partial findings are available",
                self.census.candidate_comparisons_evaluated,
                self.census.skipped_candidate_comparisons,
                class_detail
            )
        })
    }
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
