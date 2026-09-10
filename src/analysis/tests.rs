use super::pairs::{analyze_definition_pairs, definition_pair_candidates};
use super::similarity::{
    handler_similarity, is_near_matcher, matcher_similarity, ordered_common_subsequence_len,
};
use super::*;
use crate::config::{ConfigOverrides, SuppressionConfig};
use crate::discovery::{SourceFile, SourceLanguage};
use crate::gherkin;
use crate::model::Rule;
use crate::typescript;
use std::path::{Path, PathBuf};

fn definitions(source: &str) -> Vec<StepDefinition> {
    typescript::extract(
        source,
        &SourceFile {
            path: PathBuf::from("steps.ts"),
            language: SourceLanguage::TypeScript,
        },
    )
    .unwrap()
}

fn config() -> (tempfile::TempDir, Config) {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    (directory, config)
}

#[test]
fn reports_exact_normalized_and_duplicate_handlers_even_when_unused() {
    let definitions = definitions(
        r#"
Given('a user {string}', async (name) => { await save(name); });
Given('a user {string}', async (name) => { await other(name); });
Given('a  user {string}', async (value) => { await third(value); });
Then('the user exists', async (name) => { await save(name); });
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let rules: Vec<_> = result.findings.iter().map(|finding| finding.rule).collect();
    assert!(rules.contains(&Rule::DuplicateMatcher));
    assert!(rules.contains(&Rule::NormalizedMatcher));
    assert!(rules.contains(&Rule::DuplicateHandler));
}

#[test]
fn candidate_buckets_skip_unrelated_pairs_and_keep_exact_groups() {
    let mut unrelated_source = String::new();
    for index in 0..100 {
        unrelated_source.push_str(&format!(
            "Given('unique step {index}', () => action_{index}());\n"
        ));
    }
    let unrelated = definitions(&unrelated_source);
    let (_directory, config) = config();
    assert!(definition_pair_candidates(&unrelated, &config).is_empty());

    let exact = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());",
    );
    assert_eq!(definition_pair_candidates(&exact, &config).len(), 2);
}

#[test]
fn candidate_limits_preserve_partial_analysis_and_report_skipped_work() {
    let mut source = String::new();
    for index in 0..143 {
        source.push_str(&format!(
            "Given('operation label {index}', () => perform({index}));\n"
        ));
    }
    let class_definitions = definitions(&source);
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 100;
    config.max_structural_class_comparisons = 20;

    let generated = definition_pair_candidates(&class_definitions, &config);
    assert!(generated.census.truncated);
    assert!(generated.len() <= 100);
    assert_eq!(generated.census.truncated_structural_classes, 1);
    assert!(generated.census.skipped_candidate_comparisons > 0);
    assert!(generated.census.candidate_sources["structuralHandler"].skipped > 0);

    let (outcome, census, _) = analyze_for_cli(class_definitions, Vec::new(), &config).unwrap();
    assert!(!outcome.result.findings.is_empty());
    assert_eq!(outcome.operational_errors.len(), 1);
    assert!(outcome.operational_errors[0].contains("partial findings are available"));
    assert_eq!(census, generated.census);
}

#[test]
fn similarity_work_limits_fail_closed_before_quadratic_pair_verification() {
    let mut long_matchers = definitions(
        "Given('left', () => sharedImplementation());\nGiven('right', () => sharedImplementation());",
    );
    long_matchers[0].matcher = format!("{}b", "a".repeat(100_000));
    long_matchers[0].normalized_matcher = long_matchers[0].matcher.clone();
    long_matchers[1].matcher = format!("{}c", "a".repeat(100_000));
    long_matchers[1].normalized_matcher = long_matchers[1].matcher.clone();

    let (_directory, config) = config();
    let mut findings = Vec::new();
    let suppressions = super::suppression::SuppressionIndex::new(&config, &long_matchers);
    let pair_analysis =
        analyze_definition_pairs(&long_matchers, &config, &suppressions, &mut findings);
    assert!(pair_analysis.census.truncated);
    assert_eq!(pair_analysis.census.candidate_comparisons_evaluated, 0);
    assert_eq!(pair_analysis.census.skipped_candidate_comparisons, 1);
    assert_eq!(
        pair_analysis.census.candidate_sources["identicalHandler"].evaluated,
        0
    );
    assert_eq!(
        pair_analysis.census.candidate_sources["identicalHandler"].skipped,
        1
    );
    assert!(pair_analysis
        .operational_error
        .as_deref()
        .is_some_and(|error| error.contains("after safety limits")));
    assert!(!findings.iter().any(|finding| matches!(
        finding.rule,
        Rule::DuplicateHandler | Rule::NearDuplicateStep
    )));

    let mut long_handlers = definitions(
        "Given('the account is enabled', () => first());\nGiven('the accounts are enabled', () => second());",
    );
    for (index, definition) in long_handlers.iter_mut().enumerate() {
        definition.handler.alpha_normalized = format!("alpha-{index}");
        definition.handler.structural = format!("structure-{index}");
        definition.handler.behavior_signature = vec!["shared-event".to_owned(); 10_000];
    }
    let mut findings = Vec::new();
    let suppressions = super::suppression::SuppressionIndex::new(&config, &long_handlers);
    let pair_analysis =
        analyze_definition_pairs(&long_handlers, &config, &suppressions, &mut findings);
    assert!(pair_analysis.census.truncated);
    assert_eq!(pair_analysis.census.candidate_comparisons_evaluated, 0);
    assert_eq!(pair_analysis.census.skipped_candidate_comparisons, 1);
    assert_eq!(
        pair_analysis.census.candidate_sources["matcherBlocking"].evaluated,
        0
    );
    assert_eq!(
        pair_analysis.census.candidate_sources["matcherBlocking"].skipped,
        1
    );
    assert!(pair_analysis.operational_error.is_some());
}

#[test]
fn suppression_globs_compile_once_and_pair_lookups_share_the_work_budget() {
    let base = definitions("Given('base', () => sharedImplementation());").remove(0);
    let (directory, mut config) = config();
    let long_directory = "nested/".repeat(30);
    let definitions = (0..700)
        .map(|index| {
            let mut definition = base.clone();
            definition.matcher = format!("operation label {index:04}");
            definition.normalized_matcher = definition.matcher.clone();
            definition.location.path = directory
                .path()
                .join(format!("{long_directory}steps-{index}.ts"));
            definition.location.line = index + 1;
            definition
        })
        .collect::<Vec<_>>();
    config.suppressions = (0..1_000)
        .map(|index| SuppressionConfig {
            rule: Rule::DuplicateHandler,
            reason: format!("legacy exception {index}"),
            path: Some(format!("missing-{index}/**")),
            matcher: None,
        })
        .collect();

    let suppressions = super::suppression::SuppressionIndex::new(&config, &definitions);
    assert_eq!(suppressions.compiled_path_count(), 1_000);
    for _ in 0..10 {
        assert!(suppressions
            .find_reason(Rule::DuplicateHandler, &[0, 1])
            .is_none());
    }
    assert_eq!(suppressions.compiled_path_count(), 1_000);
    let unmatched = suppressions.unmatched();
    assert!(unmatched.truncated);
    assert!(unmatched.indices.len() < config.suppressions.len());

    let mut findings = Vec::new();
    let pair_analysis =
        analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);
    assert!(pair_analysis.census.truncated);
    assert!(pair_analysis.census.candidate_comparisons_evaluated < definitions.len() - 1);
    assert!(pair_analysis
        .operational_error
        .as_deref()
        .is_some_and(|error| error.contains("after safety limits")));
}

#[test]
fn suppression_lookup_borrows_and_bounds_large_reasons_until_a_finding_is_retained() {
    let definitions = definitions("Given('left', () => work()); Given('right', () => work());");
    let (_directory, mut config) = config();
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "accepted because migration is in progress ".repeat(10_000),
        path: Some("**".to_owned()),
        matcher: None,
    });

    let suppressions = super::suppression::SuppressionIndex::new(&config, &definitions);
    let reason = suppressions
        .find_reason(Rule::DuplicateHandler, &[0, 1])
        .expect("matching suppression");
    assert!(std::ptr::eq(
        reason.as_ptr(),
        config.suppressions[0].reason.as_ptr()
    ));
    assert_eq!(
        reason.chars().count(),
        crate::resource_limits::MAX_SUPPRESSION_REASON_CHARS
    );

    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .expect("duplicate handler finding");
    assert_eq!(
        finding.suppression.as_ref().unwrap().reason.chars().count(),
        crate::resource_limits::MAX_SUPPRESSION_REASON_CHARS
    );
}

#[test]
fn structural_class_limit_preserves_work_from_later_classes() {
    let mut source = String::new();
    for prefix in ["account", "invoice"] {
        for index in 0..6 {
            source.push_str(&format!(
                "Given('{prefix} operation {index}', () => {prefix}Action({index}));\n"
            ));
        }
    }
    let class_definitions = definitions(&source);
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 100;
    config.max_structural_class_comparisons = 2;

    let generated = definition_pair_candidates(&class_definitions, &config);
    assert_eq!(generated.census.truncated_structural_classes, 2);
    assert_eq!(
        generated.census.candidate_sources["structuralHandler"].evaluated,
        4
    );
    assert!((0..6).any(|left| (left + 1..6).any(|right| generated.contains_pair(left, right))));
    assert!((6..12).any(|left| (left + 1..12).any(|right| generated.contains_pair(left, right))));
}

#[test]
fn global_candidate_limit_reports_the_source_that_was_truncated() {
    let definitions = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());\nGiven('same', () => four());\nGiven('same', () => five());",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 2;
    config.max_structural_class_comparisons = 100;

    let generated = definition_pair_candidates(&definitions, &config);
    assert_eq!(generated.len(), 2);
    assert!(generated.census.truncated);
    assert_eq!(
        generated.census.candidate_sources["normalizedMatcher"].skipped,
        2
    );
    assert_eq!(
        generated
            .census
            .candidate_sources
            .values()
            .map(|source| source.evaluated)
            .sum::<usize>(),
        generated.len()
    );
}

#[test]
fn candidate_limits_are_inclusive_at_the_exact_boundary() {
    let structural = definitions(
        "Given('operation one', () => perform(1));\nGiven('operation two', () => perform(2));\nGiven('operation three', () => perform(3));",
    );
    let exact = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 3;
    config.max_structural_class_comparisons = 3;

    let structural = definition_pair_candidates(&structural, &config);
    assert_eq!(structural.len(), 3);
    assert!(!structural.census.truncated);
    assert_eq!(structural.census.skipped_candidate_comparisons, 0);
    assert_eq!(structural.census.truncated_structural_classes, 0);

    config.max_candidate_comparisons = 2;
    let exact = definition_pair_candidates(&exact, &config);
    assert_eq!(exact.len(), 2);
    assert!(!exact.census.truncated);
    assert_eq!(exact.census.skipped_candidate_comparisons, 0);
}

#[test]
fn candidate_limits_do_not_count_structural_pairs_that_cannot_reach_a_rule() {
    let definitions = definitions(
        "Given('same', () => action(1));\nGiven('same', () => action(2));\nGiven('same', () => action(3));",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 1;
    config.max_structural_class_comparisons = 10;

    let generated = definition_pair_candidates(&definitions, &config);
    assert!(generated.census.truncated);
    assert_eq!(generated.len(), 1);
    assert_eq!(generated.census.skipped_candidate_comparisons, 1);
    assert_eq!(
        generated.census.candidate_sources["normalizedMatcher"].skipped,
        1
    );
    assert_eq!(
        generated.census.candidate_sources["structuralHandler"].skipped,
        0
    );
}

#[test]
fn same_normalized_pairs_do_not_consume_the_structural_limit() {
    let definitions = definitions(
        "Given('same', () => action(1));\nGiven('same', () => action(2));\nGiven('same', () => action(3));",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 3;
    config.max_structural_class_comparisons = 1;

    let generated = definition_pair_candidates(&definitions, &config);
    assert_eq!(generated.len(), 2);
    assert!(!generated.census.truncated);
    assert_eq!(generated.census.skipped_candidate_comparisons, 0);
    assert_eq!(generated.census.truncated_structural_classes, 0);
    assert_eq!(
        generated.census.candidate_sources["normalizedMatcher"].evaluated,
        2
    );
    assert_eq!(
        generated.census.candidate_sources["structuralHandler"].evaluated,
        0
    );
}

#[test]
fn same_normalized_class_does_not_report_false_structural_truncation() {
    let definitions = definitions(
        "Given('same', () => action(1));\nGiven('same', () => action(2));\nGiven('same', () => action(3));\nGiven('same', () => action(4));",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 10;
    config.max_structural_class_comparisons = 1;

    let generated = definition_pair_candidates(&definitions, &config);
    assert!(!generated.census.truncated);
    assert_eq!(generated.census.skipped_candidate_comparisons, 0);
    assert_eq!(
        generated.census.candidate_sources["normalizedMatcher"].evaluated,
        3
    );
    assert_eq!(
        generated.census.candidate_sources["structuralHandler"].evaluated,
        0
    );
}

#[test]
fn structural_overlap_ignores_existing_pairs_within_one_handler_group() {
    let definitions = definitions(
        "Given('first', () => action('same'));\nGiven('second', () => action('same'));\nGiven('third', () => action('third'));\nGiven('fourth', () => action('fourth'));",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 10;
    config.max_structural_class_comparisons = 1;

    let generated = definition_pair_candidates(&definitions, &config);
    assert!(generated.census.truncated);
    assert_eq!(generated.census.skipped_candidate_comparisons, 4);
    assert_eq!(
        generated.census.candidate_sources["identicalHandler"].evaluated,
        1
    );
}

#[test]
fn trivial_and_unresolved_handlers_do_not_consume_candidate_budgets() {
    for definitions in [
        definitions("Given('one', () => {});\nGiven('two', () => {});"),
        definitions("Given('one', importedHandler);\nGiven('two', anotherImportedHandler);"),
    ] {
        let (_directory, config) = config();
        let generated = definition_pair_candidates(&definitions, &config);
        assert!(generated.is_empty());
        assert_eq!(generated.census.candidate_comparisons_evaluated, 0);
        assert!(!generated.census.truncated);
    }
}

#[test]
fn candidate_limit_diagnostic_bounds_affected_class_locations() {
    let mut source = String::new();
    for class in 0..5 {
        for index in 0..3 {
            source.push_str(&format!(
                "Given('class {class} operation {index}', () => action{class}({index}));\n"
            ));
        }
    }
    let class_definitions = definitions(&source);
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 100;
    config.max_structural_class_comparisons = 1;

    let (outcome, census, _) = analyze_for_cli(class_definitions, Vec::new(), &config).unwrap();
    assert_eq!(census.truncated_structural_classes, 5);
    assert_eq!(outcome.operational_errors.len(), 1);
    assert!(outcome.operational_errors[0].contains("and 2 more"));

    let definitions = definitions(
        "Given('one', () => action(1));\nGiven('two', () => action(2));\nGiven('three', () => action(3));",
    );
    let (outcome, census, _) = analyze_for_cli(definitions, Vec::new(), &config).unwrap();
    assert_eq!(census.truncated_structural_classes, 1);
    assert!(!outcome.operational_errors[0].contains("and 0 more"));
}

#[test]
fn regex_resource_errors_are_preserved_for_cli_reporting_and_library_callers() {
    let definitions = definitions("Given(/a{1000000}/, () => work());");
    let (_directory, config) = config();

    let error = analyze(definitions.clone(), Vec::new(), &config).unwrap_err();
    assert!(error.to_string().contains("regex resource limit"));

    let outcome = analyze_with_diagnostics(definitions, Vec::new(), &config).unwrap();
    assert_eq!(outcome.result.definitions.len(), 1);
    assert_eq!(outcome.operational_errors.len(), 1);
}

#[test]
fn equivalence_classes_report_a_spanning_set_instead_of_every_pair() {
    let definitions = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());\nGiven('same', () => four());",
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateMatcher)
            .count(),
        3
    );
}

#[test]
fn inline_suppression_applies_to_pair_findings() {
    let definitions = definitions(
        r#"
// cuke-dedup:ignore duplicate-matcher -- intentionally split setup paths
Given('same step', () => first());
Given('same step', () => second());
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let duplicate = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateMatcher)
        .unwrap();
    assert_eq!(
        duplicate
            .suppression
            .as_ref()
            .map(|value| value.reason.as_str()),
        Some("intentionally split setup paths")
    );
}

#[test]
fn wording_drift_with_same_handler_is_near_duplicate() {
    let definitions = definitions(
        r#"
Then('the item is visible', async () => { await expect(item).toBeVisible(); });
Then('the items are visible', async () => { await expect(item).toBeVisible(); });
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn matcher_blocking_finds_near_wording_across_related_handler_shapes() {
    let definitions = definitions(
        r##"
Given("I am on login page", async function () {
  await this.page.goto("/login");
  await this.page.fill("#email", "user@example.test");
});
Given("I am on the login page", async function () {
  await this.page.goto("/login");
  await this.page.fill("#email", "user@example.test");
  await this.page.waitForLoadState();
});
"##,
    );
    assert_ne!(
        definitions[0].handler.structural,
        definitions[1].handler.structural
    );
    assert!(handler_similarity(&definitions[0], &definitions[1]) >= 0.6);
    let (_directory, config) = config();
    let candidates = definition_pair_candidates(&definitions, &config);
    assert!(candidates.contains_pair(0, 1));
    assert_eq!(
        candidates.census.candidate_sources["matcherBlocking"].evaluated,
        1
    );

    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn matcher_blocking_rejects_similar_wording_without_handler_agreement() {
    let definitions = definitions(
        r##"
Given("I am on login page", async function () {
  await this.page.goto("/login");
  await this.page.fill("#email", "user@example.test");
});
Given("I am on the login page", async function () {
  await unrelatedAudit();
  await unrelatedNotification();
});
"##,
    );
    let (_directory, config) = config();
    assert!(definition_pair_candidates(&definitions, &config).is_empty());
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn matcher_blocking_finds_near_members_outside_an_identical_handler_spanning_tree() {
    let definitions = definitions(
        r#"
Given('completely unrelated setup', () => sharedImplementation());
Given('the account is enabled', () => sharedImplementation());
Given('the accounts are enabled', () => sharedImplementation());
"#,
    );
    let (_directory, config) = config();
    let candidates = definition_pair_candidates(&definitions, &config);
    assert!(candidates.contains_pair(1, 2));
    assert!(candidates.census.candidate_sources["matcherBlocking"].evaluated >= 1);

    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let near = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::NearDuplicateStep)
        .expect("near wording pair");
    let comparison = near.evidence.comparison.as_ref().unwrap();
    assert_eq!(
        [
            comparison.left_matcher.as_str(),
            comparison.right_matcher.as_str()
        ],
        ["the account is enabled", "the accounts are enabled"]
    );
}

#[test]
fn matcher_blocking_keeps_alpha_identical_handlers_with_empty_behavior_signatures() {
    let definitions = definitions(
        r#"
Given('completely unrelated setup', () => world.value);
Given('the account is enabled', () => world.value);
Given('the accounts are enabled', () => world.value);
"#,
    );
    assert!(definitions
        .iter()
        .all(|definition| definition.handler.behavior_signature.is_empty()));
    assert!(definitions
        .iter()
        .all(|definition| definition.handler.comparable && !definition.handler.trivial));

    let (_directory, config) = config();
    let candidates = definition_pair_candidates(&definitions, &config);
    assert!(candidates.contains_pair(1, 2));
    assert!(candidates.census.candidate_sources["matcherBlocking"].evaluated >= 1);

    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn matcher_blocking_respects_the_global_candidate_limit() {
    let mut definitions = definitions(
        r#"
Given('account operation one', () => first());
Given('account operation two', () => second());
Given('account operation three', () => third());
Given('account operation four', () => fourth());
"#,
    );
    for (index, definition) in definitions.iter_mut().enumerate() {
        definition.handler.alpha_normalized = format!("alpha-{index}");
        definition.handler.structural = format!("structure-{index}");
        definition.handler.behavior_signature = vec!["open".to_owned(), "fill".to_owned()];
    }
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 2;
    let generated = definition_pair_candidates(&definitions, &config);
    assert_eq!(generated.len(), 2);
    assert!(generated.census.truncated);
    assert_eq!(
        generated.census.candidate_sources["matcherBlocking"].evaluated,
        2
    );
    assert_eq!(
        generated.census.candidate_sources["matcherBlocking"].skipped,
        1
    );
}

#[test]
fn homogeneous_handler_groups_generate_linear_candidates() {
    let mut source = String::new();
    for index in 0..2_000 {
        source.push_str(&format!(
            "Given('homogeneous handler wording {index}', () => sharedImplementation());\n"
        ));
    }
    let definitions = definitions(&source);
    let (_directory, config) = config();
    assert!(definition_pair_candidates(&definitions, &config).len() <= definitions.len() * 2);
}

#[test]
fn literal_handler_differences_prefer_parameterization_over_near_wording() {
    let definitions = definitions(
        r#"
Then('the save button is shown', async ({ page }) => { await expect(page.locator('#save')).toBeVisible(); });
Then('the save buttons are shown', async ({ page }) => { await expect(page.locator('#saves')).toBeVisible(); });
When('再生ボタンを押す', async ({ page }) => { await page.click('#play'); });
When('停止ボタンを押す', async ({ page }) => { await page.click('#stop'); });
"#,
    );
    assert_eq!(
        definitions[0].handler.structural,
        definitions[1].handler.structural
    );
    assert_eq!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
    let similarity = matcher_similarity(&definitions[0], &definitions[1]);
    assert!(is_near_matcher(
        &definitions[0],
        &definitions[1],
        similarity
    ));
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::ParameterizationCandidate));
}

#[test]
fn empty_handlers_and_disjoint_placeholder_types_do_not_fail_analysis() {
    let definitions = definitions(
        r#"
Given('an integer {int}', () => {});
Given('a string {string}', () => {});
Given('another empty step', () => {});
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result.findings.iter().any(|finding| matches!(
        finding.rule,
        Rule::DuplicateHandler | Rule::NormalizedMatcher
    )));
}

#[test]
fn semantically_distinct_matchers_do_not_become_error_equivalent() {
    let definitions = definitions(
        r#"
Given(/^I wait (\d+) seconds$/, () => first());
Given(/^I wait (a few|several) seconds$/, () => second());
Given('custom {int}', () => third());
Given('custom {INT}', () => fourth());
Given(/^literal collision$/i, () => fifth());
Given('literal collision /i', () => sixth());
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NormalizedMatcher));
}

#[test]
fn awaited_stub_handlers_do_not_create_duplicate_handler_storms() {
    let definitions = definitions(
        r#"
async function notImplemented() { await pending(); }
Given('stub one', notImplemented);
Given('stub two', notImplemented);
Given('stub three', notImplemented);
Given('stub four', notImplemented);
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn similarity_rejects_opposite_or_different_actions() {
    for (left, right) in [
        ("the account is active", "the account is inactive"),
        ("the user can log in", "the user cannot log in"),
    ] {
        let definitions = definitions(&format!(
            "Then('{left}', () => work()); Then('{right}', () => work());"
        ));
        let similarity = matcher_similarity(&definitions[0], &definitions[1]);
        assert!(!is_near_matcher(
            &definitions[0],
            &definitions[1],
            similarity
        ));
    }
}

#[test]
fn matcher_similarity_gates_are_pinned_at_short_and_long_boundaries() {
    struct Case {
        name: &'static str,
        left: &'static str,
        right: &'static str,
        similarity: f64,
        expected: bool,
    }

    for case in [
        Case {
            name: "twelve characters accept the short boundary",
            left: "abcdefghijkl",
            right: "abcdefghijkx",
            similarity: 0.92,
            expected: true,
        },
        Case {
            name: "twelve characters reject below the short boundary",
            left: "abcdefghijkl",
            right: "abcdefghijkx",
            similarity: 0.919,
            expected: false,
        },
        Case {
            name: "thirteen characters accept the long boundary",
            left: "abcdefghijklm",
            right: "abcdefghijklz",
            similarity: 0.90,
            expected: true,
        },
        Case {
            name: "thirteen characters reject below the long boundary",
            left: "abcdefghijklm",
            right: "abcdefghijklz",
            similarity: 0.899,
            expected: false,
        },
        Case {
            name: "negation wins over a high similarity score",
            left: "user is active",
            right: "user is not active",
            similarity: 0.99,
            expected: false,
        },
    ] {
        let definitions = definitions(&format!(
            "Then('{}', () => work()); Then('{}', () => work());",
            case.left, case.right
        ));
        assert_eq!(
            is_near_matcher(&definitions[0], &definitions[1], case.similarity),
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn matcher_similarity_reports_the_stronger_normalized_score() {
    let mut pair = definitions("Given('left', () => first()); Given('right', () => second());");
    for (left, right) in [("", ""), ("kitten", "sitting"), ("account", "accounts")] {
        pair[0].normalized_matcher = left.to_owned();
        pair[1].normalized_matcher = right.to_owned();
        let expected = if left.is_empty() && right.is_empty() {
            1.0
        } else {
            let longest = left.chars().count().max(right.chars().count());
            let edit = 1.0 - strsim::levenshtein(left, right) as f64 / longest as f64;
            edit.max(strsim::jaro_winkler(left, right))
        };
        assert!(
            (matcher_similarity(&pair[0], &pair[1]) - expected).abs() < f64::EPSILON,
            "{left:?} and {right:?}"
        );
    }
}

#[test]
fn handler_similarity_contract_is_table_driven() {
    let mut base = definitions("Given('left', () => first()); Given('right', () => second());");
    base[0].handler.alpha_normalized = "alpha-left".to_owned();
    base[1].handler.alpha_normalized = "alpha-right".to_owned();
    base[0].handler.structural = "structure-left".to_owned();
    base[1].handler.structural = "structure-right".to_owned();

    let cases = [
        (
            "alpha equivalence",
            "same",
            "same",
            "left",
            "right",
            vec!["open"],
            vec!["close"],
            1.0,
        ),
        (
            "structural equivalence",
            "left",
            "right",
            "same",
            "same",
            vec!["open"],
            vec!["close"],
            0.95,
        ),
        (
            "ordered behavior overlap",
            "left",
            "right",
            "left",
            "right",
            vec!["open", "fill", "save"],
            vec!["open", "save", "notify"],
            2.0 / 3.0,
        ),
        (
            "no behavior",
            "left",
            "right",
            "left",
            "right",
            vec![],
            vec![],
            0.0,
        ),
    ];

    for (
        name,
        left_alpha,
        right_alpha,
        left_structure,
        right_structure,
        left_events,
        right_events,
        expected,
    ) in cases
    {
        let mut pair = base.clone();
        pair[0].handler.alpha_normalized = left_alpha.to_owned();
        pair[1].handler.alpha_normalized = right_alpha.to_owned();
        pair[0].handler.structural = left_structure.to_owned();
        pair[1].handler.structural = right_structure.to_owned();
        pair[0].handler.behavior_signature = left_events.into_iter().map(str::to_owned).collect();
        pair[1].handler.behavior_signature = right_events.into_iter().map(str::to_owned).collect();
        assert!(
            (handler_similarity(&pair[0], &pair[1]) - expected).abs() < f64::EPSILON,
            "{name}"
        );
    }
}

#[test]
fn duplicate_matcher_components_are_invariant_to_definition_input_order() {
    let mut ordered = definitions(
        "Given('same', () => first());\nGiven('same', () => second());\nGiven('same', () => third());",
    );
    for (index, definition) in ordered.iter_mut().enumerate() {
        definition.location.path = PathBuf::from(format!("definition-{index}.ts"));
    }
    let (_directory, config) = config();

    fn component_members(result: &AnalysisResult) -> Vec<String> {
        let mut paths = result
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateMatcher)
            .flat_map(|finding| std::iter::once(&finding.primary).chain(&finding.related))
            .map(|location| location.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        paths
    }

    let expected = [
        "definition-0.ts".to_owned(),
        "definition-1.ts".to_owned(),
        "definition-2.ts".to_owned(),
    ];
    for permutation in [[0, 1, 2], [2, 1, 0], [0, 2, 1], [1, 0, 2]] {
        let definitions = permutation
            .into_iter()
            .map(|index| ordered[index].clone())
            .collect();
        let result = analyze(definitions, Vec::new(), &config).unwrap();
        assert_eq!(component_members(&result), expected);
        assert_eq!(
            result
                .findings
                .iter()
                .filter(|finding| finding.rule == Rule::DuplicateMatcher)
                .count(),
            2
        );
    }
}

#[test]
fn shared_context_does_not_override_parameterization_evidence() {
    for (left, right) in [
        (
            "I click the save button on the checkout summary page",
            "I click the cancel button on the checkout summary page",
        ),
        (
            "I select the first row of the table",
            "I select the last row of the table",
        ),
        (
            "the advanced reporting feature is enabled for this account",
            "the advanced reporting feature is disabled for this account",
        ),
    ] {
        let definitions = definitions(&format!(
            "Then('{left}', () => choose('left')); Then('{right}', () => choose('right'));"
        ));
        let (_directory, config) = config();
        let result = analyze(definitions, Vec::new(), &config).unwrap();
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::ParameterizationCandidate));
        assert!(!result
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::NearDuplicateStep));
    }
}

#[test]
fn path_and_matcher_suppression_must_select_the_same_definition() {
    let mut definitions =
        definitions("Given('left matcher', () => work()); Given('right matcher', () => work());");
    let (directory, mut config) = config();
    definitions[0].location.path = directory.path().join("left.ts");
    definitions[1].location.path = directory.path().join("right.ts");
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "must not span definitions".to_owned(),
        path: Some("left.ts".to_owned()),
        matcher: Some("right matcher".to_owned()),
    });
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap();
    assert!(finding.suppression.is_none());

    config.suppressions[0].path = Some("*.ts".to_owned());
    config.suppressions[0].matcher = Some("missing matcher".to_owned());
    let result = analyze(result.definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap();
    assert!(finding.suppression.is_none());
}

#[test]
fn suppression_path_globs_remain_root_relative() {
    let mut definitions =
        definitions("Given('left matcher', () => work()); Given('right matcher', () => work());");
    let (_directory, mut config) = config();
    definitions[0].location.path = config.root.join("root.ts");
    definitions[1].location.path = config.root.join("nested/steps.ts");
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "root files only".to_owned(),
        path: Some("*.ts".to_owned()),
        matcher: None,
    });
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap();
    assert!(finding.suppression.is_some());

    config.suppressions[0].path = Some("steps.ts".to_owned());
    let result = analyze(result.definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap();
    assert!(finding.suppression.is_none());
}

#[test]
fn path_suppression_requires_every_definition_in_a_pair_to_be_in_scope() {
    let mut definitions =
        definitions("Given('left matcher', () => work()); Given('right matcher', () => work());");
    let (_directory, mut config) = config();
    definitions[0].location.path = config.root.join("legacy/left.ts");
    definitions[1].location.path = config.root.join("current/right.ts");
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "legacy directory only".to_owned(),
        path: Some("legacy/**".to_owned()),
        matcher: None,
    });

    let result = analyze(definitions.clone(), Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap()
        .suppression
        .is_none());

    definitions[1].location.path = config.root.join("legacy/right.ts");
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap()
        .suppression
        .is_some());
}

#[test]
fn suppression_directory_patterns_and_unmatched_detection_share_semantics() {
    let mut definitions = definitions("Given('legacy', () => work());");
    let (_directory, mut config) = config();
    definitions[0].location.path = config.root.join("legacy/nested/steps.ts");
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "legacy tree".to_owned(),
        path: Some("legacy/".to_owned()),
        matcher: None,
    });
    assert!(
        super::suppression::SuppressionIndex::new(&config, &definitions)
            .unmatched()
            .indices
            .is_empty()
    );

    config.suppressions[0].path = Some("missing/**".to_owned());
    assert_eq!(
        super::suppression::SuppressionIndex::new(&config, &definitions)
            .unmatched()
            .indices,
        [0]
    );
}

#[test]
fn opposite_assertions_are_not_duplicate_handlers() {
    let definitions = definitions(
        r#"
Then('the item is visible', async () => { await expect(item).toBeVisible(); });
Then('the item is hidden', async () => { await expect(item).not.toBeVisible(); });
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn pair_evidence_retains_source_and_unicode_safe_matcher_delta() {
    let definitions = definitions(
        r#"
Then('the item is visible', async () => { await expect(item).toBeVisible(); });
Then('the items are visible', async () => { await expect(item).toBeVisible(); });
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let comparison = result
        .findings
        .iter()
        .find_map(|finding| finding.evidence.comparison.as_ref())
        .expect("pair comparison");
    assert_eq!(comparison.matcher_diff.prefix, "the item");
    assert_eq!(comparison.matcher_diff.left_change, " is");
    assert_eq!(comparison.matcher_diff.right_change, "s are");
    assert_eq!(comparison.matcher_diff.suffix, " visible");
    assert!(comparison.left_handler.contains("toBeVisible"));
}

#[test]
fn concrete_feature_step_detects_ambiguity_and_usage() {
    let definitions = definitions(
        r#"
Given('a user named {word}', () => { makeUser(); });
Given(/^a user named .+$/, () => { makeOtherUser(); });
"#,
    );
    let steps = gherkin::extract(
        "Feature: Users\n  Scenario: Create\n    Given a user named Ada",
        Path::new("test.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::AmbiguousStep));
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::UnusedDefinition));
}

#[test]
fn scenario_outline_rows_and_case_insensitive_regexes_count_as_usage() {
    let definitions = definitions(
        r#"
Given('I have {int} items', () => { count(); });
Then(/^THE USER EXISTS$/i, () => { verify(); });
"#,
    );
    let steps = gherkin::extract(
        r#"
Feature: Outline
  Scenario Outline: Count
Given I have <count> items
Then the user exists
Examples:
  | count |
  | 2     |
"#,
        Path::new("outline.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::UnusedDefinition));
}

#[test]
fn supports_optional_and_alternative_cucumber_expression_syntax() {
    let definitions = definitions(
        r#"
Given('I have {int} cucumber(s)', () => { count(); });
Then('they are in my belly/stomach', () => { verify(); });
"#,
    );
    let steps = gherkin::extract(
        "Feature: Food\n  Scenario: Eat\n    Given I have 2 cucumbers\n    Then they are in my stomach",
        Path::new("food.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::UnusedDefinition));
}

#[test]
fn fallback_for_custom_parameters_preserves_optional_and_alternative_syntax() {
    let definitions = definitions(
        r#"
Given('I have {quantity} cucumber(s)', () => { count(); });
Then('they are in my belly/stomach with {mood}', () => { verify(); });
"#,
    );
    let steps = gherkin::extract(
        "Feature: Food\n  Scenario: Eat\n    Given I have several cucumbers\n    Then they are in my belly with joy",
        Path::new("food.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::UnusedDefinition));
}

#[test]
fn named_handlers_with_different_bodies_and_stub_handlers_do_not_duplicate() {
    let mut extracted = Vec::new();
    for (path, source) in [
        (
            "steps/a.ts",
            "function handler() { doAlpha(); } Given('alpha step', handler);",
        ),
        (
            "steps/b.ts",
            "function handler() { doBeta(); } Given('bravo step', handler);",
        ),
        (
            "steps/pending.ts",
            "function pendingStep() { throw new Error('pending'); } Given('pending one', pendingStep); Given('pending two', pendingStep);",
        ),
    ] {
        extracted.extend(
            typescript::extract(
                source,
                &SourceFile {
                    path: PathBuf::from(path),
                    language: SourceLanguage::TypeScript,
                },
            )
            .unwrap(),
        );
    }
    let (_directory, config) = config();
    let result = analyze(extracted, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn differently_bound_arguments_do_not_report_duplicate_handlers() {
    let definitions = definitions(
        r#"
function handler(value) { process(value); }
Given('first binding', handler.bind(null, 1));
Given('second binding', handler.bind(null, 2));
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn outline_ambiguities_are_deduplicated_by_template_location_and_matches() {
    let definitions = definitions(
        r#"
Given('I have {int} items', () => { first(); });
Given(/^I have \d+ items$/, () => { second(); });
"#,
    );
    let steps = gherkin::extract(
        r#"
Feature: Repeated rows
  Scenario Outline: Counts
Given I have <count> items
Examples:
  | count |
  | 1     |
  | 2     |
  | 3     |
"#,
        Path::new("outline.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::AmbiguousStep)
            .count(),
        1
    );
}

#[test]
fn outline_ambiguities_keep_disjoint_match_sets_separate() {
    let definitions = definitions(
        r#"
Given('value {int}', () => first());
Given(/^value \d+$/, () => second());
Given(/^value 1$/, () => third());
"#,
    );
    let steps = gherkin::extract(
        r#"
Feature: Different sets
  Scenario Outline: Values
    Given value <value>
    Examples:
      | value |
      | 1     |
      | 2     |
"#,
        Path::new("outline.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    let ambiguities = result
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::AmbiguousStep)
        .collect::<Vec<_>>();
    assert_eq!(ambiguities.len(), 2);
    assert!(ambiguities.iter().all(|finding| finding.related.len() >= 2));
}

#[test]
fn custom_parameter_fallback_marks_usage_but_cannot_prove_ambiguity() {
    let definitions = definitions(
        r#"
Given('I wait {duration}', () => waitDuration());
Given('I wait for the page', () => waitForPage());
"#,
    );
    let steps = gherkin::extract(
        "Feature: Wait\nScenario: custom\nGiven I wait for the page\nGiven I wait 3 seconds\n",
        Path::new("wait.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| { matches!(finding.rule, Rule::AmbiguousStep | Rule::UnusedDefinition) }));
}

#[test]
fn ordered_behavior_similarity_uses_longest_common_subsequence() {
    for (left, right, expected) in [
        (
            vec!["open", "fill", "save"],
            vec!["open", "save", "notify"],
            2,
        ),
        (vec!["open"], vec!["close"], 0),
        (vec!["open", "save"], vec!["open", "save"], 2),
        (vec![], vec!["open"], 0),
    ] {
        let left = left.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let right = right.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(ordered_common_subsequence_len(&left, &right), expected);
        assert_eq!(ordered_common_subsequence_len(&right, &left), expected);
    }
}

#[test]
fn reported_similarity_scores_round_to_three_decimal_places() {
    for (input, expected) in [
        (0.0, 0.0),
        (0.123_4, 0.123),
        (0.123_5, 0.124),
        (0.999_9, 1.0),
    ] {
        assert_eq!(super::similarity::round_score(input), expected);
    }
}
