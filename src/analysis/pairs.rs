use super::evidence::{definition_comparison, handler_evidence, matcher_difference};
use super::similarity::{
    handler_similarity_with_relationship, is_near_matcher, matcher_similarity, round_score,
};
use super::suppression::SuppressionIndex;
use super::{AnalysisCensus, CandidateSourceCensus};
use crate::config::Config;
use crate::model::{
    DefinitionCluster, Finding, FindingEvidence, MatcherKind, Rule, Severity, SourceLocation,
    StepDefinition, Suppression,
};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hash;

const MATCHER_SHINGLE_WIDTH: usize = 3;
const MAX_MATCHER_SHINGLES_PER_DEFINITION: usize = 4_096;
const MAX_MATCHER_SHINGLES_TOTAL: usize = 250_000;
const MAX_MATCHER_BLOCKING_POSTING: usize = 256;
const MAX_MATCHER_BLOCKING_PROPOSAL_WORK: u64 = 2_000_000;
const MAX_MATCHER_BLOCKING_EVENT_WORK: u64 = 10_000_000;
// A single edit-distance or LCS matrix may cost this many cells before the pair is refused.
// Tests use a far smaller ceiling so the inputs that cross it stay small: an input sized to the
// production ceiling costs 100,000,000 real cell operations whenever the budget is removed, which
// is slow enough to look like a hang rather than a failure when the budget itself is under test.
#[cfg(not(test))]
const MAX_PAIR_MATRIX_WORK: u64 = 100_000_000;
#[cfg(test)]
const MAX_PAIR_MATRIX_WORK: u64 = 1_000_000;
const MAX_TOTAL_PAIR_SIMILARITY_WORK: u64 = 1_000_000_000;
const MAX_TOTAL_PAIR_SUPPRESSION_WORK: u64 = 100_000_000;
const PAIR_LINEAR_SCAN_MULTIPLIER: u64 = 4;
const HANDLER_SIMILARITY_GATE: f64 = 0.5;
const MIN_CLUSTER_DEFINITIONS: usize = 5;

#[derive(Clone, Copy)]
enum PairSimilarityWork {
    Matcher,
    Handler,
    Evidence,
}

struct PairWorkInput<'a> {
    left: &'a StepDefinition,
    right: &'a StepDefinition,
    left_matcher_length: usize,
    right_matcher_length: usize,
    left_comparison_bytes: u64,
    right_comparison_bytes: u64,
    relationships: PairRelationships,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CandidateSource {
    NormalizedMatcher,
    IdenticalHandler,
    StructuralHandler,
    MatcherBlocking,
}

impl CandidateSource {
    const ALL: [Self; 4] = [
        Self::NormalizedMatcher,
        Self::IdenticalHandler,
        Self::StructuralHandler,
        Self::MatcherBlocking,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::NormalizedMatcher => "normalizedMatcher",
            Self::IdenticalHandler => "identicalHandler",
            Self::StructuralHandler => "structuralHandler",
            Self::MatcherBlocking => "matcherBlocking",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct CandidateSources(u8);

impl CandidateSources {
    fn insert(&mut self, source: CandidateSource) {
        self.0 |= 1 << source as u8;
    }

    fn contains(self, source: CandidateSource) -> bool {
        self.0 & (1 << source as u8) != 0
    }

    fn can_feed_near_matcher(self) -> bool {
        self.contains(CandidateSource::IdenticalHandler)
            || self.contains(CandidateSource::StructuralHandler)
            || self.contains(CandidateSource::MatcherBlocking)
    }
}

#[derive(Debug, Clone, Copy)]
struct CandidatePair {
    left: usize,
    right: usize,
    sources: CandidateSources,
    owner: CandidateSource,
}

#[derive(Clone, Copy)]
struct PairRelationships {
    exact_matcher: bool,
    normalized_matcher: bool,
    same_handler: bool,
    same_handler_structure: bool,
    same_structure: bool,
}

struct ComparisonClasses {
    exact_matcher: Vec<usize>,
    normalized_matcher: Vec<usize>,
    alpha_handler: Vec<usize>,
    structural_handler: Vec<usize>,
    structure_and_behavior: Vec<usize>,
}

impl ComparisonClasses {
    fn relationships(&self, left: usize, right: usize) -> PairRelationships {
        PairRelationships {
            exact_matcher: self.exact_matcher[left] == self.exact_matcher[right],
            normalized_matcher: self.normalized_matcher[left] == self.normalized_matcher[right],
            same_handler: self.alpha_handler[left] == self.alpha_handler[right],
            same_handler_structure: self.structural_handler[left] == self.structural_handler[right],
            same_structure: self.structure_and_behavior[left] == self.structure_and_behavior[right],
        }
    }
}

pub(super) struct PairAnalysis {
    pub(super) census: AnalysisCensus,
    pub(super) incomplete: Option<String>,
}

pub(super) fn analyze_definition_pairs(
    definitions: &[StepDefinition],
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
) -> PairAnalysis {
    let first_pair_finding = findings.len();
    let mut generated = definition_pair_candidates(definitions, config);
    let matcher_lengths = definitions
        .iter()
        .map(|definition| definition.normalized_matcher.chars().count())
        .collect::<Vec<_>>();
    let comparison_bytes = definitions
        .iter()
        .map(definition_comparison_bytes)
        .collect::<Vec<_>>();
    let classes = &generated.comparison_classes;
    let behavior_events = &generated.behavior_events;
    let mut forests = FindingForests::new(definitions.len());
    let mut similarity_work = 0_u64;
    let mut suppression_work = 0_u64;
    let mut evaluated = 0_usize;
    for candidate in generated.candidates.values() {
        let left_index = candidate.left;
        let right_index = candidate.right;
        let left = &definitions[left_index];
        let right = &definitions[right_index];
        let relationships = classes.relationships(left_index, right_index);
        let exact_matcher = relationships.exact_matcher;
        let normalized_matcher = relationships.normalized_matcher;
        let same_handler = relationships.same_handler;
        let same_structure = relationships.same_structure;
        let meaningful_handlers = left.handler.comparable
            && right.handler.comparable
            && !left.handler.trivial
            && !right.handler.trivial;
        let pair_work = PairWorkInput {
            left,
            right,
            left_matcher_length: matcher_lengths[left_index],
            right_matcher_length: matcher_lengths[right_index],
            left_comparison_bytes: comparison_bytes[left_index],
            right_comparison_bytes: comparison_bytes[right_index],
            relationships,
        };

        let normalized_rule = candidate
            .sources
            .contains(CandidateSource::NormalizedMatcher)
            .then_some(if exact_matcher {
                Rule::DuplicateMatcher
            } else {
                Rule::NormalizedMatcher
            })
            .filter(|_| normalized_matcher);
        let duplicate_handler = candidate
            .sources
            .contains(CandidateSource::IdenticalHandler)
            && !normalized_matcher
            && same_handler
            && meaningful_handlers;
        let structural_handler = candidate
            .sources
            .contains(CandidateSource::StructuralHandler)
            && !normalized_matcher
            && !same_handler
            && same_structure
            && meaningful_handlers;
        let near_handler = candidate.sources.can_feed_near_matcher()
            && !structural_handler
            && !normalized_matcher
            && meaningful_handlers;
        let rule_is_active = |rule| config.severity(rule) != Severity::Off;
        let matcher_is_needed = !normalized_matcher
            && ((duplicate_handler && rule_is_active(Rule::DuplicateHandler))
                || (structural_handler && rule_is_active(Rule::ParameterizationCandidate))
                || (near_handler && rule_is_active(Rule::NearDuplicateStep)));

        if matcher_is_needed
            && !charge_matrix_work(
                &mut similarity_work,
                pair_similarity_work(&pair_work, PairSimilarityWork::Matcher),
            )
        {
            break;
        }
        let matcher_similarity = if normalized_matcher {
            1.0
        } else if matcher_is_needed {
            matcher_similarity(left, right)
        } else {
            0.0
        };
        let parameterization_candidate = structural_handler && matcher_similarity >= 0.6;
        let near_duplicate_candidate = !parameterization_candidate
            && near_handler
            && matcher_is_needed
            && is_near_matcher(left, right, matcher_similarity);
        let handler_is_needed = normalized_rule.is_some_and(rule_is_active)
            || (duplicate_handler && rule_is_active(Rule::DuplicateHandler))
            || (parameterization_candidate && rule_is_active(Rule::ParameterizationCandidate))
            || (near_duplicate_candidate && rule_is_active(Rule::NearDuplicateStep));
        if handler_is_needed
            && !charge_matrix_work(
                &mut similarity_work,
                pair_similarity_work(&pair_work, PairSimilarityWork::Handler),
            )
        {
            break;
        }
        let handler_similarity = if handler_is_needed {
            handler_similarity_with_relationship(
                same_handler,
                relationships.same_handler_structure,
                &behavior_events[left_index],
                &behavior_events[right_index],
            )
        } else {
            0.0
        };

        let normalized_finding = normalized_rule.filter(|rule| rule_is_active(*rule));
        let duplicate_finding = (duplicate_handler && rule_is_active(Rule::DuplicateHandler))
            .then_some(Rule::DuplicateHandler);
        let handler_finding =
            if parameterization_candidate && rule_is_active(Rule::ParameterizationCandidate) {
                Some(Rule::ParameterizationCandidate)
            } else if near_duplicate_candidate
                && handler_similarity >= HANDLER_SIMILARITY_GATE
                && rule_is_active(Rule::NearDuplicateStep)
            {
                Some(Rule::NearDuplicateStep)
            } else {
                None
            };
        let finding_rules = [normalized_finding, duplicate_finding, handler_finding];
        let finding_count = finding_rules.iter().flatten().count();
        let evidence_work = pair_similarity_work(&pair_work, PairSimilarityWork::Evidence)
            .saturating_mul(u64::try_from(finding_count).unwrap_or(u64::MAX));
        let candidate_suppression_work =
            candidate_suppression_work(&finding_rules, candidate, suppressions);
        if !charge_similarity_work(&mut similarity_work, evidence_work)
            || !charge_work(
                &mut suppression_work,
                candidate_suppression_work,
                MAX_TOTAL_PAIR_SUPPRESSION_WORK,
            )
        {
            break;
        }

        if normalized_finding == Some(Rule::DuplicateMatcher) {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                suppressions,
                PairFindingDescriptor {
                    rule: Rule::DuplicateMatcher,
                    left_index,
                    right_index,
                    left,
                    right,
                    message: "Two step definitions use the same effective matcher",
                    matcher_similarity,
                    handler_similarity,
                    matcher_difference: Cow::Borrowed("Matchers are textually identical"),
                    handler_evidence: Cow::Owned(handler_evidence(left, right)),
                    suggested_action:
                        "Keep one definition or make the matchers intentionally distinct",
                },
            );
        } else if normalized_finding == Some(Rule::NormalizedMatcher) {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                suppressions,
                PairFindingDescriptor {
                    rule: Rule::NormalizedMatcher,
                    left_index,
                    right_index,
                    left,
                    right,
                    message: "Two matchers are equivalent after normalization",
                    matcher_similarity,
                    handler_similarity,
                    matcher_difference: Cow::Owned(format!(
                        "`{}` normalizes to `{}`",
                        right.matcher, right.normalized_matcher
                    )),
                    handler_evidence: Cow::Owned(handler_evidence(left, right)),
                    suggested_action:
                        "Consolidate the definitions or use clearly distinct matchers",
                },
            );
        }

        if duplicate_finding.is_some() {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                suppressions,
                PairFindingDescriptor {
                    rule: Rule::DuplicateHandler,
                    left_index,
                    right_index,
                    left,
                    right,
                    message: "Different matchers use the same normalized implementation",
                    matcher_similarity,
                    handler_similarity,
                    matcher_difference: Cow::Owned(matcher_difference(left, right)),
                    handler_evidence: Cow::Borrowed(
                        "Handlers are identical after parameter and local-variable normalization",
                    ),
                    suggested_action:
                        "Review whether one parameterized step can replace both definitions",
                },
            );
        }

        if handler_finding == Some(Rule::ParameterizationCandidate) {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                suppressions,
                PairFindingDescriptor {
                    rule: Rule::ParameterizationCandidate,
                    left_index,
                    right_index,
                    left,
                    right,
                    message: "Handler structures differ primarily in literal values",
                    matcher_similarity,
                    handler_similarity,
                    matcher_difference: Cow::Owned(matcher_difference(left, right)),
                    handler_evidence: Cow::Borrowed(
                        "Handlers have the same control flow and calls after literal normalization",
                    ),
                    suggested_action:
                        "Consider replacing the literal differences with a step parameter",
                },
            );
        } else if handler_finding == Some(Rule::NearDuplicateStep) {
            push_pair_finding(
                findings,
                &mut forests,
                config,
                suppressions,
                PairFindingDescriptor {
                    rule: Rule::NearDuplicateStep,
                    left_index,
                    right_index,
                    left,
                    right,
                    message:
                        "Matcher wording is very close and handler behavior substantially overlaps",
                    matcher_similarity,
                    handler_similarity,
                    matcher_difference: Cow::Owned(matcher_difference(left, right)),
                    handler_evidence: if same_structure {
                        Cow::Owned(handler_evidence(left, right))
                    } else {
                        Cow::Owned(format!(
                            "Handlers share {:.1}% ordered behavior",
                            handler_similarity * 100.0
                        ))
                    },
                    suggested_action:
                        "Check for wording drift and consolidate if the steps express one behavior",
                },
            );
        }
        evaluated += 1;
    }
    if evaluated < generated.candidates.len() {
        generated.mark_verification_truncated(evaluated);
    }
    collapse_large_pair_findings(findings, first_pair_finding);
    let incomplete = generated.incompleteness(config);
    PairAnalysis {
        census: generated.census,
        incomplete,
    }
}

fn pair_similarity_work(input: &PairWorkInput<'_>, work: PairSimilarityWork) -> u64 {
    match work {
        PairSimilarityWork::Matcher if !input.relationships.normalized_matcher => {
            matrix_work(input.left_matcher_length, input.right_matcher_length)
        }
        PairSimilarityWork::Handler
            if !input.relationships.same_handler && !input.relationships.same_handler_structure =>
        {
            matrix_work(
                input.left.handler.behavior_signature.len(),
                input.right.handler.behavior_signature.len(),
            )
        }
        PairSimilarityWork::Evidence => input
            .left_comparison_bytes
            .saturating_add(input.right_comparison_bytes)
            .saturating_mul(PAIR_LINEAR_SCAN_MULTIPLIER),
        _ => 0,
    }
}

fn candidate_suppression_work(
    rules: &[Option<Rule>],
    candidate: &CandidatePair,
    suppressions: &SuppressionIndex<'_>,
) -> u64 {
    let indices = [candidate.left, candidate.right];
    rules.iter().flatten().fold(0_u64, |work, &rule| {
        work.saturating_add(suppressions.lookup_work(rule, &indices))
    })
}

fn charge_similarity_work(total: &mut u64, work: u64) -> bool {
    charge_work(total, work, MAX_TOTAL_PAIR_SIMILARITY_WORK)
}

fn charge_work(total: &mut u64, work: u64, limit: u64) -> bool {
    let Some(next) = total.checked_add(work) else {
        return false;
    };
    if next > limit {
        return false;
    }
    *total = next;
    true
}

fn charge_matrix_work(total: &mut u64, work: u64) -> bool {
    work <= MAX_PAIR_MATRIX_WORK && charge_similarity_work(total, work)
}

fn comparison_classes(definitions: &[StepDefinition]) -> ComparisonClasses {
    let mut exact_matchers = HashMap::new();
    let mut normalized_matchers = HashMap::new();
    let mut alpha_handlers = HashMap::new();
    let mut structural_handlers = HashMap::new();
    let mut structures_and_behavior = HashMap::new();
    let mut classes = ComparisonClasses {
        exact_matcher: Vec::with_capacity(definitions.len()),
        normalized_matcher: Vec::with_capacity(definitions.len()),
        alpha_handler: Vec::with_capacity(definitions.len()),
        structural_handler: Vec::with_capacity(definitions.len()),
        structure_and_behavior: Vec::with_capacity(definitions.len()),
    };
    for definition in definitions {
        classes.exact_matcher.push(intern_class(
            &mut exact_matchers,
            (
                definition.matcher_kind,
                definition.matcher.as_str(),
                definition.matcher_flags.as_str(),
            ),
        ));
        classes.normalized_matcher.push(intern_class(
            &mut normalized_matchers,
            (
                definition.matcher_kind,
                definition.normalized_matcher.as_str(),
            ),
        ));
        classes.alpha_handler.push(intern_class(
            &mut alpha_handlers,
            definition.handler.alpha_normalized.as_str(),
        ));
        classes.structural_handler.push(intern_class(
            &mut structural_handlers,
            definition.handler.structural.as_str(),
        ));
        classes.structure_and_behavior.push(intern_class(
            &mut structures_and_behavior,
            (
                definition.handler.structural.as_str(),
                definition.handler.behavior_signature.as_slice(),
            ),
        ));
    }
    classes
}

fn intern_class<K: Eq + Hash>(classes: &mut HashMap<K, usize>, key: K) -> usize {
    let next = classes.len();
    *classes.entry(key).or_insert(next)
}

fn behavior_event_ids(definitions: &[StepDefinition]) -> Vec<Vec<usize>> {
    let mut events = HashMap::new();
    definitions
        .iter()
        .map(|definition| {
            definition
                .handler
                .behavior_signature
                .iter()
                .map(|event| intern_class(&mut events, event.as_str()))
                .collect()
        })
        .collect()
}

fn behavior_call_event_ids(definitions: &[StepDefinition]) -> Vec<Vec<usize>> {
    let mut events = HashMap::new();
    definitions
        .iter()
        .map(|definition| {
            let mut calls = definition
                .handler
                .behavior_signature
                .iter()
                .filter(|event| event.starts_with("call:"))
                .map(|event| intern_class(&mut events, event.as_str()))
                .collect::<Vec<_>>();
            calls.sort_unstable();
            calls.dedup();
            calls
        })
        .collect()
}

fn definition_comparison_bytes(definition: &StepDefinition) -> u64 {
    let fixed = [
        definition.matcher.len(),
        definition.normalized_matcher.len(),
        definition.matcher_flags.len(),
        definition.handler.exact.len(),
        definition.handler.normalized.len(),
        definition.handler.alpha_normalized.len(),
        definition.handler.structural.len(),
        definition.handler.source_snippet.len(),
    ]
    .into_iter()
    .fold(0_usize, usize::saturating_add);
    let events = definition.handler.behavior_signature.iter().fold(
        definition.handler.behavior_signature.len(),
        |total, event| total.saturating_add(event.len()),
    );
    u64::try_from(fixed.saturating_add(events)).unwrap_or(u64::MAX)
}

fn matrix_work(left: usize, right: usize) -> u64 {
    let left = u64::try_from(left).unwrap_or(u64::MAX);
    let right = u64::try_from(right).unwrap_or(u64::MAX);
    left.saturating_mul(right)
        .saturating_add(left)
        .saturating_add(right)
}

pub(super) fn definition_pair_candidates(
    definitions: &[StepDefinition],
    config: &Config,
) -> CandidateGeneration {
    let comparison_classes = comparison_classes(definitions);
    let behavior_events = behavior_event_ids(definitions);
    let behavior_call_events = behavior_call_event_ids(definitions);
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
            insert_spanning(
                exact_group,
                CandidateSource::NormalizedMatcher,
                &mut builder,
            );
        }
        insert_multipartite_spanning(
            &exact_groups,
            CandidateSource::NormalizedMatcher,
            &mut builder,
        );
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
            CandidateSource::IdenticalHandler,
            &mut builder,
        );
    }

    let mut structural_classes = Vec::new();
    let mut structural_class_by_definition = vec![None; definitions.len()];
    let mut handler_group_by_definition = vec![0_usize; definitions.len()];
    let mut matcher_group_by_definition = vec![0_usize; definitions.len()];
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
        let mut matcher_groups: HashMap<(MatcherKind, &str), Vec<usize>> = HashMap::new();
        for &index in &group {
            let definition = &definitions[index];
            matcher_groups
                .entry((definition.matcher_kind, &definition.normalized_matcher))
                .or_default()
                .push(index);
        }
        let matcher_groups = sorted_groups(matcher_groups);
        for (matcher_group, indexes) in matcher_groups.iter().enumerate() {
            for &index in indexes {
                matcher_group_by_definition[index] = matcher_group;
            }
        }
        let same_handler_proposals = handler_groups.iter().fold(0_u64, |total, indexes| {
            let mut matcher_counts = HashMap::new();
            for &index in indexes {
                *matcher_counts
                    .entry(matcher_group_by_definition[index])
                    .or_insert(0_u64) += 1;
            }
            total.saturating_add(multipartite_count_sizes(matcher_counts.into_values()))
        });
        structural_classes.push(StructuralClass {
            anchor: definitions[group[0]].location.clone(),
            unique_proposals: multipartite_pair_count(&matcher_groups)
                .saturating_sub(same_handler_proposals),
            matcher_groups,
        });
    }
    for &(left, right) in &builder.proposals {
        let Some(class_index) = structural_class_by_definition[left] else {
            continue;
        };
        if structural_class_by_definition[right] == Some(class_index)
            && handler_group_by_definition[left] != handler_group_by_definition[right]
            && matcher_group_by_definition[left] != matcher_group_by_definition[right]
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
        'groups: for left_group in 0..structural_class.matcher_groups.len() {
            for right_group in left_group + 1..structural_class.matcher_groups.len() {
                for left in &structural_class.matcher_groups[left_group] {
                    for right in &structural_class.matcher_groups[right_group] {
                        if handler_group_by_definition[*left] == handler_group_by_definition[*right]
                        {
                            continue;
                        }
                        if inserted >= per_class_limit {
                            let remaining =
                                structural_class.unique_proposals.saturating_sub(inserted);
                            if remaining > 0 {
                                builder.skip(CandidateSource::StructuralHandler, remaining);
                                truncated_structural_classes.push(structural_class.anchor.clone());
                            }
                            break 'groups;
                        }
                        match builder.insert(CandidateSource::StructuralHandler, *left, *right) {
                            InsertOutcome::Inserted => inserted = inserted.saturating_add(1),
                            InsertOutcome::Existing => {}
                            InsertOutcome::Limit => {
                                builder.skip(
                                    CandidateSource::StructuralHandler,
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

    insert_matcher_blocking_candidates(
        definitions,
        &comparison_classes,
        &behavior_events,
        &behavior_call_events,
        &mut builder,
    );
    let census = builder.census(truncated_structural_classes.len());
    CandidateGeneration {
        candidates: builder.candidates,
        census,
        truncated_structural_classes,
        comparison_classes,
        behavior_events,
    }
}

struct StructuralClass {
    anchor: SourceLocation,
    unique_proposals: u64,
    matcher_groups: Vec<Vec<usize>>,
}

fn sorted_groups<K>(groups: HashMap<K, Vec<usize>>) -> Vec<Vec<usize>> {
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by_key(|group| group.first().copied().unwrap_or(usize::MAX));
    groups
}

fn insert_spanning(group: &[usize], source: CandidateSource, builder: &mut CandidateBuilder) {
    for pair in group.windows(2) {
        builder.insert(source, pair[0], pair[1]);
    }
}

fn insert_multipartite_spanning(
    groups: &[Vec<usize>],
    source: CandidateSource,
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

fn insert_matcher_blocking_candidates(
    definitions: &[StepDefinition],
    classes: &ComparisonClasses,
    behavior_events: &[Vec<usize>],
    behavior_call_events: &[Vec<usize>],
    builder: &mut CandidateBuilder,
) {
    let mut shingle_count = 0_usize;
    let mut shingles = Vec::with_capacity(definitions.len());
    for definition in definitions {
        if !meaningful_handler(definition) {
            shingles.push(Vec::new());
            continue;
        }
        let Some(definition_shingles) = matcher_shingle_keys(&definition.normalized_matcher) else {
            builder.skip(CandidateSource::MatcherBlocking, 1);
            return;
        };
        let Some(next_count) = shingle_count.checked_add(definition_shingles.len()) else {
            builder.skip(CandidateSource::MatcherBlocking, 1);
            return;
        };
        if next_count > MAX_MATCHER_SHINGLES_TOTAL {
            builder.skip(CandidateSource::MatcherBlocking, 1);
            return;
        }
        shingle_count = next_count;
        shingles.push(definition_shingles);
    }

    let sorted_events = behavior_events
        .iter()
        .map(|events| {
            let mut events = events.clone();
            events.sort_unstable();
            events
        })
        .collect::<Vec<_>>();
    let mut postings: HashMap<(MatcherKind, u64), MatcherPosting> = HashMap::new();
    for (index, definition) in definitions.iter().enumerate() {
        if !meaningful_handler(definition) {
            continue;
        }
        for &shingle in &shingles[index] {
            postings
                .entry((definition.matcher_kind, shingle))
                .or_default()
                .insert(index);
        }
    }

    let mut ordered_postings = postings.into_iter().collect::<Vec<_>>();
    ordered_postings.sort_by_key(|(key, _)| *key);
    let mut proposal_work = 0_u64;
    let mut event_work = 0_u64;
    let mut considered_pairs = HashSet::new();
    let mut saturated_postings = Vec::new();
    for (_, posting) in &ordered_postings {
        if posting.indices.len() > MAX_MATCHER_BLOCKING_POSTING {
            saturated_postings.push(posting.indices.clone());
            continue;
        }
        if posting.indices.len() < 2 {
            continue;
        }
        for left_offset in 0..posting.indices.len() {
            for right_offset in left_offset + 1..posting.indices.len() {
                if !consider_matcher_blocking_pair(
                    classes,
                    &sorted_events,
                    behavior_call_events,
                    posting.indices[left_offset],
                    posting.indices[right_offset],
                    &mut considered_pairs,
                    &mut proposal_work,
                    &mut event_work,
                    builder,
                ) {
                    return;
                }
            }
        }
    }

    // A high-frequency shingle cannot be expanded without quadratic work. Keep each saturated
    // posting independent so unrelated shingle groups cannot interleave and displace its useful
    // neighbors. Complete-matcher ordering yields a deterministic linear sample per posting;
    // `considered_pairs` deduplicates overlapping postings and the shared work limits remain final.
    for mut lexical in saturated_postings {
        lexical.sort_by(|&left, &right| {
            let left = &definitions[left];
            let right = &definitions[right];
            left.matcher_kind
                .cmp(&right.matcher_kind)
                .then(left.normalized_matcher.cmp(&right.normalized_matcher))
                .then(left.location.path.cmp(&right.location.path))
                .then(left.location.line.cmp(&right.location.line))
                .then(left.location.column.cmp(&right.location.column))
        });
        for pair in lexical.windows(2) {
            if definitions[pair[0]].matcher_kind != definitions[pair[1]].matcher_kind {
                continue;
            }
            if !consider_matcher_blocking_pair(
                classes,
                &sorted_events,
                behavior_call_events,
                pair[0],
                pair[1],
                &mut considered_pairs,
                &mut proposal_work,
                &mut event_work,
                builder,
            ) {
                return;
            }
        }
    }
}

#[derive(Default)]
struct MatcherPosting {
    indices: Vec<usize>,
}

impl MatcherPosting {
    fn insert(&mut self, index: usize) {
        self.indices.push(index);
    }
}

fn matcher_shingle_keys(matcher: &str) -> Option<Vec<u64>> {
    let mut window = ['\0'; MATCHER_SHINGLE_WIDTH];
    let mut length = 0_usize;
    let mut digit_run = false;
    let mut keys = HashSet::new();
    for character in matcher.chars().flat_map(char::to_lowercase) {
        if character.is_numeric() {
            if digit_run {
                continue;
            }
            digit_run = true;
            push_matcher_character('0', &mut window, &mut length, &mut keys);
        } else {
            digit_run = false;
            push_matcher_character(character, &mut window, &mut length, &mut keys);
        }
        if keys.len() > MAX_MATCHER_SHINGLES_PER_DEFINITION {
            return None;
        }
    }
    if length > 0 && length < MATCHER_SHINGLE_WIDTH {
        keys.insert(encode_matcher_characters(&window[..length]));
    }
    let mut keys = keys.into_iter().collect::<Vec<_>>();
    keys.sort_unstable();
    Some(keys)
}

fn push_matcher_character(
    character: char,
    window: &mut [char; MATCHER_SHINGLE_WIDTH],
    length: &mut usize,
    keys: &mut HashSet<u64>,
) {
    if *length < MATCHER_SHINGLE_WIDTH {
        window[*length] = character;
        *length += 1;
    } else {
        window.rotate_left(1);
        window[MATCHER_SHINGLE_WIDTH - 1] = character;
    }
    if *length == MATCHER_SHINGLE_WIDTH {
        keys.insert(encode_matcher_characters(window));
    }
}

fn encode_matcher_characters(characters: &[char]) -> u64 {
    characters
        .iter()
        .enumerate()
        .fold(0_u64, |encoded, (index, character)| {
            encoded | ((u64::from(u32::from(*character)) + 1) << (index * 21))
        })
}

fn meaningful_handler(definition: &StepDefinition) -> bool {
    definition.handler.comparable && !definition.handler.trivial
}

fn covered_by_structural_source(relationships: PairRelationships) -> bool {
    !relationships.same_handler && relationships.same_structure
}

fn can_reach_handler_similarity_gate(
    relationships: PairRelationships,
    left_events: &[usize],
    right_events: &[usize],
    left_call_events: &[usize],
    right_call_events: &[usize],
) -> bool {
    if relationships.same_handler_structure {
        return true;
    }
    let longest = left_events.len().max(right_events.len());
    if longest == 0 {
        return false;
    }
    if !sorted_events_overlap(left_call_events, right_call_events) {
        return false;
    }
    let mut overlap = 0_usize;
    let mut left_events = left_events.iter().peekable();
    let mut right_events = right_events.iter().peekable();
    while let (Some(left), Some(right)) = (left_events.peek(), right_events.peek()) {
        match left.cmp(right) {
            std::cmp::Ordering::Less => {
                left_events.next();
            }
            std::cmp::Ordering::Greater => {
                right_events.next();
            }
            std::cmp::Ordering::Equal => {
                left_events.next();
                right_events.next();
                overlap += 1;
            }
        }
    }
    overlap as f64 / longest as f64 >= HANDLER_SIMILARITY_GATE
}

fn sorted_events_overlap(left: &[usize], right: &[usize]) -> bool {
    let mut left = left.iter().peekable();
    let mut right = right.iter().peekable();
    while let (Some(left_event), Some(right_event)) = (left.peek(), right.peek()) {
        match left_event.cmp(right_event) {
            std::cmp::Ordering::Less => {
                left.next();
            }
            std::cmp::Ordering::Greater => {
                right.next();
            }
            std::cmp::Ordering::Equal => return true,
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn consider_matcher_blocking_pair(
    classes: &ComparisonClasses,
    sorted_events: &[Vec<usize>],
    call_events: &[Vec<usize>],
    left: usize,
    right: usize,
    considered_pairs: &mut HashSet<(usize, usize)>,
    proposal_work: &mut u64,
    event_work: &mut u64,
    builder: &mut CandidateBuilder,
) -> bool {
    let pair = candidate_pair(left, right);
    let relationships = classes.relationships(left, right);
    if relationships.normalized_matcher
        || covered_by_structural_source(relationships)
        || builder.candidates.contains_key(&pair)
        || !considered_pairs.insert(pair)
    {
        return true;
    }
    *proposal_work = proposal_work.saturating_add(1);
    if *proposal_work > MAX_MATCHER_BLOCKING_PROPOSAL_WORK {
        builder.skip(CandidateSource::MatcherBlocking, 1);
        return false;
    }
    try_insert_matcher_blocking_candidate(
        relationships,
        sorted_events,
        call_events,
        left,
        right,
        event_work,
        builder,
    )
}

fn try_insert_matcher_blocking_candidate(
    relationships: PairRelationships,
    sorted_events: &[Vec<usize>],
    call_events: &[Vec<usize>],
    left: usize,
    right: usize,
    event_work: &mut u64,
    builder: &mut CandidateBuilder,
) -> bool {
    if builder.candidates.len() >= builder.limit {
        builder.skip(CandidateSource::MatcherBlocking, 1);
        return false;
    }
    let work = u64::try_from(
        sorted_events[left]
            .len()
            .saturating_add(sorted_events[right].len())
            .saturating_add(call_events[left].len())
            .saturating_add(call_events[right].len()),
    )
    .unwrap_or(u64::MAX);
    *event_work = event_work.saturating_add(work);
    if *event_work > MAX_MATCHER_BLOCKING_EVENT_WORK {
        builder.skip(CandidateSource::MatcherBlocking, 1);
        return false;
    }
    if can_reach_handler_similarity_gate(
        relationships,
        &sorted_events[left],
        &sorted_events[right],
        &call_events[left],
        &call_events[right],
    ) && builder.insert(CandidateSource::MatcherBlocking, left, right) == InsertOutcome::Limit
    {
        return false;
    }
    true
}

fn multipartite_pair_count(groups: &[Vec<usize>]) -> u64 {
    multipartite_count_sizes(
        groups
            .iter()
            .map(|group| u64::try_from(group.len()).unwrap_or(u64::MAX)),
    )
}

fn multipartite_count_sizes(sizes: impl IntoIterator<Item = u64>) -> u64 {
    let mut prior = 0_u64;
    let mut total = 0_u64;
    for size in sizes {
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
    candidates: BTreeMap<(usize, usize), CandidatePair>,
    proposals: BTreeSet<(usize, usize)>,
    limit: usize,
    sources: BTreeMap<CandidateSource, CandidateSourceCensus>,
}

impl CandidateBuilder {
    fn new(limit: usize) -> Self {
        Self {
            candidates: BTreeMap::new(),
            proposals: BTreeSet::new(),
            limit,
            sources: CandidateSource::ALL
                .into_iter()
                .map(|source| (source, CandidateSourceCensus::default()))
                .collect(),
        }
    }

    fn insert(&mut self, source: CandidateSource, left: usize, right: usize) -> InsertOutcome {
        let pair = candidate_pair(left, right);
        if !self.proposals.insert(pair) {
            if let Some(candidate) = self.candidates.get_mut(&pair) {
                candidate.sources.insert(source);
            }
            return InsertOutcome::Existing;
        }
        if self.candidates.len() >= self.limit {
            self.skip(source, 1);
            return InsertOutcome::Limit;
        }
        let mut sources = CandidateSources::default();
        sources.insert(source);
        self.candidates.insert(
            pair,
            CandidatePair {
                left: pair.0,
                right: pair.1,
                sources,
                owner: source,
            },
        );
        self.sources.entry(source).or_default().evaluated += 1;
        InsertOutcome::Inserted
    }

    fn skip(&mut self, source: CandidateSource, count: u64) {
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
                .map(|(source, census)| (source.as_str().to_owned(), census.clone()))
                .collect(),
        }
    }
}

fn candidate_pair(left: usize, right: usize) -> (usize, usize) {
    (left.min(right), left.max(right))
}

pub(super) struct CandidateGeneration {
    candidates: BTreeMap<(usize, usize), CandidatePair>,
    pub(super) census: AnalysisCensus,
    truncated_structural_classes: Vec<SourceLocation>,
    comparison_classes: ComparisonClasses,
    behavior_events: Vec<Vec<usize>>,
}

#[cfg(test)]
impl CandidateGeneration {
    pub(super) fn len(&self) -> usize {
        self.candidates.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub(super) fn contains_pair(&self, left: usize, right: usize) -> bool {
        self.candidates.contains_key(&candidate_pair(left, right))
    }
}

impl CandidateGeneration {
    fn mark_verification_truncated(&mut self, evaluated: usize) {
        let skipped = self.candidates.len().saturating_sub(evaluated);
        for candidate in self.candidates.values().skip(evaluated) {
            if let Some(source) = self
                .census
                .candidate_sources
                .get_mut(candidate.owner.as_str())
            {
                source.evaluated = source.evaluated.saturating_sub(1);
                source.skipped = source.skipped.saturating_add(1);
            }
        }
        self.census.candidate_comparisons_evaluated = evaluated;
        self.census.skipped_candidate_comparisons = self
            .census
            .skipped_candidate_comparisons
            .saturating_add(u64::try_from(skipped).unwrap_or(u64::MAX));
        self.census.truncated = true;
    }

    fn incompleteness(&self, config: &Config) -> Option<String> {
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
                "analysis is incomplete: evaluated {} candidate definition comparisons and skipped {} after safety limits{}; partial findings are available",
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

struct PairFindingDescriptor<'a> {
    rule: Rule,
    left_index: usize,
    right_index: usize,
    left: &'a StepDefinition,
    right: &'a StepDefinition,
    message: &'static str,
    matcher_similarity: f64,
    handler_similarity: f64,
    matcher_difference: Cow<'static, str>,
    handler_evidence: Cow<'static, str>,
    suggested_action: &'static str,
}

fn push_pair_finding(
    findings: &mut Vec<Finding>,
    forests: &mut FindingForests,
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    finding: PairFindingDescriptor<'_>,
) {
    let severity = config.severity(finding.rule);
    if severity == Severity::Off {
        return;
    }
    let suppression_reason =
        suppressions.find_reason(finding.rule, &[finding.left_index, finding.right_index]);
    if !forests.retain_edge(
        finding.rule,
        suppression_reason.is_some(),
        finding.left_index,
        finding.right_index,
    ) {
        return;
    }
    let suppression = suppression_reason.map(|reason| Suppression {
        reason: reason.to_owned(),
    });
    findings.push(Finding {
        rule: finding.rule,
        severity,
        message: finding.message.to_owned(),
        primary: finding.left.location.clone(),
        related: vec![finding.right.location.clone()],
        evidence: FindingEvidence {
            matcher_similarity: Some(round_score(finding.matcher_similarity)),
            handler_similarity: Some(round_score(finding.handler_similarity)),
            matcher_difference: finding.matcher_difference.into_owned(),
            handler_evidence: finding.handler_evidence.into_owned(),
            comparison: Some(definition_comparison(finding.left, finding.right)),
            cluster: None,
        },
        suggested_action: finding.suggested_action.to_owned(),
        suppression,
    });
}

fn collapse_large_pair_findings(findings: &mut Vec<Finding>, first_pair_finding: usize) {
    let pair_findings = findings.drain(first_pair_finding..).collect::<Vec<_>>();
    let mut groups: BTreeMap<(Rule, Option<String>), Vec<usize>> = BTreeMap::new();
    for (index, finding) in pair_findings.iter().enumerate() {
        if supports_cluster_reporting(finding.rule)
            && finding.evidence.comparison.is_some()
            && finding.related.len() == 1
        {
            groups
                .entry((
                    finding.rule,
                    finding
                        .suppression
                        .as_ref()
                        .map(|suppression| suppression.reason.clone()),
                ))
                .or_default()
                .push(index);
        }
    }

    let mut collapsed = vec![false; pair_findings.len()];
    let mut cluster_findings = Vec::new();
    for edge_indices in groups.values() {
        collapse_group_components(
            &pair_findings,
            edge_indices,
            &mut collapsed,
            &mut cluster_findings,
        );
    }
    findings.extend(
        pair_findings
            .into_iter()
            .enumerate()
            .filter_map(|(index, finding)| (!collapsed[index]).then_some(finding)),
    );
    findings.extend(cluster_findings);
}

fn supports_cluster_reporting(rule: Rule) -> bool {
    matches!(
        rule,
        Rule::DuplicateMatcher | Rule::NormalizedMatcher | Rule::DuplicateHandler
    )
}

fn collapse_group_components(
    findings: &[Finding],
    edge_indices: &[usize],
    collapsed: &mut [bool],
    clusters: &mut Vec<Finding>,
) {
    let mut location_ids = HashMap::<SourceLocation, usize>::new();
    for &edge_index in edge_indices {
        let finding = &findings[edge_index];
        for location in std::iter::once(&finding.primary).chain(&finding.related) {
            let next_id = location_ids.len();
            location_ids.entry(location.clone()).or_insert(next_id);
        }
    }
    let mut components = DisjointSet::new(location_ids.len());
    for &edge_index in edge_indices {
        let finding = &findings[edge_index];
        components.union(
            location_ids[&finding.primary],
            location_ids[&finding.related[0]],
        );
    }
    let mut edges_by_component = BTreeMap::<usize, Vec<usize>>::new();
    for &edge_index in edge_indices {
        let root = components.find(location_ids[&findings[edge_index].primary]);
        edges_by_component.entry(root).or_default().push(edge_index);
    }
    for component_edges in edges_by_component.values() {
        let cluster = build_cluster_finding(findings, component_edges);
        let Some(cluster) = cluster else {
            continue;
        };
        for &edge_index in component_edges {
            collapsed[edge_index] = true;
        }
        clusters.push(cluster);
    }
}

fn build_cluster_finding(findings: &[Finding], edge_indices: &[usize]) -> Option<Finding> {
    let mut fingerprints = HashMap::<SourceLocation, String>::new();
    for &edge_index in edge_indices {
        let finding = &findings[edge_index];
        let comparison = finding.evidence.comparison.as_ref()?;
        fingerprints
            .entry(finding.primary.clone())
            .or_insert_with(|| comparison.left_fingerprint.clone());
        fingerprints
            .entry(finding.related[0].clone())
            .or_insert_with(|| comparison.right_fingerprint.clone());
    }
    if fingerprints.len() < MIN_CLUSTER_DEFINITIONS {
        return None;
    }
    let mut locations = fingerprints.keys().cloned().collect::<Vec<_>>();
    locations.sort_by(compare_locations);
    let mut definition_fingerprints = fingerprints.into_values().collect::<Vec<_>>();
    definition_fingerprints.sort();

    let representative = &findings[edge_indices[0]];
    let definition_count = locations.len();
    Some(Finding {
        rule: representative.rule,
        severity: representative.severity,
        message: format!(
            "{definition_count} step definitions form a connected `{}` cluster",
            representative.rule
        ),
        primary: locations.remove(0),
        related: locations,
        evidence: FindingEvidence {
            matcher_similarity: None,
            handler_similarity: None,
            matcher_difference: format!(
                "{} pair findings were collapsed into this cluster",
                edge_indices.len()
            ),
            handler_evidence: "Every cluster member is listed as a primary or related location"
                .to_owned(),
            comparison: None,
            cluster: Some(DefinitionCluster {
                member_count: definition_count,
                definition_fingerprints,
                pair_findings_collapsed: edge_indices.len(),
                members_truncated: false,
            }),
        },
        suggested_action: representative.suggested_action.clone(),
        suppression: representative.suppression.clone(),
    })
}

fn compare_locations(left: &SourceLocation, right: &SourceLocation) -> Ordering {
    left.path
        .cmp(&right.path)
        .then(left.line.cmp(&right.line))
        .then(left.column.cmp(&right.column))
        .then(left.end_line.cmp(&right.end_line))
        .then(left.end_column.cmp(&right.end_column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{SourceFile, SourceLanguage};
    use std::path::PathBuf;

    fn definition() -> StepDefinition {
        crate::typescript::extract(
            "Given('abc', () => { open(); fill(); save(); });",
            &SourceFile {
                path: PathBuf::from("steps.ts"),
                language: SourceLanguage::TypeScript,
            },
        )
        .unwrap()
        .remove(0)
    }

    fn pair_finding(
        rule: Rule,
        left: &StepDefinition,
        right: &StepDefinition,
        suppression: Option<&str>,
    ) -> Finding {
        Finding {
            rule,
            severity: Severity::Error,
            message: "pair".to_owned(),
            primary: left.location.clone(),
            related: vec![right.location.clone()],
            evidence: FindingEvidence {
                matcher_similarity: Some(0.75),
                handler_similarity: Some(0.75),
                matcher_difference: "pair matcher evidence".to_owned(),
                handler_evidence: "pair handler evidence".to_owned(),
                comparison: Some(definition_comparison(left, right)),
                cluster: None,
            },
            suggested_action: "review pair".to_owned(),
            suppression: suppression.map(|reason| Suppression {
                reason: reason.to_owned(),
            }),
        }
    }

    fn located_definitions(count: usize) -> Vec<StepDefinition> {
        (0..count)
            .map(|index| {
                let mut definition = definition();
                definition.location.path = PathBuf::from(format!("steps/{index}.ts"));
                definition.matcher = format!("step {index}");
                definition.normalized_matcher = definition.matcher.clone();
                definition
            })
            .collect()
    }

    #[test]
    fn fuzzy_pair_evidence_is_never_collapsed_into_clusters() {
        let definitions = located_definitions(5);
        let mut findings = definitions
            .windows(2)
            .map(|pair| pair_finding(Rule::NearDuplicateStep, &pair[0], &pair[1], None))
            .collect::<Vec<_>>();

        collapse_large_pair_findings(&mut findings, 0);

        assert_eq!(findings.len(), 4);
        assert!(findings.iter().all(|finding| {
            finding.evidence.comparison.is_some() && finding.evidence.cluster.is_none()
        }));
    }

    #[test]
    fn cluster_reporting_does_not_cross_suppression_partitions() {
        let definitions = located_definitions(6);
        let mut findings = vec![pair_finding(
            Rule::DuplicateHandler,
            &definitions[0],
            &definitions[1],
            Some("accepted legacy pair"),
        )];
        findings.extend(
            definitions[1..]
                .windows(2)
                .map(|pair| pair_finding(Rule::DuplicateHandler, &pair[0], &pair[1], None)),
        );

        collapse_large_pair_findings(&mut findings, 0);

        assert_eq!(findings.len(), 2);
        let suppressed = findings
            .iter()
            .find(|finding| finding.suppression.is_some())
            .unwrap();
        assert!(suppressed.evidence.comparison.is_some());
        assert_eq!(suppressed.related.len(), 1);
        let active = findings
            .iter()
            .find(|finding| finding.suppression.is_none())
            .unwrap();
        assert_eq!(active.related.len(), 4);
        assert_eq!(active.evidence.cluster.as_ref().unwrap().member_count, 5);
    }

    fn insert_blocking_candidates(definitions: &[StepDefinition], builder: &mut CandidateBuilder) {
        let classes = comparison_classes(definitions);
        let events = behavior_event_ids(definitions);
        let call_events = behavior_call_event_ids(definitions);
        insert_matcher_blocking_candidates(definitions, &classes, &events, &call_events, builder);
    }

    #[test]
    fn candidate_source_flags_preserve_independent_rule_inputs() {
        let mut sources = CandidateSources::default();
        for source in CandidateSource::ALL {
            assert!(!sources.contains(source));
            sources.insert(source);
            assert!(sources.contains(source));
        }
        assert!(sources.can_feed_near_matcher());
        let mut normalized_only = CandidateSources::default();
        normalized_only.insert(CandidateSource::NormalizedMatcher);
        assert!(!normalized_only.can_feed_near_matcher());
    }

    #[test]
    fn matcher_shingle_keys_are_case_folded_unicode_safe_and_deduplicated() {
        let keys = |values: &[&str]| {
            let mut keys = values
                .iter()
                .map(|value| encode_matcher_characters(&value.chars().collect::<Vec<_>>()))
                .collect::<Vec<_>>();
            keys.sort_unstable();
            keys
        };
        assert_eq!(matcher_shingle_keys(""), Some(Vec::new()));
        assert_eq!(matcher_shingle_keys("AB"), Some(keys(&["ab"])));
        assert_eq!(matcher_shingle_keys("Abca"), Some(keys(&["abc", "bca"])));
        assert_eq!(
            matcher_shingle_keys("再生再生"),
            Some(keys(&["再生再", "生再生"]))
        );
        assert_eq!(matcher_shingle_keys("aaaa"), Some(keys(&["aaa"])));
        assert_eq!(
            matcher_shingle_keys("Step 123"),
            Some(keys(&["ste", "tep", "ep ", "p 0"]))
        );
    }

    #[test]
    fn matcher_shingle_keys_bound_allocation_for_large_inputs() {
        assert_eq!(
            matcher_shingle_keys(&"a".repeat(1_000_000))
                .expect("one repeated shingle")
                .len(),
            1
        );
        let diverse = (0..MAX_MATCHER_SHINGLES_PER_DEFINITION + 1_000)
            .filter_map(|offset| char::from_u32(0x1_000 + u32::try_from(offset).unwrap()))
            .collect::<String>();
        assert!(matcher_shingle_keys(&diverse).is_none());
    }

    #[test]
    fn matcher_blocking_fails_closed_when_a_work_budget_is_exceeded() {
        let mut oversized = definition();
        oversized.normalized_matcher = (0..MAX_MATCHER_SHINGLES_PER_DEFINITION + 1_000)
            .filter_map(|offset| char::from_u32(0x1_000 + u32::try_from(offset).unwrap()))
            .collect();
        let mut builder = CandidateBuilder::new(usize::MAX);
        insert_blocking_candidates(&[oversized], &mut builder);
        assert!(builder.candidates.is_empty());
        assert_eq!(
            builder.sources[&CandidateSource::MatcherBlocking].skipped,
            1
        );

        let left = definition();
        let mut right = definition();
        right.matcher = "abd".to_owned();
        right.normalized_matcher = "abd".to_owned();
        right.handler.alpha_normalized = "different-alpha".to_owned();
        right.handler.structural = "different-structure".to_owned();
        let definitions = [left, right];
        let classes = comparison_classes(&definitions);
        let relationships = classes.relationships(0, 1);
        let mut sorted_events = behavior_event_ids(&definitions);
        sorted_events
            .iter_mut()
            .for_each(|events| events.sort_unstable());
        let call_events = behavior_call_event_ids(&definitions);
        let mut event_work = MAX_MATCHER_BLOCKING_EVENT_WORK;
        let mut builder = CandidateBuilder::new(usize::MAX);
        assert!(!try_insert_matcher_blocking_candidate(
            relationships,
            &sorted_events,
            &call_events,
            0,
            1,
            &mut event_work,
            &mut builder
        ));
        assert_eq!(
            builder.sources[&CandidateSource::MatcherBlocking].skipped,
            1
        );
    }

    #[test]
    fn handler_prefilters_are_safe_upper_bounds_for_the_near_rule() {
        let base = definition();
        assert!(meaningful_handler(&base));
        let mut trivial = base.clone();
        trivial.handler.trivial = true;
        assert!(!meaningful_handler(&trivial));
        let mut unresolved = base.clone();
        unresolved.handler.comparable = false;
        assert!(!meaningful_handler(&unresolved));

        let mut same_structure = base.clone();
        same_structure.handler.alpha_normalized = "different".to_owned();
        let definitions = [base.clone(), same_structure.clone()];
        let classes = comparison_classes(&definitions);
        assert!(covered_by_structural_source(classes.relationships(0, 1)));
        same_structure
            .handler
            .behavior_signature
            .push("notify".to_owned());
        let definitions = [base.clone(), same_structure];
        let classes = comparison_classes(&definitions);
        assert!(!covered_by_structural_source(classes.relationships(0, 1)));
        assert!(!covered_by_structural_source(classes.relationships(0, 0)));

        let cases = [
            (vec!["call:a", "b", "c", "d"], vec!["call:a", "b"], true),
            (
                vec!["call:a", "b", "c", "d", "e"],
                vec!["call:a", "b"],
                false,
            ),
            (
                vec!["call:a", "call:a", "call:a"],
                vec!["call:a", "call:b", "call:b"],
                false,
            ),
            (
                vec!["call:a", "call:b", "call:b"],
                vec!["call:a", "call:a", "call:a"],
                false,
            ),
            (vec!["if_statement"], vec!["if_statement"], false),
            (vec![], vec!["call:a"], false),
        ];
        for (left, right, expected) in cases {
            let mut left_definition = base.clone();
            let mut right_definition = base.clone();
            right_definition.handler.alpha_normalized = "different-alpha".to_owned();
            right_definition.handler.structural = "different-structure".to_owned();
            left_definition.handler.behavior_signature =
                left.into_iter().map(str::to_owned).collect();
            right_definition.handler.behavior_signature =
                right.into_iter().map(str::to_owned).collect();
            let definitions = [left_definition, right_definition];
            let classes = comparison_classes(&definitions);
            let mut events = behavior_event_ids(&definitions);
            events.iter_mut().for_each(|events| events.sort_unstable());
            let call_events = behavior_call_event_ids(&definitions);
            assert_eq!(
                can_reach_handler_similarity_gate(
                    classes.relationships(0, 1),
                    &events[0],
                    &events[1],
                    &call_events[0],
                    &call_events[1],
                ),
                expected
            );
        }

        let empty_events = Vec::<usize>::new();
        let mut structurally_equal = base.clone();
        structurally_equal.handler.alpha_normalized = "different-alpha".to_owned();
        let definitions = [base, structurally_equal];
        let classes = comparison_classes(&definitions);
        assert!(!classes.relationships(0, 1).same_handler);
        assert!(classes.relationships(0, 1).same_handler_structure);
        assert!(can_reach_handler_similarity_gate(
            classes.relationships(0, 1),
            &empty_events,
            &empty_events,
            &empty_events,
            &empty_events,
        ));
    }

    #[test]
    fn behavior_events_are_interned_before_pairwise_sequence_comparison() {
        let prefix = "x".repeat(4_096);
        let mut left = definition();
        let mut right = definition();
        left.handler.behavior_signature = vec![format!("{prefix}a"); 256];
        right.handler.behavior_signature = vec![format!("{prefix}b"); 256];

        let events = behavior_event_ids(&[left, right]);
        assert_eq!(events[0].len(), 256);
        assert_eq!(events[1].len(), 256);
        assert!(events[0].iter().all(|event| *event == events[0][0]));
        assert!(events[1].iter().all(|event| *event == events[1][0]));
        assert_ne!(events[0][0], events[1][0]);
        assert_eq!(
            handler_similarity_with_relationship(false, false, &events[0], &events[1]),
            0.0
        );
    }

    #[test]
    fn matcher_blocking_rejections_do_not_spend_handler_lcs_work() {
        let mut left = definition();
        left.matcher = format!("abc {}", "x".repeat(64));
        left.normalized_matcher = left.matcher.clone();
        left.handler.alpha_normalized = "left-alpha".to_owned();
        left.handler.structural = "left-structure".to_owned();
        left.handler.behavior_signature = vec!["call:shared-event".to_owned(); 10_000];

        let mut right = definition();
        right.matcher = format!("abc {}", "y".repeat(64));
        right.normalized_matcher = right.matcher.clone();
        right.handler.alpha_normalized = "right-alpha".to_owned();
        right.handler.structural = "right-structure".to_owned();
        right.handler.behavior_signature = vec!["call:shared-event".to_owned(); 10_000];

        let definitions = [left, right];
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(directory.path(), Default::default()).unwrap();
        let generated = definition_pair_candidates(&definitions, &config);
        let candidate = generated.candidates.values().next().unwrap();
        assert_eq!(generated.candidates.len(), 1);
        assert_eq!(candidate.owner, CandidateSource::MatcherBlocking);
        assert!(candidate.sources.contains(CandidateSource::MatcherBlocking));
        assert!(!is_near_matcher(
            &definitions[0],
            &definitions[1],
            matcher_similarity(&definitions[0], &definitions[1])
        ));

        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut findings = Vec::new();
        let analysis =
            analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

        assert!(!analysis.census.truncated);
        assert_eq!(analysis.census.candidate_comparisons_evaluated, 1);
        assert_eq!(analysis.census.skipped_candidate_comparisons, 0);
        assert_eq!(
            analysis.census.candidate_sources["matcherBlocking"].evaluated,
            1
        );
        assert!(analysis.incomplete.is_none());
        assert!(findings.is_empty());
    }

    #[test]
    fn matcher_blocking_survivors_retain_computed_similarity_evidence() {
        let mut left = definition();
        left.matcher = "the account record is enabled".to_owned();
        left.normalized_matcher = left.matcher.clone();
        left.handler.alpha_normalized = "left-alpha".to_owned();
        left.handler.structural = "left-structure".to_owned();
        left.handler.behavior_signature = [
            "call:open",
            "call:fill",
            "call:save",
            "call:close",
            "call:archive",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();

        let mut right = definition();
        right.matcher = "the account records are enabled".to_owned();
        right.normalized_matcher = right.matcher.clone();
        right.handler.alpha_normalized = "right-alpha".to_owned();
        right.handler.structural = "right-structure".to_owned();
        right.handler.behavior_signature = ["call:open", "call:fill", "call:save"]
            .into_iter()
            .map(str::to_owned)
            .collect();

        let expected_matcher_similarity = matcher_similarity(&left, &right);
        let definitions = [left, right];
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(directory.path(), Default::default()).unwrap();
        let generated = definition_pair_candidates(&definitions, &config);
        let candidate = generated.candidates.values().next().unwrap();
        assert_eq!(generated.candidates.len(), 1);
        assert_eq!(candidate.owner, CandidateSource::MatcherBlocking);

        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut findings = Vec::new();
        let analysis =
            analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

        assert!(!analysis.census.truncated);
        assert_eq!(analysis.census.candidate_comparisons_evaluated, 1);
        let finding = findings
            .iter()
            .find(|finding| finding.rule == Rule::NearDuplicateStep)
            .expect("near-duplicate finding");
        assert_eq!(
            finding.evidence.matcher_similarity,
            Some(round_score(expected_matcher_similarity))
        );
        assert_eq!(finding.evidence.handler_similarity, Some(0.6));
        assert_eq!(
            finding.evidence.handler_evidence,
            "Handlers share 60.0% ordered behavior"
        );
        assert!(finding.evidence.comparison.is_some());
    }

    #[test]
    fn disabled_source_rules_do_not_leak_placeholder_similarity_evidence() {
        let mut left = definition();
        left.matcher = "the account is enabled".to_owned();
        left.normalized_matcher = left.matcher.clone();
        let mut right = left.clone();
        right.matcher = "the accounts are enabled".to_owned();
        right.normalized_matcher = right.matcher.clone();
        let expected_matcher_similarity = round_score(matcher_similarity(&left, &right));
        let definitions = [left, right];
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::load(directory.path(), Default::default()).unwrap();
        config.rules.insert(Rule::DuplicateHandler, Severity::Off);

        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut findings = Vec::new();
        let analysis =
            analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

        assert!(!analysis.census.truncated);
        assert!(!findings
            .iter()
            .any(|finding| finding.rule == Rule::DuplicateHandler));
        let near = findings
            .iter()
            .find(|finding| finding.rule == Rule::NearDuplicateStep)
            .expect("near-duplicate finding");
        assert_eq!(
            near.evidence.matcher_similarity,
            Some(expected_matcher_similarity)
        );
        assert_eq!(near.evidence.handler_similarity, Some(1.0));
        assert!(near.evidence.comparison.is_some());

        config.rules.insert(Rule::DuplicateHandler, Severity::Error);
        config.rules.insert(Rule::NearDuplicateStep, Severity::Off);
        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut findings = Vec::new();
        let analysis =
            analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

        assert!(!analysis.census.truncated);
        assert!(!findings
            .iter()
            .any(|finding| finding.rule == Rule::NearDuplicateStep));
        let duplicate = findings
            .iter()
            .find(|finding| finding.rule == Rule::DuplicateHandler)
            .expect("duplicate-handler finding");
        assert_eq!(
            duplicate.evidence.matcher_similarity,
            Some(expected_matcher_similarity)
        );
        assert_eq!(duplicate.evidence.handler_similarity, Some(1.0));
        assert!(duplicate.evidence.comparison.is_some());
    }

    #[test]
    fn disabled_parameterization_skips_unreachable_structural_matcher_work() {
        let mut left = definition();
        left.matcher = format!("{}b", "a".repeat(100_000));
        left.normalized_matcher = left.matcher.clone();
        left.handler.alpha_normalized = "left-alpha".to_owned();

        let mut right = left.clone();
        right.matcher = format!("{}c", "a".repeat(100_000));
        right.normalized_matcher = right.matcher.clone();
        right.handler.alpha_normalized = "right-alpha".to_owned();

        let definitions = [left, right];
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::load(directory.path(), Default::default()).unwrap();
        config
            .rules
            .insert(Rule::ParameterizationCandidate, Severity::Off);
        assert_ne!(config.severity(Rule::NearDuplicateStep), Severity::Off);
        assert!(
            matrix_work(
                definitions[0].normalized_matcher.chars().count(),
                definitions[1].normalized_matcher.chars().count(),
            ) > MAX_PAIR_MATRIX_WORK
        );

        let generated = definition_pair_candidates(&definitions, &config);
        let candidate = generated.candidates.values().next().unwrap();
        assert_eq!(generated.candidates.len(), 1);
        assert_eq!(candidate.owner, CandidateSource::StructuralHandler);
        assert!(candidate
            .sources
            .contains(CandidateSource::StructuralHandler));

        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut findings = Vec::new();
        let analysis =
            analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

        assert!(!analysis.census.truncated);
        assert_eq!(analysis.census.candidate_comparisons_evaluated, 1);
        assert_eq!(analysis.census.skipped_candidate_comparisons, 0);
        assert_eq!(
            analysis.census.candidate_sources["structuralHandler"].evaluated,
            1
        );
        assert!(analysis.incomplete.is_none());
        assert!(findings.is_empty());
    }

    #[test]
    fn work_accounting_contract_charges_similarity_stages_and_enforces_boundaries() {
        let mut left = definition();
        let mut right = left.clone();
        left.handler.alpha_normalized = format!("{}a", "x".repeat(10_000));
        right.handler.alpha_normalized = format!("{}b", "x".repeat(10_000));
        left.handler.structural = "shared-structure".repeat(1_000);
        right.handler.structural = left.handler.structural.clone();
        left.handler.behavior_signature = vec!["left".to_owned(); 2];
        right.handler.behavior_signature = vec!["right".to_owned(); 3];
        let pair = [left, right];
        let unrelated = PairRelationships {
            exact_matcher: false,
            normalized_matcher: false,
            same_handler: false,
            same_handler_structure: false,
            same_structure: false,
        };
        let work = |relationships, stage| {
            let input = PairWorkInput {
                left: &pair[0],
                right: &pair[1],
                left_matcher_length: 3,
                right_matcher_length: 5,
                left_comparison_bytes: definition_comparison_bytes(&pair[0]),
                right_comparison_bytes: definition_comparison_bytes(&pair[1]),
                relationships,
            };
            pair_similarity_work(&input, stage)
        };
        assert_eq!(
            work(unrelated, PairSimilarityWork::Matcher),
            matrix_work(3, 5)
        );
        assert_eq!(
            work(unrelated, PairSimilarityWork::Handler),
            matrix_work(2, 3)
        );
        assert!(work(unrelated, PairSimilarityWork::Evidence) > 100_000);
        assert_eq!(
            work(
                PairRelationships {
                    normalized_matcher: true,
                    ..unrelated
                },
                PairSimilarityWork::Matcher,
            ),
            0
        );
        for relationships in [
            PairRelationships {
                same_handler: true,
                ..unrelated
            },
            PairRelationships {
                same_handler_structure: true,
                ..unrelated
            },
        ] {
            assert_eq!(work(relationships, PairSimilarityWork::Handler), 0);
        }

        let mut total = 0;
        assert!(charge_work(&mut total, 5, 5));
        assert_eq!(total, 5);
        assert!(!charge_work(&mut total, 1, 5));
        assert_eq!(total, 5);
        let mut overflow = u64::MAX;
        assert!(!charge_work(&mut overflow, 1, u64::MAX));
        assert_eq!(overflow, u64::MAX);

        let mut similarity = 0;
        assert!(charge_similarity_work(
            &mut similarity,
            MAX_TOTAL_PAIR_SIMILARITY_WORK
        ));
        assert!(!charge_similarity_work(&mut similarity, 1));

        let mut matrix = 0;
        assert!(charge_matrix_work(&mut matrix, MAX_PAIR_MATRIX_WORK));
        assert!(charge_matrix_work(&mut matrix, 1));
        assert_eq!(matrix, MAX_PAIR_MATRIX_WORK + 1);
        let mut exhausted_matrix_total = MAX_TOTAL_PAIR_SIMILARITY_WORK;
        assert!(!charge_matrix_work(&mut exhausted_matrix_total, 1));
        let mut oversized_matrix = 0;
        assert!(!charge_matrix_work(
            &mut oversized_matrix,
            MAX_PAIR_MATRIX_WORK + 1
        ));
        assert_eq!(oversized_matrix, 0);

        let definitions = crate::typescript::extract(
            "Given('same', () => perform(1));\nGiven('same', () => perform(2));\nGiven('same', () => perform(3));",
            &SourceFile {
                path: PathBuf::from("steps.ts"),
                language: SourceLanguage::TypeScript,
            },
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(directory.path(), Default::default()).unwrap();
        let generated = definition_pair_candidates(&definitions, &config);
        assert_eq!(generated.candidates.len(), 2);
        assert!(generated.candidates.values().all(|candidate| candidate
            .sources
            .contains(CandidateSource::NormalizedMatcher)));
        assert!(generated.candidates.values().all(|candidate| !candidate
            .sources
            .contains(CandidateSource::StructuralHandler)));
    }

    #[test]
    fn work_accounting_contract_charges_only_rules_that_will_be_evaluated() {
        let left = definition();
        let mut right = left.clone();
        right.matcher = "different matcher".to_owned();
        right.normalized_matcher = right.matcher.clone();
        right.handler.alpha_normalized = "different-alpha".to_owned();
        let definitions = [left, right];
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::load(directory.path(), Default::default()).unwrap();
        config.suppressions = [
            Rule::ParameterizationCandidate,
            Rule::NearDuplicateStep,
            Rule::NearDuplicateStep,
        ]
        .into_iter()
        .map(|rule| crate::config::SuppressionConfig {
            rule,
            reason: "temporary migration".to_owned(),
            path: Some("**".to_owned()),
            matcher: None,
        })
        .collect();
        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut sources = CandidateSources::default();
        sources.insert(CandidateSource::StructuralHandler);
        let candidate = CandidatePair {
            left: 0,
            right: 1,
            sources,
            owner: CandidateSource::StructuralHandler,
        };

        let parameterization_work =
            suppressions.lookup_work(Rule::ParameterizationCandidate, &[0, 1]);
        let near_work = suppressions.lookup_work(Rule::NearDuplicateStep, &[0, 1]);
        assert_eq!(near_work, parameterization_work * 2);
        assert_eq!(
            candidate_suppression_work(
                &[Some(Rule::ParameterizationCandidate)],
                &candidate,
                &suppressions,
            ),
            parameterization_work
        );
        assert_eq!(
            candidate_suppression_work(&[Some(Rule::NearDuplicateStep)], &candidate, &suppressions,),
            near_work
        );
        assert_eq!(
            candidate_suppression_work(
                &[
                    Some(Rule::ParameterizationCandidate),
                    None,
                    Some(Rule::NearDuplicateStep),
                ],
                &candidate,
                &suppressions,
            ),
            parameterization_work + near_work
        );
        assert_eq!(
            candidate_suppression_work(&[], &candidate, &suppressions),
            0
        );

        config.suppressions = vec![crate::config::SuppressionConfig {
            rule: Rule::DuplicateHandler,
            reason: "temporary migration".to_owned(),
            path: Some("**".to_owned()),
            matcher: None,
        }];
        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut identical_sources = CandidateSources::default();
        identical_sources.insert(CandidateSource::IdenticalHandler);
        let identical_candidate = CandidatePair {
            sources: identical_sources,
            owner: CandidateSource::IdenticalHandler,
            ..candidate
        };
        let duplicate_work = suppressions.lookup_work(Rule::DuplicateHandler, &[0, 1]);
        assert_eq!(
            candidate_suppression_work(
                &[Some(Rule::DuplicateHandler)],
                &identical_candidate,
                &suppressions,
            ),
            duplicate_work
        );
    }

    #[test]
    fn saturated_matcher_postings_use_a_linear_lexical_fallback() {
        let base = definition();
        let definitions = (0..=MAX_MATCHER_BLOCKING_POSTING)
            .map(|index| {
                let mut definition = base.clone();
                definition.matcher = format!("the account record {index:03} is enabled");
                definition.normalized_matcher = definition.matcher.clone();
                definition.handler.alpha_normalized = format!("alpha-{index}");
                definition.handler.structural = format!("structure-{index}");
                definition.handler.behavior_signature = vec![
                    "call:open".to_owned(),
                    "call:fill".to_owned(),
                    format!("call:specific-{index}"),
                ];
                definition
            })
            .collect::<Vec<_>>();
        let mut builder = CandidateBuilder::new(usize::MAX);
        insert_blocking_candidates(&definitions, &mut builder);
        assert_eq!(builder.candidates.len(), definitions.len() - 1);
        assert!(builder.candidates.contains_key(&candidate_pair(0, 1)));
        assert_eq!(
            builder.sources[&CandidateSource::MatcherBlocking].skipped,
            0
        );
    }

    #[test]
    fn saturated_matcher_fallback_keeps_interleaved_postings_independent() {
        let base = definition();
        let definitions = (0..=MAX_MATCHER_BLOCKING_POSTING)
            .flat_map(|index| {
                ["aaa", "bbb"].into_iter().enumerate().map({
                    let base = base.clone();
                    move |(group, shingle)| {
                        let mut definition = base.clone();
                        definition.matcher = format!("{index:03} {shingle} shared");
                        definition.normalized_matcher = definition.matcher.clone();
                        definition.handler.alpha_normalized = format!("alpha-{group}-{index}");
                        definition.handler.structural = format!("structure-{group}-{index}");
                        definition.handler.behavior_signature = vec![
                            format!("call:group-{group}"),
                            "call:open".to_owned(),
                            "call:fill".to_owned(),
                            "call:save".to_owned(),
                        ];
                        definition
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut builder = CandidateBuilder::new(usize::MAX);
        insert_blocking_candidates(&definitions, &mut builder);

        assert!(builder.candidates.contains_key(&candidate_pair(0, 2)));
        assert!(builder.candidates.contains_key(&candidate_pair(1, 3)));
        assert_eq!(
            builder.sources[&CandidateSource::MatcherBlocking].skipped,
            0
        );
    }

    #[test]
    fn repeated_posting_pairs_are_charged_once_and_fallback_requires_saturation() {
        let base = definition();
        let shared_prefix = (0..80)
            .filter_map(|offset| char::from_u32(0x1_000 + offset))
            .collect::<String>();
        let definitions = (0..MAX_MATCHER_BLOCKING_POSTING)
            .map(|index| {
                let mut definition = base.clone();
                definition.matcher = format!(
                    "{shared_prefix}{}",
                    char::from_u32(0x2_000 + u32::try_from(index).unwrap()).unwrap()
                );
                definition.normalized_matcher = definition.matcher.clone();
                definition.handler.alpha_normalized = format!("alpha-{index}");
                definition.handler.structural = format!("structure-{index}");
                definition.handler.behavior_signature = vec![format!("event-{index}")];
                definition
            })
            .collect::<Vec<_>>();
        let mut builder = CandidateBuilder::new(usize::MAX);
        insert_blocking_candidates(&definitions, &mut builder);
        assert!(builder.candidates.is_empty());
        assert_eq!(
            builder.sources[&CandidateSource::MatcherBlocking].skipped,
            0
        );

        let mut left = base.clone();
        left.matcher = "ab".to_owned();
        left.normalized_matcher = left.matcher.clone();
        let mut right = base;
        right.matcher = "ac".to_owned();
        right.normalized_matcher = right.matcher.clone();
        let mut builder = CandidateBuilder::new(usize::MAX);
        insert_blocking_candidates(&[left, right], &mut builder);
        assert!(builder.candidates.is_empty());
    }
}
