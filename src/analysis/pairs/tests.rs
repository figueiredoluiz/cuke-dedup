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
    let anchor_events = behavior_anchor_event_ids(definitions);
    insert_matcher_blocking_candidates(definitions, &classes, &events, &anchor_events, builder);
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
    let anchor_events = behavior_anchor_event_ids(&definitions);
    let mut event_work = MAX_MATCHER_BLOCKING_EVENT_WORK;
    let mut builder = CandidateBuilder::new(usize::MAX);
    assert!(!try_insert_matcher_blocking_candidate(
        relationships,
        BehaviorEventViews {
            ordered: &behavior_event_ids(&definitions),
            sorted: &sorted_events,
            anchors: &anchor_events,
        },
        0,
        1,
        &mut event_work,
        &mut builder
    ));
    assert_eq!(
        builder.sources[&CandidateSource::MatcherBlocking].skipped,
        1
    );

    let mut left = definition();
    left.handler.behavior_signature = vec!["method:instance async".to_owned()];
    let mut right = definition();
    right.matcher = "abd".to_owned();
    right.normalized_matcher = "abd".to_owned();
    right.handler.alpha_normalized = "different-alpha".to_owned();
    right.handler.structural = "different-structure".to_owned();
    right.handler.behavior_signature = vec!["method:instance sync".to_owned()];
    let definitions = [left, right];
    let classes = comparison_classes(&definitions);
    let events = behavior_event_ids(&definitions);
    let mut sorted_events = events.clone();
    sorted_events
        .iter_mut()
        .for_each(|events| events.sort_unstable());
    let anchor_events = behavior_anchor_event_ids(&definitions);
    let mut considered = HashSet::new();
    let mut proposal_work = MAX_MATCHER_BLOCKING_PROPOSAL_WORK;
    let mut event_work = 0;
    let mut builder = CandidateBuilder::new(usize::MAX);
    assert!(!consider_matcher_blocking_pair(
        &definitions,
        &classes,
        &events,
        &sorted_events,
        &anchor_events,
        0,
        1,
        &mut considered,
        &mut proposal_work,
        &mut event_work,
        &mut builder,
    ));
    assert_eq!(proposal_work, MAX_MATCHER_BLOCKING_PROPOSAL_WORK + 1);
    assert_eq!(event_work, 0);
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
        left_definition.handler.behavior_signature = left.into_iter().map(str::to_owned).collect();
        right_definition.handler.behavior_signature =
            right.into_iter().map(str::to_owned).collect();
        let definitions = [left_definition, right_definition];
        let classes = comparison_classes(&definitions);
        let events = behavior_event_ids(&definitions);
        let mut sorted_events = events.clone();
        sorted_events
            .iter_mut()
            .for_each(|events| events.sort_unstable());
        let anchor_events = behavior_anchor_event_ids(&definitions);
        assert_eq!(
            can_reach_handler_similarity_gate(
                classes.relationships(0, 1),
                &events[0],
                &events[1],
                &sorted_events[0],
                &sorted_events[1],
                &anchor_events[0],
                &anchor_events[1],
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
    // Equal AST shapes cannot bypass conflicting assertion behavior before insertion.
    assert!(!can_reach_handler_similarity_gate(
        classes.relationships(0, 1),
        &[1],
        &[2],
        &[1],
        &[2],
        &[1],
        &[2],
    ));
    // Reordered events cannot use the exact-sequence shortcut, but the multiset overlap remains
    // a safe upper bound: final ordered similarity decides whether the candidate is retained.
    assert!(can_reach_handler_similarity_gate(
        classes.relationships(0, 1),
        &[1, 2, 3],
        &[3, 2, 1],
        &[1, 2, 3],
        &[1, 2, 3],
        &[1, 2, 3],
        &[1, 2, 3],
    ));
    assert!(can_reach_handler_similarity_gate(
        classes.relationships(0, 1),
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        &[1, 2, 3, 4, 5, 6, 7, 8, 9],
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        &[1, 2, 3, 4, 5, 6, 7, 8, 9],
        &[1],
        &[1],
    ));
    assert!(can_reach_handler_similarity_gate(
        classes.relationships(0, 1),
        &empty_events,
        &empty_events,
        &empty_events,
        &empty_events,
        &empty_events,
        &empty_events,
    ));

    let mut sync_method = definitions[0].clone();
    sync_method.matcher = "the account status is ready".to_owned();
    sync_method.normalized_matcher = sync_method.matcher.clone();
    sync_method.handler.alpha_normalized = "sync-alpha".to_owned();
    sync_method.handler.structural = "sync-structure".to_owned();
    sync_method.handler.behavior_signature = vec![
        "method:instance sync".to_owned(),
        "assert:expect#toBe:subject:value".to_owned(),
    ];
    let mut async_method = sync_method.clone();
    async_method.matcher = "the account status is steady".to_owned();
    async_method.normalized_matcher = async_method.matcher.clone();
    async_method.handler.alpha_normalized = "async-alpha".to_owned();
    async_method.handler.structural = "async-structure".to_owned();
    async_method.handler.behavior_signature[0] = "method:instance async".to_owned();
    let definitions = [sync_method, async_method];
    let mut builder = CandidateBuilder::new(usize::MAX);
    insert_blocking_candidates(&definitions, &mut builder);
    assert!(builder.candidates.is_empty());
    assert_eq!(
        builder.sources[&CandidateSource::MatcherBlocking].evaluated,
        0
    );
    assert_eq!(
        builder.sources[&CandidateSource::MatcherBlocking].skipped,
        0
    );
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
    let analysis = analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

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
    let analysis = analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

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
    let analysis = analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

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
    let analysis = analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

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
    let analysis = analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);

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
        same_deferred_assertions: true,
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
    assert_eq!(
        work(
            PairRelationships {
                same_handler: true,
                ..unrelated
            },
            PairSimilarityWork::Handler
        ),
        0
    );
    assert_eq!(
        work(
            PairRelationships {
                same_handler_structure: true,
                ..unrelated
            },
            PairSimilarityWork::Handler
        ),
        matrix_work(2, 3)
    );
    assert_eq!(
        work(
            PairRelationships {
                same_handler_structure: true,
                same_structure: true,
                ..unrelated
            },
            PairSimilarityWork::Handler
        ),
        0
    );

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

    let parameterization_work = suppressions.lookup_work(Rule::ParameterizationCandidate, &[0, 1]);
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
