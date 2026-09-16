use super::evidence::{definition_comparison, handler_evidence, matcher_difference};
use super::similarity::{
    handler_runtime_compatible, handler_similarity_with_relationship, is_near_matcher,
    matcher_similarity, round_score,
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
    same_deferred_assertions: bool,
}

struct ComparisonClasses {
    exact_matcher: Vec<usize>,
    normalized_matcher: Vec<usize>,
    alpha_handler: Vec<usize>,
    structural_handler: Vec<usize>,
    structure_and_behavior: Vec<usize>,
    deferred_assertions: Vec<usize>,
}

impl ComparisonClasses {
    fn relationships(&self, left: usize, right: usize) -> PairRelationships {
        PairRelationships {
            exact_matcher: self.exact_matcher[left] == self.exact_matcher[right],
            normalized_matcher: self.normalized_matcher[left] == self.normalized_matcher[right],
            same_handler: self.alpha_handler[left] == self.alpha_handler[right],
            same_handler_structure: self.structural_handler[left] == self.structural_handler[right],
            same_structure: self.structure_and_behavior[left] == self.structure_and_behavior[right],
            same_deferred_assertions: self.deferred_assertions[left]
                == self.deferred_assertions[right],
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
            && relationships.same_deferred_assertions
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
        let handler_similarity = if handler_is_needed && handler_runtime_compatible(left, right) {
            handler_similarity_with_relationship(
                same_handler,
                relationships.same_structure,
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
            if !input.relationships.same_handler && !input.relationships.same_structure =>
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
    let mut deferred_assertions = HashMap::new();
    let mut classes = ComparisonClasses {
        exact_matcher: Vec::with_capacity(definitions.len()),
        normalized_matcher: Vec::with_capacity(definitions.len()),
        alpha_handler: Vec::with_capacity(definitions.len()),
        structural_handler: Vec::with_capacity(definitions.len()),
        structure_and_behavior: Vec::with_capacity(definitions.len()),
        deferred_assertions: Vec::with_capacity(definitions.len()),
    };
    for definition in definitions {
        classes.deferred_assertions.push(intern_class(
            &mut deferred_assertions,
            definition
                .handler
                .behavior_signature
                .iter()
                .filter(|event| event.starts_with("deferred-assert:"))
                .collect::<Vec<_>>(),
        ));
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
            (
                definition.handler.alpha_normalized.as_str(),
                definition.handler.behavior_signature.as_slice(),
            ),
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
                .filter(|event| !event.starts_with("method:"))
                .map(|event| intern_class(&mut events, event.as_str()))
                .collect()
        })
        .collect()
}

fn behavior_anchor_event_ids(definitions: &[StepDefinition]) -> Vec<Vec<usize>> {
    let mut events = HashMap::new();
    definitions
        .iter()
        .map(|definition| {
            let mut anchors = definition
                .handler
                .behavior_signature
                .iter()
                .filter_map(|event| {
                    // Deferred anchors retain their full value and execution-context identity.
                    if event.starts_with("call:") || event.starts_with("deferred-assert:") {
                        Some(Cow::Borrowed(event.as_str()))
                    } else if event.starts_with("assert:") {
                        // Assertion values are semantic during final similarity verification, but
                        // candidate blocking only needs the assertion shape. Removing the final
                        // expected-arguments fingerprint lets potentially related assertions reach
                        // verification without allowing conflicting values to count as overlap.
                        event
                            .rsplit_once(':')
                            .map(|(shape, _)| Cow::Owned(shape.to_owned()))
                    } else {
                        None
                    }
                })
                .map(|event| intern_class(&mut events, event))
                .collect::<Vec<_>>();
            anchors.sort_unstable();
            anchors.dedup();
            anchors
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
    let behavior_anchor_events = behavior_anchor_event_ids(definitions);
    let mut normalized_matchers: HashMap<(MatcherKind, &str), Vec<usize>> = HashMap::new();
    let mut handlers: HashMap<(&str, &[String]), Vec<usize>> = HashMap::new();
    let mut structures: HashMap<(&str, &[String]), Vec<usize>> = HashMap::new();

    for (index, definition) in definitions.iter().enumerate() {
        normalized_matchers
            .entry((definition.matcher_kind, &definition.normalized_matcher))
            .or_default()
            .push(index);
        if definition.handler.comparable && !definition.handler.trivial {
            handlers
                .entry((
                    &definition.handler.alpha_normalized,
                    &definition.handler.behavior_signature,
                ))
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
        &behavior_anchor_events,
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
    behavior_anchor_events: &[Vec<usize>],
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
                    definitions,
                    classes,
                    behavior_events,
                    &sorted_events,
                    behavior_anchor_events,
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
                definitions,
                classes,
                behavior_events,
                &sorted_events,
                behavior_anchor_events,
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
    left_ordered_events: &[usize],
    right_ordered_events: &[usize],
    left_events: &[usize],
    right_events: &[usize],
    left_anchor_events: &[usize],
    right_anchor_events: &[usize],
) -> bool {
    if relationships.same_handler_structure && left_ordered_events == right_ordered_events {
        return true;
    }
    let longest = left_events.len().max(right_events.len());
    if longest == 0 {
        return false;
    }
    if !sorted_events_overlap(left_anchor_events, right_anchor_events) {
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
    definitions: &[StepDefinition],
    classes: &ComparisonClasses,
    behavior_events: &[Vec<usize>],
    sorted_events: &[Vec<usize>],
    anchor_events: &[Vec<usize>],
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
    // Charge every unique enumerated proposal before any semantic rejection. Otherwise a large
    // posting of runtime-incompatible handlers could grow `considered_pairs` quadratically without
    // ever reaching the safety bound.
    *proposal_work = proposal_work.saturating_add(1);
    if *proposal_work > MAX_MATCHER_BLOCKING_PROPOSAL_WORK {
        builder.skip(CandidateSource::MatcherBlocking, 1);
        return false;
    }
    // A shared wrapper call cannot outweigh differing deferred assertions. Interned sequences
    // preserve values, polarity and order, and reject these pairs before charging event work.
    if !relationships.same_deferred_assertions
        || !handler_runtime_compatible(&definitions[left], &definitions[right])
    {
        return true;
    }
    try_insert_matcher_blocking_candidate(
        relationships,
        BehaviorEventViews {
            ordered: behavior_events,
            sorted: sorted_events,
            anchors: anchor_events,
        },
        left,
        right,
        event_work,
        builder,
    )
}

#[derive(Clone, Copy)]
struct BehaviorEventViews<'a> {
    ordered: &'a [Vec<usize>],
    sorted: &'a [Vec<usize>],
    anchors: &'a [Vec<usize>],
}

fn try_insert_matcher_blocking_candidate(
    relationships: PairRelationships,
    events: BehaviorEventViews<'_>,
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
        events.sorted[left]
            .len()
            .saturating_add(events.sorted[right].len())
            .saturating_add(events.anchors[left].len())
            .saturating_add(events.anchors[right].len()),
    )
    .unwrap_or(u64::MAX);
    *event_work = event_work.saturating_add(work);
    if *event_work > MAX_MATCHER_BLOCKING_EVENT_WORK {
        builder.skip(CandidateSource::MatcherBlocking, 1);
        return false;
    }
    if can_reach_handler_similarity_gate(
        relationships,
        &events.ordered[left],
        &events.ordered[right],
        &events.sorted[left],
        &events.sorted[right],
        &events.anchors[left],
        &events.anchors[right],
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
mod tests;
