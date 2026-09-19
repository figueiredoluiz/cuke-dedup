use super::*;
use crate::config::{ConfigOverrides, SuppressionConfig};
use crate::discovery::{SourceFile, SourceLanguage};
use std::path::{Path, PathBuf};

fn definitions(source: &str) -> Vec<StepDefinition> {
    crate::typescript::extract(
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
fn cucumber_expression_expansion_stops_at_the_regex_pattern_limit() {
    let matcher = ".".repeat(600_000);
    assert!(matches!(
        cucumber_regex_expression(&matcher),
        CucumberRegexExpression::ResourceLimit
    ));
}

#[test]
fn regex_pattern_limit_is_inclusive() {
    let mut definition = crate::typescript::extract(
        "Given(/x/, () => work());",
        &SourceFile {
            path: PathBuf::from("steps.ts"),
            language: SourceLanguage::TypeScript,
        },
    )
    .unwrap()
    .remove(0);
    definition.matcher = format!(
        "(?x){}",
        " ".repeat(MAX_REGEX_PATTERN_BYTES.saturating_sub(4))
    );

    let (compiled, error) = compile_matcher(&definition, &BTreeMap::new());
    assert!(error.is_none());
    assert!(compiled.regex.is_some());
}

#[test]
fn broad_ambiguity_groups_use_linear_membership_storage() {
    let matched = (0..10_000).collect::<BTreeSet<_>>();
    let mut proven = ProvenAmbiguities::new(matched.len());

    proven.record(&matched);
    proven.record(&matched);

    assert_eq!(proven.groups.len(), 1);
    assert_eq!(
        proven.memberships.iter().map(Vec::len).sum::<usize>(),
        10_000
    );
    assert!(proven.contains(0, 9_999));
}

#[test]
fn matcher_overlap_respects_the_shared_candidate_budget() {
    let definitions = definitions(
        "Given('value {first}', () => first());\n\
         Given('value {second}', () => second());\n\
         Given('value {third}', () => third());",
    );
    let (_directory, mut config) = config();
    for name in ["first", "second", "third"] {
        config
            .parameter_types
            .insert(name.to_owned(), "red|green".to_owned());
    }
    let suppressions = SuppressionIndex::new(&config, &definitions);
    let mut findings = Vec::new();

    let outcome =
        analyze_feature_usage(&definitions, &[], &config, &suppressions, &mut findings, 1);

    assert_eq!(outcome.overlap_census.evaluated, 1);
    assert_eq!(outcome.overlap_census.skipped, 1);
    assert!(outcome
        .incomplete
        .iter()
        .any(|diagnostic| diagnostic.contains("matcher-overlap analysis is incomplete")));
}

#[test]
fn matcher_overlap_charges_index_scans_before_candidate_generation() {
    let definitions = definitions(
        "Given('value {first}', () => first());\n\
         Given('value {second}', () => second());\n\
         Given('value {third}', () => third());\n\
         Given('value {fourth}', () => fourth());\n\
         Given('value {fifth}', () => fifth());",
    );
    let (_directory, mut config) = config();
    for name in ["first", "second", "third", "fourth", "fifth"] {
        config
            .parameter_types
            .insert(name.to_owned(), format!("{name}-only"));
    }
    let summaries = definitions
        .iter()
        .map(|definition| {
            let matcher = compile_matcher(definition, &config.parameter_types).0;
            MatcherSummary {
                authoritative: matcher.authoritative,
                has_regex: matcher.regex.is_some(),
            }
        })
        .collect::<Vec<_>>();
    let suppressions = SuppressionIndex::new(&config, &definitions);
    let mut findings = Vec::new();

    // One definition per window puts one automaton scan per definition into the witness scan
    // charge, so the scan budget is exceeded before any candidate pair is proposed. Previously the
    // same condition was produced by shrinking the regex limits until one index split into many
    // sets; windowing is now the mechanism that multiplies scans.
    let outcome = analyze_matcher_overlap(
        &definitions,
        &summaries,
        &ProvenAmbiguities::new(definitions.len()),
        &config,
        &suppressions,
        &mut findings,
        1,
        1,
        // Scan work the step-matching pass reports for this corpus at one definition per window:
        // one automaton per window. With a work budget of 1 the witness scan budget is 4, so not
        // even one witness is affordable and nothing is scanned.
        definitions.len(),
    );

    assert_eq!(outcome.census.evaluated, 0);
    assert_eq!(outcome.census.skipped, 1);
    assert!(outcome.incomplete.is_some());
    assert!(findings.is_empty());
}

#[test]
fn reverse_witnesses_do_not_consume_the_unique_pair_budget_twice() {
    let definitions = definitions(
        "Given('value {first}', () => first());\n\
         Given('value {second}', () => second());",
    );
    let (_directory, mut config) = config();
    for name in ["first", "second"] {
        config
            .parameter_types
            .insert(name.to_owned(), "red|green".to_owned());
    }
    let suppressions = SuppressionIndex::new(&config, &definitions);
    let mut findings = Vec::new();

    let outcome =
        analyze_feature_usage(&definitions, &[], &config, &suppressions, &mut findings, 1);

    assert_eq!(outcome.overlap_census.evaluated, 1);
    assert_eq!(outcome.overlap_census.skipped, 0);
    assert!(outcome.incomplete.is_empty());
    assert_eq!(findings.len(), 1);
}

#[test]
fn matcher_overlap_caps_finding_storms_and_marks_the_result_incomplete() {
    let mut source = String::new();
    let mut parameter_types = BTreeMap::new();
    for index in 0..150 {
        source.push_str(&format!(
            "Given('value {{type_{index}}}', () => action_{index}());\n"
        ));
        parameter_types.insert(format!("type_{index}"), "red|green".to_owned());
    }
    let definitions = definitions(&source);
    let (_directory, mut config) = config();
    config.parameter_types = parameter_types;
    let suppressions = SuppressionIndex::new(&config, &definitions);
    let mut findings = Vec::new();

    let outcome = analyze_feature_usage(
        &definitions,
        &[],
        &config,
        &suppressions,
        &mut findings,
        30_000,
    );

    assert_eq!(findings.len(), MAX_STATIC_OVERLAP_FINDINGS);
    assert_eq!(outcome.overlap_census.skipped, 1);
    assert!(outcome
        .incomplete
        .iter()
        .any(|diagnostic| diagnostic.contains("matcher-overlap analysis is incomplete")));
    assert!(findings
        .iter()
        .all(|finding| finding.rule == Rule::OverlappingMatcher));
}

#[test]
fn oversized_combined_matcher_programs_split_without_changing_matches() {
    let compiled = (0..200)
        .map(|index| {
            let expression = format!("^distinct matcher value {index:04}$");
            CompiledMatcher {
                regex: Some(Regex::new(&expression).unwrap()),
                expression: Some(expression),
                authoritative: true,
            }
        })
        .collect::<Vec<_>>();

    let index = MatcherIndex::build_with_limits(&compiled, 4 * 1024, 64 * 1024);
    assert!(index.sets.len() > 1);
    assert!(index.fallback_definitions.is_empty());

    let mut matches = Vec::new();
    index.matches(&compiled, "distinct matcher value 0123", &mut matches);
    assert_eq!(matches, vec![123]);
}

#[test]
fn matcher_index_falls_back_when_even_one_combined_pattern_is_rejected() {
    let compiled = vec![CompiledMatcher {
        regex: Some(Regex::new("^fallback$").unwrap()),
        expression: Some("(".to_owned()),
        authoritative: true,
    }];
    let index = MatcherIndex::build_with_limits(&compiled, 32, 32);
    assert!(index.sets.is_empty());
    assert_eq!(index.fallback_definitions, [0]);
    let mut matches = Vec::new();
    index.matches(&compiled, "fallback", &mut matches);
    assert_eq!(matches, [0]);
}

#[test]
fn ambiguity_membership_intersection_handles_ordered_disjoint_groups() {
    let mut proven = ProvenAmbiguities::new(5);
    proven.record(&[0, 2].into_iter().collect());
    proven.record(&[1, 3].into_iter().collect());
    proven.record(&[2, 4].into_iter().collect());
    assert!(!proven.contains(0, 1));
    assert!(proven.contains(0, 2));
    assert!(!proven.contains(1, 4));
}

#[test]
fn feature_usage_caches_repeated_text_and_supports_suppressed_ambiguity() {
    let definitions =
        definitions("Given('same step', () => first());\nGiven(/^same step$/, () => second());");
    let steps = crate::gherkin::extract(
        "Feature: F\n  Scenario Outline: S\n    Given same step\n    And same step\n    Examples:\n      | value |\n      | one |\n",
        Path::new("example.feature"),
    )
    .unwrap();
    let (_directory, mut config) = config();
    config.suppressions.push(SuppressionConfig {
        rule: Rule::AmbiguousStep,
        reason: "known overlap".to_owned(),
        path: Some("steps.ts".to_owned()),
        matcher: None,
    });
    let suppressions = SuppressionIndex::new(&config, &definitions);
    let mut findings = Vec::new();
    let outcome = analyze_feature_usage(
        &definitions,
        &steps,
        &config,
        &suppressions,
        &mut findings,
        100,
    );
    assert_eq!(outcome.used, [0, 1].into_iter().collect());
    assert_eq!(findings.len(), 2);
    assert!(findings
        .iter()
        .all(|finding| finding.suppression.as_ref().unwrap().reason == "known overlap"));
}

#[test]
fn overlap_and_unused_rules_honor_disabled_severity_and_usage() {
    let definitions = definitions("Given('unused', () => work());");
    let (_directory, mut config) = config();
    config.rules.insert(Rule::OverlappingMatcher, Severity::Off);
    config.rules.insert(Rule::UnusedDefinition, Severity::Off);
    let suppressions = SuppressionIndex::new(&config, &definitions);
    let mut findings = Vec::new();
    let outcome = analyze_feature_usage(
        &definitions,
        &[],
        &config,
        &suppressions,
        &mut findings,
        100,
    );
    analyze_unused(
        &definitions,
        &BTreeSet::new(),
        &config,
        &suppressions,
        &mut findings,
    );
    assert_eq!(outcome.overlap_census, CandidateSourceCensus::default());
    assert!(findings.is_empty());

    config
        .rules
        .insert(Rule::UnusedDefinition, Severity::Warning);
    let suppressions = SuppressionIndex::new(&config, &definitions);
    analyze_unused(
        &definitions,
        &[0].into_iter().collect(),
        &config,
        &suppressions,
        &mut findings,
    );
    assert!(findings.is_empty());
}

#[test]
fn non_authoritative_matchers_cannot_prove_a_definition_unused() {
    let mut extracted = definitions("Given('a {projectType}', () => work());");
    extracted.push(definitions("Given(/^value$/v, () => work());").remove(0));
    let (_directory, mut config) = config();
    config
        .rules
        .insert(Rule::UnusedDefinition, Severity::Warning);
    let suppressions = SuppressionIndex::new(&config, &extracted);
    let mut findings = Vec::new();

    let outcome =
        analyze_feature_usage(&extracted, &[], &config, &suppressions, &mut findings, 100);
    analyze_unused(
        &extracted,
        &outcome.used,
        &config,
        &suppressions,
        &mut findings,
    );

    assert_eq!(outcome.used, [0, 1].into_iter().collect());
    assert!(findings.is_empty());
}

#[test]
fn witness_generation_covers_builtins_declared_literals_and_unknowns() {
    let mut definitions = definitions(
        "Given('I have {int} item(s) in red/blue', () => work());\nGiven(/^regex$/, () => work());",
    );
    assert_eq!(
        witness_for(&definitions[0], &BTreeMap::new()).as_deref(),
        Some("I have 1 items in red")
    );
    assert!(witness_for(&definitions[1], &BTreeMap::new()).is_none());
    definitions[0].matcher = "a {colour}".to_owned();
    assert!(witness_for(&definitions[0], &BTreeMap::new()).is_none());
    for (pattern, expected) in [
        ("red|green", Some("red")),
        ("(red|green)", Some("red")),
        ("(?:red|green)", Some("red")),
        ("", None),
        ("red.*", None),
        ("(red", None),
    ] {
        assert_eq!(
            literal_alternative(pattern).as_deref(),
            expected,
            "{pattern}"
        );
    }
    assert_eq!(witness_literal(r"a\(b) c/d"), "a(b) c");
    assert_eq!(witness_literal("open(unclosed"), "open(unclosed");
}

#[test]
fn fallback_expression_handles_optional_alternative_and_declared_parameters() {
    let parameters = BTreeMap::from([("colour".to_owned(), "red|green".to_owned())]);
    let exact = fallback_cucumber_expression_regex(
        "I eat/eats {int} cucumber(s) with {colour}",
        &parameters,
    );
    assert!(exact.exact);
    let regex = Regex::new(&exact.expression).unwrap();
    assert!(regex.is_match("I eat 2 cucumbers with red"));
    assert!(regex.is_match("I eats -2 cucumber with green"));

    let approximate = fallback_cucumber_expression_regex("a {custom}", &BTreeMap::new());
    assert!(!approximate.exact);
    assert!(Regex::new(&approximate.expression)
        .unwrap()
        .is_match("a anything"));
    assert_eq!(
        fallback_cucumber_token_regex("open(unclosed"),
        r"open\(unclosed"
    );
    assert!(fallback_cucumber_literal_regex("two  spaces").contains("two"));
}

#[test]
fn matcher_compilation_reports_limits_but_not_unsupported_syntax() {
    let mut definition = definitions("Given(/^valid$/, () => work());").remove(0);
    definition.matcher = "x".repeat(MAX_REGEX_PATTERN_BYTES + 1);
    let (compiled, diagnostic) = compile_matcher(&definition, &BTreeMap::new());
    assert!(compiled.regex.is_none());
    assert!(diagnostic.unwrap().contains("regex resource limit"));

    definition.matcher = "(".to_owned();
    let (compiled, diagnostic) = compile_matcher(&definition, &BTreeMap::new());
    assert!(compiled.regex.is_none());
    assert!(diagnostic.is_none());

    definition.matcher_kind = MatcherKind::CucumberExpression;
    definition.matcher = "x".repeat(MAX_REGEX_PATTERN_BYTES + 1);
    let (compiled, diagnostic) = compile_matcher(&definition, &BTreeMap::new());
    assert!(compiled.regex.is_none());
    assert!(diagnostic.unwrap().contains("regex resource limit"));
}

/// Windowing must not change what is reported, only how much is alive at once.
///
/// Each row is an axis from the windowing matrix. Every row runs at one definition per window, at
/// two, and at a window wider than the corpus, and all three must produce identical findings. At
/// window 1 every pair is cross-window, so the merge paths are exercised by construction — a fixture
///-sized corpus never crosses a boundary at the default window of 250.
#[test]
fn windowing_does_not_change_findings_on_any_axis() {
    struct Case {
        axis: &'static str,
        source: &'static str,
        feature: &'static str,
    }
    let cases = [
        Case {
            axis: "step matching definitions in more than one window",
            source: "Given('the gate opens {word}', () => a());\nGiven(/^the gate opens wide$/, () => b());\nGiven('the gate opens wide', () => c());",
            feature: "Feature: F\n  Scenario: S\n    Given the gate opens wide\n",
        },
        Case {
            // The hazard: a proven ambiguity split across windows must still withhold the overlap
            // finding, because `ambiguous-step` owns that pair.
            axis: "proven ambiguity pair split across windows withholds the overlap",
            source: "Given('the dial reads {int}', () => a());\nGiven(/^the dial reads \\d+$/, () => b());",
            feature: "Feature: F\n  Scenario: S\n    Given the dial reads 7\n",
        },
        Case {
            axis: "overlap with no feature corpus at all",
            source: "Given('the pump runs {word}', () => a());\nGiven(/^the pump runs .+$/, () => b());",
            feature: "Feature: F\n  Scenario: S\n    Given something unrelated\n",
        },
        Case {
            axis: "equivalent-matcher group spanning windows keeps one representative witness",
            source: "Given('the lamp glows {word}', () => a());\nGiven('the lamp glows {word}', () => b());\nGiven(/^the lamp glows .+$/, () => c());",
            feature: "Feature: F\n  Scenario: S\n    Given nothing here\n",
        },
        Case {
            axis: "unused definitions are indeterminate or proven across windows",
            source: "Given('the seal closes', () => a());\nGiven('the rotor spins', () => b());\nGiven('the brake holds', () => c());",
            feature: "Feature: F\n  Scenario: S\n    Given the rotor spins\n",
        },
    ];

    for case in cases {
        let definitions = definitions(case.source);
        let steps = crate::gherkin::extract(case.feature, Path::new("example.feature")).unwrap();
        let (_directory, config) = config();
        let suppressions = SuppressionIndex::new(&config, &definitions);

        // Findings and `used` are window-invariant **for runs that do not truncate**, which is what
        // these cases are. Once a budget is exhausted that no longer holds: the witness-scan charge
        // grows as windows shrink, and the proposal cap cuts a window-major stream where a single
        // pass cut a witness-major one, so both which pairs survive and whether the run truncates
        // become window-dependent. These cases run well inside their budgets, where every output
        // must agree, and asserting all four pins that — it is not a claim of invariance under
        // truncation.
        /// Everything one window size observed, so two sizes can be compared field by field.
        struct Observed {
            findings: Vec<String>,
            used: Vec<usize>,
            incomplete: Vec<String>,
            evaluated: u64,
        }
        let mut reference: Option<Observed> = None;
        for window in [1, 2, definitions.len() + 5] {
            let mut findings = Vec::new();
            let outcome = analyze_feature_usage_in_windows(
                &definitions,
                &steps,
                &config,
                &suppressions,
                &mut findings,
                1_000,
                window,
            );
            let mut rendered: Vec<String> = findings
                .iter()
                .map(|finding| {
                    format!(
                        "{:?}|{}|{}|{:?}",
                        finding.rule,
                        finding.primary.line,
                        finding.message,
                        finding
                            .related
                            .iter()
                            .map(|location| location.line)
                            .collect::<Vec<_>>()
                    )
                })
                .collect();
            rendered.sort();
            let used = outcome.used.iter().copied().collect::<Vec<_>>();
            let incomplete = outcome.incomplete.clone();
            let evaluated = u64::try_from(outcome.overlap_census.evaluated).unwrap();
            match &reference {
                None => {
                    reference = Some(Observed {
                        findings: rendered,
                        used,
                        incomplete,
                        evaluated,
                    });
                }
                Some(expected) => {
                    assert_eq!(
                        &rendered, &expected.findings,
                        "{}: findings changed at window {window}",
                        case.axis
                    );
                    assert_eq!(
                        &used, &expected.used,
                        "{}: used set changed at window {window}",
                        case.axis
                    );
                    assert_eq!(
                        &incomplete, &expected.incomplete,
                        "{}: incomplete diagnostics changed at window {window}",
                        case.axis
                    );
                    assert_eq!(
                        evaluated, expected.evaluated,
                        "{}: overlap census changed at window {window}",
                        case.axis
                    );
                }
            }
        }
        // A case must discriminate, or comparing it across window sizes proves nothing. Findings
        // are the observable for the ambiguity and overlap axes; for the usage axis it is a `used`
        // set that is neither empty nor everything, meaning the pass actually classified.
        let observed = reference.expect("at least one window was evaluated");
        assert!(
            !observed.findings.is_empty()
                || (!observed.used.is_empty() && observed.used.len() < definitions.len()),
            "{}: no finding and no partial used set, so the comparison proves nothing",
            case.axis
        );
    }
}
