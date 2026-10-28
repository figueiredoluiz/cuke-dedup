use super::*;
use crate::config::{ConfigOverrides, SuppressionConfig};
use crate::discovery::SourceFile;
use crate::typescript::TYPESCRIPT;
use std::path::{Path, PathBuf};

/// Extracts TypeScript step definitions from a synthetic steps.ts source, panicking on extraction failure.
fn definitions(source: &str) -> Vec<StepDefinition> {
    crate::typescript::extract(
        source,
        &SourceFile {
            path: PathBuf::from("steps.ts"),
            language: TYPESCRIPT,
        },
    )
    .unwrap()
}

/// Matcher summaries as the step-matching pass computes them, for direct overlap calls.
fn summaries(definitions: &[StepDefinition], config: &Config) -> Vec<MatcherSummary> {
    definitions
        .iter()
        .map(|definition| {
            let matcher = compile_matcher(definition, &config.parameter_types).0;
            MatcherSummary {
                authoritative: matcher.authoritative,
                has_regex: matcher.regex.is_some(),
            }
        })
        .collect()
}

/// Creates a default configuration and returns its temporary directory to keep the root alive.
fn config() -> (tempfile::TempDir, Config) {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    (directory, config)
}

type OverlapFixture = (
    tempfile::TempDir,
    Vec<StepDefinition>,
    Config,
    Vec<MatcherSummary>,
    Vec<(usize, String)>,
);

/// Builds definitions, summaries, and a red-colour witness with a retained temporary configuration root.
fn overlap_fixture(source: &str) -> OverlapFixture {
    let definitions = definitions(source);
    let (directory, mut config) = config();
    config
        .parameter_types
        .insert("colour".to_owned(), "red".to_owned());
    let summaries = summaries(&definitions, &config);
    let witnesses = vec![(0, "value red".to_owned())];
    (directory, definitions, config, summaries, witnesses)
}

/// Checks that expanding a large literal matcher stops with a regex resource-limit result.
#[test]
fn cucumber_expression_expansion_stops_at_the_regex_pattern_limit() {
    let matcher = ".".repeat(600_000);
    assert!(matches!(
        cucumber_regex_expression(&matcher),
        CucumberRegexExpression::ResourceLimit
    ));
}

/// Checks that a regex exactly at the pattern-byte limit still compiles.
#[test]
fn regex_pattern_limit_is_inclusive() {
    let mut definition = crate::typescript::extract(
        "Given(/x/, () => work());",
        &SourceFile {
            path: PathBuf::from("steps.ts"),
            language: TYPESCRIPT,
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

/// Checks that recording a broad ambiguity twice stores one group and one membership per definition.
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

/// Checks that overlap analysis stops after its shared candidate allowance and reports incompleteness.
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

/// Witness index scans are charged against the configured limit before any candidate is proposed.
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
    config.max_candidate_comparisons = 1;
    for name in ["first", "second", "third", "fourth", "fifth"] {
        config
            .parameter_types
            .insert(name.to_owned(), format!("{name}-only"));
    }
    let summaries = summaries(&definitions, &config);
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
        &BTreeMap::new(),
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

/// Stored overlap proposals never exceed the budget, even when one witness's match list crosses
/// it inside a single window, and the proposable match that did not fit is reported.
#[test]
fn matcher_overlap_proposals_stop_at_the_budget_inside_one_match_list() {
    // Six regexes accept the one witness `value red`: one match list of six against a budget of 4.
    let (_directory, definitions, config, summaries, witnesses) = overlap_fixture(
        "Given('value {colour}', () => colour());\n\
         Given(/^value red$/, () => one());\n\
         Given(/^value r.*$/, () => two());\n\
         Given(/^value .*$/, () => three());\n\
         Given(/^val.*$/, () => four());\n\
         Given(/^v.*$/, () => five());\n\
         Given(/^.*red$/, () => six());",
    );
    let collect = |window, budget| {
        collect_overlap_proposals(
            &definitions,
            &summaries,
            &config.parameter_types,
            window,
            &witnesses,
            budget,
            None,
        )
    };
    assert_eq!(
        collect(definitions.len(), 4),
        (
            vec![vec![1, 2, 3, 4]],
            true,
            OverlapProposalSource::CompiledWindows
        )
    );
    assert_eq!(
        collect(definitions.len(), 6),
        (
            vec![vec![1, 2, 3, 4, 5, 6]],
            false,
            OverlapProposalSource::CompiledWindows
        )
    );
}

/// Checks that complete cached matches preserve compiled proposals while missing or over-budget cache entries fall back to scanning.
#[test]
fn complete_feature_matches_replace_only_an_untruncated_overlap_scan() {
    let (_directory, definitions, config, summaries, witnesses) = overlap_fixture(
        "Given('value {colour}', () => colour());\n\
         Given(/^value red$/, () => regex());\n\
         Given('value {colour}', () => duplicate());",
    );
    let complete = BTreeMap::from([("value red", vec![0, 1, 2])]);

    assert_eq!(
        cached_overlap_proposals(&definitions, &summaries, &witnesses, 1, &complete),
        Some(vec![vec![1]])
    );
    let cached = collect_overlap_proposals(
        &definitions,
        &summaries,
        &config.parameter_types,
        1,
        &witnesses,
        1,
        Some(&complete),
    );
    let compiled = collect_overlap_proposals(
        &definitions,
        &summaries,
        &config.parameter_types,
        1,
        &witnesses,
        1,
        None,
    );
    assert_eq!(cached.0, compiled.0);
    assert!(!cached.1 && !compiled.1);
    assert_eq!(cached.2, OverlapProposalSource::FeatureCache);
    assert_eq!(compiled.2, OverlapProposalSource::CompiledWindows);
    assert_eq!(
        cached_overlap_proposals(&definitions, &summaries, &witnesses, 0, &complete),
        None,
        "an over-budget cache must preserve the compiled path's partial ordering"
    );
    assert_eq!(
        collect_overlap_proposals(
            &definitions,
            &summaries,
            &config.parameter_types,
            1,
            &witnesses,
            0,
            Some(&complete),
        )
        .2,
        OverlapProposalSource::CompiledWindows
    );
    assert_eq!(
        cached_overlap_proposals(&definitions, &summaries, &witnesses, 1, &BTreeMap::new()),
        None,
        "an unobserved witness must still scan every matcher window"
    );
    assert_eq!(
        collect_overlap_proposals(
            &definitions,
            &summaries,
            &config.parameter_types,
            1,
            &witnesses,
            1,
            Some(&BTreeMap::new()),
        )
        .2,
        OverlapProposalSource::CompiledWindows
    );
}

/// A `Break` from the visitor ends the window walk: no later window is compiled or visited.
#[test]
fn scan_windows_stop_at_a_break() {
    let definitions = ["one", "two", "three"].map(|matcher| StepDefinition {
        matcher: matcher.to_owned(),
        normalized_matcher: matcher.to_owned(),
        matcher_kind: MatcherKind::CucumberExpression,
        matcher_flags: String::new(),
        handler: crate::model::HandlerFingerprint {
            comparable: false,
            trivial: true,
            effect_stream: Default::default(),
            capture_scope: None,
            source_snippet: String::new(),
            behavior_signature: Vec::new(),
            structural: String::new(),
            alpha_normalized: String::new(),
            normalized: String::new(),
            exact: String::new(),
        },
        framework: crate::model::Framework::UNKNOWN,
        analysis_policy: Default::default(),
        registration: "Given".to_owned(),
        location: crate::model::SourceLocation::new("synthetic", 1, 1, 1, 1),
        inline_suppressions: Vec::new(),
    });
    let (_directory, config) = config();
    let mut visited = Vec::new();
    let work = scan_windows(
        &definitions,
        &config.parameter_types,
        1,
        |start, _, _, _| {
            visited.push(start);
            if start == 1 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        },
    );
    assert_eq!(visited, vec![0, 1]);
    // Only the two visited windows are compiled and charged.
    let whole = scan_windows(&definitions, &config.parameter_types, 1, |_, _, _, _| {
        ControlFlow::Continue(())
    });
    assert_eq!((work, whole), (2, 3));
}

/// Checks that two directional witnesses evaluate their shared overlap pair once without exhausting its budget.
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

/// Checks that overlap findings stop at the report cap and leave an explicit incomplete-analysis diagnostic.
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

/// Checks that splitting an oversized combined regex set preserves the matching definition index.
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

/// Checks that a rejected combined pattern falls back to its valid individual regex.
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

/// Checks membership intersections distinguish shared ambiguity groups from disjoint groups.
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

/// Checks repeated feature text marks both definitions used and retains the configured ambiguity suppression.
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

/// Checks disabled overlap and unused rules emit nothing, and used definitions stay exempt when unused reporting is enabled.
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

/// Checks unresolved parameter types and unsupported regex flags cannot produce unused-definition findings.
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

/// Checks witness generation handles built-ins and literal alternatives while withholding unsupported parameter samples.
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
        ("(?m:red|green)", Some("red")),
        ("(?ms:red|green)", Some("red")),
        ("((red|green))", Some("red")),
        ("(red)|(green)", None),
        ("", None),
        ("red.*", None),
        ("(red", None),
    ] {
        for framework in [
            crate::typescript::frameworks::CUCUMBER_JS,
            crate::ruby::CUCUMBER_RUBY,
        ] {
            crate::analysis::tests::set_framework(&mut definitions[0], framework);
            assert_eq!(
                definitions[0]
                    .analysis_profile()
                    .dialect()
                    .parameter_sample(pattern)
                    .as_deref(),
                expected,
                "{framework:?}/{pattern}"
            );
        }
    }
    assert_eq!(witness_literal(r"a\(b) c/d"), "a(b) c");
    assert_eq!(witness_literal("open(unclosed"), "open(unclosed");
}

/// Checks fallback expressions handle optional text, alternatives, declared parameters, and approximate unknown parameters.
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

/// Checks oversized regex and Cucumber matchers report resource limits while invalid regex syntax has no resource diagnostic.
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
