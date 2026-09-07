use super::pairs::definition_pair_candidates;
use super::similarity::{is_near_matcher, matcher_similarity, ordered_common_subsequence_len};
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
    assert!(definition_pair_candidates(&unrelated).unwrap().is_empty());

    let exact = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());",
    );
    assert_eq!(definition_pair_candidates(&exact).unwrap().len(), 2);
}

#[test]
fn candidate_limit_fails_closed_before_pathological_pair_expansion() {
    let mut source = String::new();
    for index in 0..143 {
        source.push_str(&format!(
            "Given('operation label {index}', () => perform({index}));\n"
        ));
    }
    let error = definition_pair_candidates(&definitions(&source)).unwrap_err();
    assert!(error
        .to_string()
        .contains("candidate definition comparisons"));
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
fn homogeneous_handler_groups_generate_linear_candidates() {
    let mut source = String::new();
    for index in 0..2_000 {
        source.push_str(&format!(
            "Given('homogeneous handler wording {index}', () => sharedImplementation());\n"
        ));
    }
    let definitions = definitions(&source);
    assert!(definition_pair_candidates(&definitions).unwrap().len() <= definitions.len() * 2);
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
    assert!(super::unmatched_suppressions(&config, &definitions).is_empty());

    config.suppressions[0].path = Some("missing/**".to_owned());
    assert_eq!(super::unmatched_suppressions(&config, &definitions), [0]);
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
    let left = vec!["open".to_owned(), "fill".to_owned(), "save".to_owned()];
    let right = vec!["open".to_owned(), "save".to_owned(), "notify".to_owned()];
    assert_eq!(ordered_common_subsequence_len(&left, &right), 2);
    assert_eq!(ordered_common_subsequence_len(&right, &left), 2);
}
