use super::feature_matches::MatchRow;
use super::*;

mod properties;
use crate::config::{ConfigOverrides, SuppressionConfig};
use crate::discovery::SourceFile;
use crate::typescript::semantics::default_policy;
use crate::typescript::TYPESCRIPT;
use std::path::{Path, PathBuf};

struct UsageLimits {
    ambiguity: AmbiguityBudget,
    matching: MatchingBudget,
    cache: usize,
    row: usize,
}

/// Runs final usage and overlap outcomes under explicit test-local allowances.
fn limited_usage(
    definitions: &[StepDefinition],
    steps: &[FeatureStep],
    config: &Config,
    limits: UsageLimits,
    window: usize,
) -> (Vec<Finding>, FeatureUsageOutcome) {
    let suppressions = SuppressionIndex::new(config, definitions);
    let mut findings = Vec::new();
    let outcome = analyze_feature_usage_with_budget(
        definitions,
        steps,
        config,
        &suppressions,
        &mut findings,
        100,
        window,
        limits.ambiguity,
        limits.matching,
        limits.cache,
        limits.row,
    );
    (findings, outcome)
}

/// Runs final usage/overlap outcomes with test-local ambiguity admission limits.
fn limited_ambiguities(
    definitions: &[StepDefinition],
    steps: &[FeatureStep],
    config: &Config,
    limits: [usize; 3],
    window: usize,
) -> (Vec<Finding>, FeatureUsageOutcome) {
    limited_usage(
        definitions,
        steps,
        config,
        UsageLimits {
            ambiguity: AmbiguityBudget::new(limits[0], limits[1], limits[2]),
            matching: MatchingBudget::default(),
            cache: MAX_DIRECT_MATCH_CACHE_BYTES,
            row: MAX_ROW_MATCHES,
        },
        window,
    )
}

/// Runs final outcomes with test-local direct scan, match, cache and row allowances.
fn limited_matching(
    definitions: &[StepDefinition],
    steps: &[FeatureStep],
    config: &Config,
    limits: [usize; 4],
    window: usize,
) -> (Vec<Finding>, FeatureUsageOutcome) {
    limited_usage(
        definitions,
        steps,
        config,
        UsageLimits {
            ambiguity: AmbiguityBudget::default(),
            matching: MatchingBudget::new(limits[0], limits[1]),
            cache: limits[2],
            row: limits[3],
        },
        window,
    )
}

/// Runs one constrained direct-match case beside its unlimited final-outcome control.
fn limited_matching_with_control(
    definitions: &[StepDefinition],
    steps: &[FeatureStep],
    config: &Config,
    limits: [usize; 4],
    window: usize,
) -> (
    (Vec<Finding>, FeatureUsageOutcome),
    (Vec<Finding>, FeatureUsageOutcome),
) {
    (
        limited_matching(definitions, steps, config, [usize::MAX; 4], window),
        limited_matching(definitions, steps, config, limits, window),
    )
}

/// Independently sums every retained ambiguity text and native path field from the final API result.
fn ambiguity_payload(findings: &[Finding]) -> usize {
    findings
        .iter()
        .map(|f| {
            f.message.len()
                + f.primary.path.as_os_str().len()
                + f.related
                    .iter()
                    .map(|l| l.path.as_os_str().len())
                    .sum::<usize>()
                + f.evidence.matcher_difference.len()
                + f.evidence.handler_evidence.len()
                + f.suggested_action.len()
                + f.suppression.as_ref().map_or(0, |s| s.reason.len())
        })
        .sum()
}

/// Covers exact/one-over membership, source-group and payload quotas without losing usage or suppression.
#[test]
fn ambiguity_admission_boundary_matrix_preserves_complete_groups() {
    let definitions = definitions("Given(/^same$/,()=>first()); Given('same',()=>second());");
    let mut steps = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given same\n  And same\n",
        Path::new("a.feature"),
    )
    .unwrap();
    steps[1].location.path = PathBuf::from("b.feature");
    for suppressed in [false, true] {
        let (_root, mut config) = config();
        config
            .rules
            .insert(Rule::OverlappingMatcher, Severity::Warning);
        if suppressed {
            config.suppressions.push(SuppressionConfig {
                rule: Rule::AmbiguousStep,
                reason: "known 🌍".into(),
                path: None,
                matcher: None,
            });
        }
        for window in [1, 250] {
            let (full, complete) =
                limited_ambiguities(&definitions, &steps, &config, [12, 2, usize::MAX], window);
            assert!(complete.incomplete.is_empty());
            assert_eq!(full.len(), 2);
            let bytes = ambiguity_payload(&full);
            for (slots, groups, payload, retained) in [
                (12, 2, bytes, 2),
                (11, 2, bytes, 1),
                (12, 1, bytes, 1),
                (12, 2, bytes - 1, 1),
                (0, 2, bytes, 0),
                (12, 2, 0, 0),
            ] {
                let (findings, outcome) = limited_ambiguities(
                    &definitions,
                    &steps,
                    &config,
                    [slots, groups, payload],
                    window,
                );
                assert_eq!(
                    findings.len(),
                    retained,
                    "{slots}/{groups}/{payload}/{window}"
                );
                assert_eq!(
                    serde_json::to_value(&findings).unwrap(),
                    serde_json::to_value(&full[..retained]).unwrap()
                );
                assert_eq!(outcome.used, [0, 1].into_iter().collect());
                assert_eq!(outcome.incomplete.is_empty(), retained == 2);
                if retained < 2 {
                    assert_eq!(outcome.overlap_census, CandidateSourceCensus::default());
                }
                assert!(findings.iter().all(|f| f.rule == Rule::AmbiguousStep
                    && f.related.len() == 2
                    && f.suppression.is_some() == suppressed));
            }
        }
    }
}

/// Completes later expansions of accepted member sets after a different set at the same source is refused.
#[test]
fn ambiguity_admission_finishes_accepted_outline_counts_after_refusal() {
    let definitions = definitions("Given(/^value (red|blue)$/,()=>first()); Given('value red',()=>second()); Given('value blue',()=>third());");
    let steps = crate::gherkin::extract("Feature: F\n Scenario Outline: S\n  Given value <v>\n Examples:\n | v |\n | red |\n | blue |\n | red |\n | blue |\n", Path::new("outline.feature")).unwrap();
    let (_root, config) = config();
    let (full, complete) =
        limited_ambiguities(&definitions, &steps, &config, [100, 2, usize::MAX], 1);
    assert!(complete.incomplete.is_empty());
    assert_eq!(full.len(), 2);
    let (partial, outcome) =
        limited_ambiguities(&definitions, &steps, &config, [8, 1, usize::MAX], 1);
    assert_eq!(partial.len(), 1);
    assert_eq!(
        serde_json::to_value(&partial[0]).unwrap(),
        serde_json::to_value(&full[0]).unwrap()
    );
    assert!(partial[0].message.contains("2 ambiguous expansions"));
    assert_eq!(
        crate::modes::finding_fingerprint(&partial[0]),
        crate::modes::finding_fingerprint(&full[0])
    );
    assert!(!outcome.incomplete.is_empty());
    assert_eq!(outcome.used, [0, 1, 2].into_iter().collect());
}

/// Keeps disabled ambiguity independent of its quota and uncertain matchers indeterminate after a cut.
#[test]
fn ambiguity_admission_preserves_disabled_overlap_and_uncertain_usage_controls() {
    let definitions = definitions("Given(/^same$/,()=>first()); Given('same',()=>second()); Given('value {unknown}',()=>third()); Given('unused',()=>fourth());");
    let steps = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given same\n",
        Path::new("input.feature"),
    )
    .unwrap();
    let (_root, mut config) = config();
    config
        .rules
        .insert(Rule::OverlappingMatcher, Severity::Warning);
    config
        .rules
        .insert(Rule::UnusedDefinition, Severity::Warning);
    let (mut findings, outcome) = limited_ambiguities(&definitions, &steps, &config, [0, 0, 0], 1);
    assert!(findings.is_empty());
    assert!(!outcome.incomplete.is_empty());
    assert_eq!(outcome.used, [0, 1, 2].into_iter().collect());
    let suppressions = SuppressionIndex::new(&config, &definitions);
    analyze_unused(
        &definitions,
        &outcome.used,
        &config,
        &suppressions,
        &mut findings,
    );
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].primary, definitions[3].location);
    config.rules.insert(Rule::AmbiguousStep, Severity::Off);
    let (findings, outcome) = limited_ambiguities(&definitions[..2], &steps, &config, [0, 0, 0], 1);
    assert!(outcome.incomplete.is_empty());
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, Rule::OverlappingMatcher);
}

/// Includes long Unicode paths in exact payload admission before preserving their original locations.
#[test]
fn ambiguity_admission_counts_original_path_bytes() {
    let paths = [
        PathBuf::from("長い🌍".repeat(200)),
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0xff, b'y']))
        },
    ];
    for path in paths {
        let mut definitions = definitions("Given('same',()=>first()); Given('same',()=>second());");
        for d in &mut definitions {
            d.location.path = path.clone();
        }
        let mut steps = crate::gherkin::extract(
            "Feature: F\n Scenario: S\n  Given same\n",
            Path::new("input.feature"),
        )
        .unwrap();
        steps[0].location.path = path;
        let (_root, config) = config();
        let (full, _) = limited_ambiguities(&definitions, &steps, &config, [8, 1, usize::MAX], 1);
        assert_eq!(full.len(), 1);
        let bytes = ambiguity_payload(&full);
        for (limit, expected) in [(bytes, 1), (bytes - 1, 0)] {
            let (findings, outcome) =
                limited_ambiguities(&definitions, &steps, &config, [8, 1, limit], 1);
            assert_eq!(findings.len(), expected);
            assert_eq!(outcome.incomplete.is_empty(), expected == 1);
            if expected == 1 {
                assert_eq!(findings[0].primary, steps[0].location);
                assert_eq!(findings[0].related[0], definitions[0].location);
            }
        }
    }
}

/// Refuses arithmetic overflow atomically so a later fitting charge still consumes the exact allowance.
#[test]
fn ambiguity_admission_arithmetic_refusal_is_atomic() {
    let mut budget = AmbiguityBudget::new(8, 1, 1);
    assert!(!budget.admit_group(usize::MAX, 1));
    assert!(!budget.admit_group(usize::MAX / 2 + 1, 0));
    assert!(budget.admit_group(2, 2));
    assert!(!budget.admit_group(0, 0));
    assert!(!budget.admit_payload([usize::MAX, 1]));
    assert!(budget.admit_payload([1]));
    assert!(!budget.admit_payload([1]));
}

/// Preserves complete JS/Ruby ambiguity groups, outline counts, suppression and window-independent identity.
#[test]
fn ambiguity_admission_preserves_supported_outcome_matrix() {
    for language in [TYPESCRIPT, crate::ruby::RUBY] {
        let source = if language == TYPESCRIPT {
            "import { Given } from '@cucumber/cucumber';\nGiven(/^value (red|blue)$/,()=>first());\nGiven('value red',()=>second());\nGiven('value blue',()=>third());"
        } else {
            "Given(/^value (red|blue)$/) { first }; Given('value red') { second }; Given('value blue') { third }"
        };
        let file = SourceFile {
            path: PathBuf::from(if language == TYPESCRIPT {
                "steps.ts"
            } else {
                "steps.rb"
            }),
            language,
        };
        let definitions = crate::source_adapter::adapter_for_language(language)
            .unwrap()
            .extract(source, &file)
            .unwrap()
            .definitions;
        assert_eq!(definitions.len(), 3);
        for (feature, expansions) in [
            ("Feature: F\n Scenario: S\n  Given value red\n  And value blue\n", [false, false]),
            ("Feature: F\n Scenario Outline: S\n  Given value <v>\n Examples:\n | v |\n | red |\n | blue |\n | red |\n", [true, false]),
        ] {
            let steps = crate::gherkin::extract(feature, Path::new("input.feature")).unwrap();
            for suppressed in [false, true] {
                let (_root, mut config) = config();
                if suppressed { config.suppressions.push(SuppressionConfig { rule: Rule::AmbiguousStep, reason: "owned overlap".into(), path: None, matcher: None }); }
                let mut reference = None;
                for window in [1, 2, 250] {
                    let suppressions = SuppressionIndex::new(&config, &definitions);
                    let mut findings = Vec::new();
                    let outcome = analyze_feature_usage_in_windows(&definitions, &steps, &config, &suppressions, &mut findings, 100, window);
                    assert!(outcome.incomplete.is_empty(), "{:?}", outcome.incomplete);
                    assert_eq!(outcome.used, [0, 1, 2].into_iter().collect());
                    assert_eq!(findings.len(), 2);
                    for (index, finding) in findings.iter().enumerate() {
                        assert_eq!(finding.rule, Rule::AmbiguousStep);
                        assert_eq!(finding.related, vec![definitions[0].location.clone(), definitions[index + 1].location.clone()]);
                        assert_eq!(finding.message.contains("2 ambiguous expansions"), expansions[index]);
                        assert_eq!(finding.suppression.is_some(), suppressed);
                    }
                    let snapshot = serde_json::to_value(&findings).unwrap();
                    let fingerprints = findings.iter().map(crate::modes::finding_fingerprint).collect::<Vec<_>>();
                    if let Some(expected) = &reference { assert_eq!(&(snapshot, fingerprints), expected); }
                    else { reference = Some((snapshot, fingerprints)); }
                }
            }
        }
    }
}

/// Requires the default ambiguity group ceiling to retain complete findings and expose an incomplete API result.
#[test]
fn ambiguity_admission_default_group_limit_is_explicit() {
    let definitions = definitions("Given('same',()=>first()); Given('same',()=>second());");
    let template = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given same\n",
        Path::new("input.feature"),
    )
    .unwrap()
    .remove(0);
    let steps: Vec<_> = (0..25_001)
        .map(|index| {
            let mut step = template.clone();
            step.location.line = index + 1;
            step
        })
        .collect();
    let (_root, config) = ambiguity_only_config();
    let outcome =
        crate::analysis::analyze_with_diagnostics(definitions.clone(), steps.clone(), &config)
            .unwrap();
    assert_eq!(outcome.result.findings.len(), 25_000);
    assert!(outcome
        .result
        .findings
        .iter()
        .all(|finding| finding.related.len() == 2));
    assert!(outcome
        .incomplete
        .iter()
        .any(|message| message.contains("ambiguity")));
    assert!(crate::analysis::analyze(definitions, steps, &config).is_err());
}

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
    let mut summaries = Vec::new();
    scan_windows(
        definitions,
        &config.parameter_types,
        definitions.len().max(1),
        |_, matchers, _, _| {
            summaries.extend(matchers.iter().map(MatcherSummary::of));
            ControlFlow::Continue(())
        },
    );
    summaries
}

/// Creates a default configuration and returns its temporary directory to keep the root alive.
fn config() -> (tempfile::TempDir, Config) {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    (directory, config)
}

/// Enables only ambiguity reporting for tests that exercise its capacity.
fn ambiguity_only_config() -> (tempfile::TempDir, Config) {
    let (directory, mut config) = config();
    for &rule in Rule::ALL {
        config.rules.insert(rule, Severity::Off);
    }
    config.rules.insert(Rule::AmbiguousStep, Severity::Warning);
    (directory, config)
}

/// Runs static overlap with an empty feature corpus and returns findings with its budget census.
fn static_overlap(
    definitions: &[StepDefinition],
    config: &Config,
    budget: usize,
) -> (Vec<Finding>, FeatureUsageOutcome) {
    let suppressions = SuppressionIndex::new(config, definitions);
    let mut findings = Vec::new();
    let outcome = analyze_feature_usage(
        definitions,
        &[],
        config,
        &suppressions,
        &mut findings,
        budget,
    );
    (findings, outcome)
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

/// A dialect that passes the matcher through unchanged, so the shared compiler's limits apply to
/// caller-built text with no adapter translation.
struct IdentityDialect;

impl crate::source_adapter::semantics::RegexDialect for IdentityDialect {
    /// Returns the matcher as the translation body, ignoring flags.
    fn translate(&self, matcher: &str, _: &str) -> Option<crate::matcher::TranslatedRegex> {
        Some(crate::matcher::TranslatedRegex::new("", matcher))
    }
    /// Keys on the canonical source pattern.
    fn regex_key(&self) -> crate::matcher::RegexKey {
        crate::matcher::RegexKey::CanonicalSource
    }
    /// Every flag counts toward identity.
    fn identity_flags(&self) -> crate::matcher::IdentityFlags {
        crate::matcher::IdentityFlags::All
    }
    /// Allows similarity comparisons regardless of flags.
    fn similarity_flags_match(&self, _: &str, _: &str) -> bool {
        true
    }
}

static IDENTITY_DOMAIN: crate::source_adapter::semantics::DomainSpec =
    crate::source_adapter::semantics::DomainSpec::new("identity", &IdentityDialect);

/// Checks a regex at the pattern-byte limit compiles and one byte over is rejected.
#[test]
fn regex_pattern_limit_is_inclusive() {
    use crate::source_adapter::semantics::{HandlerScope, StepMatching, StreamEvidence};
    // The extracted definition is a scaffold: the test sets every field `prepare_matcher` reads.
    let mut definition = definitions("Given(/x/, () => work());").remove(0);
    definition.location = crate::model::SourceLocation::new("limit.txt", 3, 5, 3, 6);
    definition.matcher_kind = MatcherKind::RegularExpression;
    definition.matcher_flags = String::new();
    definition.analysis_policy = crate::source_adapter::AnalysisPolicy::new(
        &IDENTITY_DOMAIN,
        HandlerScope::Shared,
        StepMatching::ByText,
        StreamEvidence::Complete,
        false,
    );
    for (padding, within) in [(4, true), (3, false)] {
        definition.matcher = format!(
            "(?x){}",
            " ".repeat(MAX_REGEX_PATTERN_BYTES.saturating_sub(padding))
        );
        let (prepared, error) = prepare_matcher(&definition, &BTreeMap::new());
        assert_eq!(
            error.is_some_and(|message| message.contains("limit.txt:3:5")),
            !within,
            "{padding}"
        );
        assert_eq!(prepared.expression.is_some(), within, "{padding}");
    }
}

/// Checks that recording a broad ambiguity twice stores one group and one membership per definition.
#[test]
fn broad_ambiguity_groups_use_linear_membership_storage() {
    let matched: Arc<[usize]> = (0..10_000).collect::<Vec<_>>().into();
    let mut proven = ProvenAmbiguities::new(matched.len());

    proven.record(Arc::clone(&matched));
    proven.record(matched);

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
    let (_findings, outcome) = static_overlap(&definitions, &config, 1);

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
        None,
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
    let mut row = MatchRow::default();
    for index in 0..3 {
        row.append(index);
    }
    let mut complete = AmbiguityRows::new(["value red"], usize::MAX, usize::MAX);
    for index in row.indices() {
        assert_eq!(complete.append("value red", index), ScanStatus::Complete);
    }

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
        cached_overlap_proposals(
            &definitions,
            &summaries,
            &witnesses,
            1,
            &AmbiguityRows::new([], usize::MAX, usize::MAX),
        ),
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
            Some(&AmbiguityRows::new([], usize::MAX, usize::MAX)),
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
        analysis_policy: default_policy(),
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
    let (findings, outcome) = static_overlap(&definitions, &config, 30000);

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
            PreparedMatcher {
                expression: Some(expression),
                authoritative: true,
            }
        })
        .collect::<Vec<_>>();

    let index = MatcherIndex::build_with_limits(&compiled, 4 * 1024, 64 * 1024);
    assert!(index.sets.len() > 1);
    assert!(index.fallback.is_empty());

    let mut matches = Vec::new();
    index.matches("distinct matcher value 0123", &mut matches);
    assert_eq!(matches, vec![123]);
}

/// Checks each singleton-set failure by how the pattern compiles alone, at limits where the set
/// and the individual program disagree in both directions; a valid matcher after a rejected one
/// pins the rebuild.
///
/// Under a 64-byte limit `foo|bar|baz|qux` compiles alone (literal search) but not as a set. Under
/// 1,024 bytes nine capture groups fit only as a set, which compiles without captures, and so are
/// matched, not rejected. `(` compiles in no form; `\w{64}` exceeds the size limit in both.
#[test]
fn matcher_index_resolves_singleton_failures_by_the_individual_compile() {
    let prepared = |expressions: &[&str]| {
        expressions
            .iter()
            .map(|expression| PreparedMatcher {
                expression: Some((*expression).to_owned()),
                authoritative: true,
            })
            .collect::<Vec<_>>()
    };
    let mut matches = Vec::new();

    let index = MatcherIndex::build_with_limits(&prepared(&["foo|bar|baz|qux"]), 64, 64);
    assert!(index.sets.is_empty() && index.rejected.is_empty());
    assert_eq!(index.scan_work(), 1);
    index.matches("baz", &mut matches);
    assert_eq!(matches, [0]);

    let captures = "(x)".repeat(9);
    let index = MatcherIndex::build_with_limits(&prepared(&[&captures]), 1024, 1024);
    assert_eq!((index.sets.len(), index.fallback.len()), (1, 0));
    index.matches(&"x".repeat(9), &mut matches);
    assert_eq!(matches, [0]);

    let index = MatcherIndex::build_with_limits(
        &prepared(&["^a$", "^b$", "(", "^c$", r"\w{64}", "^d$"]),
        1024 * 1024,
        1024 * 1024,
    );
    assert_eq!(index.rejected, [(2, false), (4, true)]);
    assert_eq!(index.sets.len(), 1, "rebuilt without the rejected matchers");
    assert!(index.fallback.is_empty());
    index.matches("d", &mut matches);
    assert_eq!(matches, [5]);
}

/// Checks the window walk clears the expression of a matcher its index rejects for size and reports
/// the limit at the matcher's location, while invalid syntax is dropped before the index with no
/// diagnostic.
#[test]
fn rejected_matchers_lose_their_regex_and_report_only_the_size_limit() {
    let definitions = definitions(
        "Given(/^valid$/, () => work());\nGiven(/^(?:\\w{2}){100000}$/, () => work());\nGiven(/^[z-a]$/, () => work());",
    );
    let (_directory, config) = config();
    let mut outcome = Vec::new();
    scan_windows(
        &definitions,
        &config.parameter_types,
        3,
        |_, matchers, _, diagnostics| {
            outcome.extend(
                matchers
                    .iter()
                    .map(|matcher| matcher.expression.is_some())
                    .zip(diagnostics),
            );
            ControlFlow::Continue(())
        },
    );
    assert_eq!(outcome.len(), 3);
    assert_eq!(outcome[0], (true, None));
    assert!(!outcome[1].0);
    assert!(outcome[1]
        .1
        .as_deref()
        .is_some_and(|message| message.contains("steps.ts:2:1")));
    assert_eq!(outcome[2], (false, None));
    let prepared = |index: usize| {
        prepare_matcher(&definitions[index], &config.parameter_types)
            .0
            .expression
    };
    assert!(prepared(1).is_some(), "size is decided by the index");
    assert!(prepared(2).is_none(), "syntax is decided before it");
}

/// Checks membership intersections distinguish shared ambiguity groups from disjoint groups.
#[test]
fn ambiguity_membership_intersection_handles_ordered_disjoint_groups() {
    let mut proven = ProvenAmbiguities::new(5);
    proven.record(Arc::from([0, 2]));
    proven.record(Arc::from([1, 3]));
    proven.record(Arc::from([2, 4]));
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
        witness_for(&definitions[0], &BTreeMap::new(), &mut HashMap::new()).as_deref(),
        Some("I have 1 items in red")
    );
    assert!(witness_for(&definitions[1], &BTreeMap::new(), &mut HashMap::new()).is_none());
    definitions[0].matcher = "a {colour}".to_owned();
    assert!(witness_for(&definitions[0], &BTreeMap::new(), &mut HashMap::new()).is_none());
    // A sample is derived once per domain and type name: a shared memo answers from the first
    // pattern, where a fresh one derives from the second.
    let mut memo = HashMap::new();
    let first = BTreeMap::from([("colour".to_owned(), "red|green".to_owned())]);
    let second = BTreeMap::from([("colour".to_owned(), "blue|green".to_owned())]);
    assert_eq!(
        witness_for(&definitions[0], &first, &mut memo).as_deref(),
        Some("a red")
    );
    assert_eq!(
        witness_for(&definitions[0], &second, &mut memo).as_deref(),
        Some("a red")
    );
    assert_eq!(
        witness_for(&definitions[0], &second, &mut HashMap::new()).as_deref(),
        Some("a blue")
    );
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
        // An alternation of inline-flag groups: the first group, when the whole pattern takes it.
        ("(?:(?m:red)|(?ms:green))", Some("red")),
        ("(?:(?ms:red-1))", Some("red-1")),
        ("(?:(?m:red.*)|(?m:green))", None),
        ("(?:(?m:red)|(?m:green))(?:(?m:blue))", None),
        // Matches inside `red` as `ed`, so only an anchored check refuses it.
        ("(?:(?m:red)|(?m:e))(?:(?m:d))", None),
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
    // A caller dialect's default reads the same Rust regex forms.
    use crate::source_adapter::semantics::RegexDialect;
    assert_eq!(
        IdentityDialect
            .parameter_sample("(?:(?m:red)|(?ms:green))")
            .as_deref(),
        Some("red")
    );
    assert_eq!(
        IdentityDialect
            .parameter_sample("(?m:red|green)")
            .as_deref(),
        Some("red")
    );
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
    let (prepared, diagnostic) = prepare_matcher(&definition, &BTreeMap::new());
    assert!(prepared.expression.is_none());
    assert!(diagnostic.unwrap().contains("regex resource limit"));

    // Within the pattern limit, but the `(?i:...)` wrapper of the translation exceeds it.
    definition.matcher = "x".repeat(MAX_REGEX_PATTERN_BYTES);
    definition.matcher_flags = "i".to_owned();
    let (prepared, diagnostic) = prepare_matcher(&definition, &BTreeMap::new());
    assert!(prepared.expression.is_none());
    assert!(diagnostic.unwrap().contains("regex resource limit"));
    definition.matcher_flags = String::new();

    definition.matcher = "(".to_owned();
    let (_, diagnostic) = prepare_matcher(&definition, &BTreeMap::new());
    assert!(diagnostic.is_none());

    definition.matcher_kind = MatcherKind::CucumberExpression;
    definition.matcher = "x".repeat(MAX_REGEX_PATTERN_BYTES + 1);
    let (prepared, diagnostic) = prepare_matcher(&definition, &BTreeMap::new());
    assert!(prepared.expression.is_none());
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

/// Ordinary accepted matchers repeated across overlaps hit the evidence-byte ceiling before
/// the count ceiling, and the result reports that analysis is incomplete.
#[test]
fn overlap_evidence_byte_budget_marks_analysis_incomplete() {
    let prefix = "x".repeat(3_000);
    let source = (0..50)
        .map(|index| format!("Given('{prefix} {{type_{index}}}', () => action_{index}());\n"))
        .collect::<String>();
    let definitions = definitions(&source);
    let (_directory, mut config) = config();
    for index in 0..50 {
        config
            .parameter_types
            .insert(format!("type_{index}"), "red|green".to_owned());
    }
    let (findings, outcome) = static_overlap(&definitions, &config, 30000);
    assert!(!findings.is_empty());
    assert!(findings.len() < 50 * 49 / 2);
    assert_eq!(outcome.overlap_census.skipped, 1);
    assert!(outcome
        .incomplete
        .iter()
        .any(|message| message.contains("matcher-overlap analysis is incomplete")));
    let bytes: usize = findings
        .iter()
        .map(|finding| serde_json::to_vec(finding).unwrap().len())
        .sum();
    assert!(bytes <= MAX_FINDING_EVIDENCE_BYTES);
    assert!(
        bytes > MAX_FINDING_EVIDENCE_BYTES - 100_000,
        "{bytes} retained bytes"
    );
}

/// A legacy raw-text baseline suppresses one occurrence after sampling and model round trips,
/// while an additional occurrence and a literal presentation sample remain new findings.
fn assert_original_identity(finding: &Finding, message: String, difference: String) {
    use crate::modes::{apply_reference_baseline, finding_fingerprint};
    let mut legacy = finding.clone();
    legacy.original_fingerprint = None;
    legacy.message = message;
    legacy.evidence.matcher_difference = difference;
    if legacy.rule == Rule::OverlappingMatcher {
        let witness = legacy
            .message
            .split("both accept the step `")
            .nth(1)
            .unwrap()
            .strip_suffix('`')
            .unwrap();
        legacy.evidence.handler_evidence = format!(
            "No feature step exercises this overlap yet; `{witness}` would be ambiguous at runtime"
        );
    }
    let expected = finding_fingerprint(&legacy);
    assert_eq!(finding_fingerprint(finding), expected);
    let restored: Finding = serde_json::from_value(serde_json::to_value(finding).unwrap()).unwrap();
    let mut current = [restored.clone(), restored];
    let outcome = apply_reference_baseline(&mut current, &[legacy.clone()], "accepted").unwrap();
    assert_eq!((outcome.suppressed, outcome.new_findings), (1, 1));
    assert!(!current[0].is_active());
    assert!(current[1].is_active());
    let old_json = serde_json::to_value(&legacy).unwrap();
    assert!(old_json.get("original_fingerprint").is_none());
    let old_restored: Finding = serde_json::from_value(old_json).unwrap();
    assert_eq!(finding_fingerprint(&old_restored), expected);
    if finding.original_fingerprint.is_some() {
        let mut literal = finding.clone();
        literal.original_fingerprint = None;
        let outcome =
            apply_reference_baseline(std::slice::from_mut(&mut literal), &[legacy], "accepted")
                .unwrap();
        assert_eq!(outcome.new_findings, 1);
        assert!(literal.is_active());
    }
}

/// Short, large ASCII and large Unicode evidence preserve legacy identities for unused,
/// overlap, single-step ambiguity and repeated outline expansions.
#[test]
fn sampled_fallback_rules_preserve_legacy_baselines() {
    for (prefix, pattern, witness, values) in [
        (
            "value".to_owned(),
            "red|blue",
            "red",
            ["red".to_owned(), "blue".to_owned()],
        ),
        (
            "x".repeat(9_000),
            "red|blue",
            "red",
            ["red".to_owned(), "blue".to_owned()],
        ),
        (
            "界".repeat(3_000),
            "red|blue",
            "red",
            ["red".to_owned(), "blue".to_owned()],
        ),
        (
            "value".to_owned(),
            "x+",
            "x",
            ["x".repeat(9_000), "x".repeat(9_001)],
        ),
    ] {
        let left = format!("{prefix} {{first}}");
        let right = format!("{prefix} {{second}}");
        let source = format!(
            "Given({}, () => first());\nGiven({}, () => second());",
            serde_json::to_string(&left).unwrap(),
            serde_json::to_string(&right).unwrap()
        );
        let definitions = definitions(&source);
        let (_directory, mut config) = config();
        for name in ["first", "second"] {
            config
                .parameter_types
                .insert(name.to_owned(), pattern.to_owned());
        }
        config.rules.insert(Rule::UnusedDefinition, Severity::Error);
        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut unused = Vec::new();
        analyze_unused(
            &definitions,
            &BTreeSet::new(),
            &config,
            &suppressions,
            &mut unused,
        );
        assert_eq!(unused.len(), 2);
        for (finding, matcher) in unused.iter().zip([&left, &right]) {
            assert_original_identity(
                finding,
                format!("Step definition `{matcher}` is not used by the feature corpus"),
                "No discovered feature step matched this definition".to_owned(),
            );
        }
        let mut overlap = Vec::new();
        let outcome =
            analyze_feature_usage(&definitions, &[], &config, &suppressions, &mut overlap, 10);
        assert!(outcome.incomplete.is_empty());
        if pattern == "x+" {
            // Unbounded custom parameters have no static witness; concrete steps still match.
            assert!(overlap.is_empty());
        } else {
            assert_eq!(overlap.len(), 1);
            assert_eq!(overlap[0].rule, Rule::OverlappingMatcher);
            assert_original_identity(
                &overlap[0],
                format!(
                    "Matchers `{left}` and `{right}` both accept the step `{prefix} {witness}`"
                ),
                format!("`{left}` ↔ `{right}`"),
            );
        }
        for expansions in [1, 2] {
            let steps = values
                .iter()
                .take(expansions)
                .map(|color| FeatureStep {
                    keyword: "Given".to_owned(),
                    text: format!("{prefix} {color}"),
                    location: crate::model::SourceLocation::new("use.feature", 3, 1, 3, 20),
                })
                .collect::<Vec<_>>();
            let mut findings = Vec::new();
            let outcome = analyze_feature_usage(
                &definitions,
                &steps,
                &config,
                &suppressions,
                &mut findings,
                10,
            );
            assert!(outcome.incomplete.is_empty());
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].rule, Rule::AmbiguousStep);
            let message = if expansions == 1 {
                format!("Feature step `{}` matches 2 definitions", steps[0].text)
            } else {
                "Scenario outline step has 2 ambiguous expansions matching 2 definitions".to_owned()
            };
            assert_original_identity(&findings[0], message, format!("`{left}`, `{right}`"));
        }
    }
}

/// Omitting matcher-list members changes presentation only; a change after the displayed prefix
/// still produces a new ambiguous-step finding when compared with the original baseline.
#[test]
fn sampled_ambiguity_members_keep_original_identity() {
    let (_directory, mut config) = config();
    for index in 0..21 {
        config
            .parameter_types
            .insert(format!("type_{index}"), "red".to_owned());
    }
    let mut variants = Vec::new();
    for last in [19, 20] {
        let matchers = (0..20)
            .map(|index| format!("value {{type_{}}}", if index == 19 { last } else { index }))
            .collect::<Vec<_>>();
        let source = matchers
            .iter()
            .map(|matcher| {
                format!(
                    "Given({}, () => action());\n",
                    serde_json::to_string(matcher).unwrap()
                )
            })
            .collect::<String>();
        let definitions = definitions(&source);
        let suppressions = SuppressionIndex::new(&config, &definitions);
        let steps = [FeatureStep {
            keyword: "Given".to_owned(),
            text: "value red".to_owned(),
            location: crate::model::SourceLocation::new("use.feature", 3, 1, 3, 20),
        }];
        let mut findings = Vec::new();
        analyze_feature_usage(
            &definitions,
            &steps,
            &config,
            &suppressions,
            &mut findings,
            500,
        );
        assert_eq!(findings.len(), 1);
        assert_original_identity(
            &findings[0],
            "Feature step `value red` matches 20 definitions".to_owned(),
            matchers
                .iter()
                .map(|matcher| format!("`{matcher}`"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        variants.push(findings.remove(0));
    }
    let mut changed = [variants[1].clone()];
    let outcome =
        crate::modes::apply_reference_baseline(&mut changed, &variants[..1], "accepted").unwrap();
    assert_eq!(outcome.new_findings, 1);
    assert!(changed[0].is_active());
}

/// A late ambiguity survives a matcher window boundary, and disabling its consumers skips direct usage.
#[test]
fn late_feature_match_preserves_each_enabled_consumer() {
    let mut source = String::new();
    for index in 0..63 {
        source.push_str(&format!("Given('unseen {index}', () => work());\n"));
    }
    source.push_str("Given('last {int}', () => first());\nGiven(/^last [0-9]+$/, () => second());");
    let definitions = definitions(&source);
    let steps = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given unrelated\n  And last 1\n",
        Path::new("late.feature"),
    )
    .unwrap();
    let (_root, mut config) = config();
    for mask in 0..8 {
        for (bit, rule) in [
            (1, Rule::UnusedDefinition),
            (2, Rule::AmbiguousStep),
            (4, Rule::OverlappingMatcher),
        ] {
            config.rules.insert(
                rule,
                if mask & bit != 0 {
                    Severity::Warning
                } else {
                    Severity::Off
                },
            );
        }
        for window in [1, 64, 250] {
            let (mut findings, outcome) =
                limited_ambiguities(&definitions, &steps, &config, [usize::MAX; 3], window);
            analyze_unused(
                &definitions,
                &outcome.used,
                &config,
                &SuppressionIndex::new(&config, &definitions),
                &mut findings,
            );
            assert!(outcome.incomplete.is_empty());
            let unused: Vec<_> = findings
                .iter()
                .filter(|f| f.rule == Rule::UnusedDefinition)
                .map(|f| f.primary.line)
                .collect();
            assert_eq!(
                unused,
                if mask & 1 != 0 {
                    (1..=63).collect::<Vec<_>>()
                } else {
                    vec![]
                }
            );
            let ambiguous: Vec<_> = findings
                .iter()
                .filter(|f| f.rule == Rule::AmbiguousStep)
                .collect();
            assert_eq!(ambiguous.len(), usize::from(mask & 2 != 0));
            if let Some(finding) = ambiguous.first() {
                assert_eq!(finding.primary.line, 4);
                assert_eq!(
                    finding.related.iter().map(|l| l.line).collect::<Vec<_>>(),
                    [64, 65]
                );
            }
            assert_eq!(
                findings
                    .iter()
                    .filter(|f| f.rule == Rule::OverlappingMatcher)
                    .count(),
                usize::from(mask & 6 == 4)
            );
            if mask & 3 == 0 {
                assert!(
                    outcome.used.is_empty(),
                    "inactive feature consumers retained direct usage"
                );
            }
        }
    }
}

/// Exact scan and match budgets retain the late finding; one-under limits fail closed.
#[test]
fn direct_matching_budget_boundaries_preserve_only_complete_outcomes() {
    let definitions = definitions(
        "Given('only',()=>first()); Given(/^late$/,()=>second()); Given('late',()=>third());",
    );
    let steps = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given only\n  And late\n",
        Path::new("late.feature"),
    )
    .unwrap();
    let (_root, mut config) = config();
    for &rule in Rule::ALL {
        config.rules.insert(rule, Severity::Off);
    }
    for rule in [
        Rule::AmbiguousStep,
        Rule::UnusedDefinition,
        Rule::OverlappingMatcher,
    ] {
        config.rules.insert(rule, Severity::Warning);
    }

    let (findings, complete) = limited_matching(
        &definitions,
        &steps,
        &config,
        [6, 3, usize::MAX, usize::MAX],
        1,
    );
    assert!(complete.incomplete.is_empty());
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, Rule::AmbiguousStep);
    assert_eq!(findings[0].primary.line, 4);
    assert_eq!(complete.used, (0..3).collect());

    for limits in [
        [5, usize::MAX, usize::MAX, usize::MAX],
        [usize::MAX, 2, usize::MAX, usize::MAX],
    ] {
        let (findings, outcome) = limited_matching(&definitions, &steps, &config, limits, 1);
        assert!(findings.is_empty(), "partial matching produced a finding");
        assert_eq!(outcome.used, (0..3).collect());
        assert!(outcome.ambiguity_truncated);
        assert_eq!(outcome.overlap_census, CandidateSourceCensus::default());
        assert!(outcome
            .incomplete
            .iter()
            .any(|message| message.contains("direct feature matching")));
    }

    let mut matched_texts = FeatureMatches::for_texts(["only"], &config, usize::MAX, usize::MAX);
    let mut no_work = MatchingBudget::new(0, usize::MAX);
    let stopped = scan_feature_batch(&definitions, &config, 1, &mut matched_texts, &mut no_work);
    assert_eq!(stopped.status, ScanStatus::CpuExhausted);
    assert_eq!(stopped.summaries.len(), definitions.len());

    let (cache_findings, cache_outcome) = limited_matching(
        &definitions,
        &steps,
        &config,
        [usize::MAX, usize::MAX, 0, usize::MAX],
        1,
    );
    assert!(cache_findings.is_empty());
    assert!(cache_outcome
        .incomplete
        .iter()
        .any(|message| message.contains("matching cache")));

    let prepared: Vec<_> = definitions
        .iter()
        .map(|definition| prepare_matcher(definition, &config.parameter_types).0)
        .collect();
    let index = MatcherIndex::build(&prepared);
    let mut cache_blocked = FeatureMatches::for_texts(["only"], &config, 0, usize::MAX);
    let mut window_used = vec![false; prepared.len()];
    let mut local = Vec::new();
    let mut unlimited = MatchingBudget::default();
    assert_eq!(
        cache_blocked.scan(
            0,
            &prepared,
            &index,
            &mut window_used,
            &mut local,
            &mut unlimited,
        ),
        ScanStatus::CacheOverflow
    );

    let mut usage_config = config.clone();
    usage_config
        .rules
        .insert(Rule::AmbiguousStep, Severity::Off);
    usage_config
        .rules
        .insert(Rule::OverlappingMatcher, Severity::Off);
    let (_, usage_outcome) = limited_matching(
        &definitions,
        &steps,
        &usage_config,
        [0, usize::MAX, usize::MAX, usize::MAX],
        1,
    );
    assert_eq!(usage_outcome.used, (0..definitions.len()).collect());
    assert!(usage_outcome
        .incomplete
        .iter()
        .any(|message| message.contains("direct feature matching")));
}

/// Cache-driven recomputation retains findings, order, usage and completeness.
#[test]
fn matching_cache_batches_without_reducing_visibility() {
    let patterns =
        definitions("Given(/^(red|blue)$/,()=>first()); Given(/^never$/,()=>unreachable());");
    let mut definitions = vec![patterns[1].clone(); 66];
    definitions[0] = patterns[0].clone();
    definitions[65] = patterns[0].clone();
    for (line, definition) in definitions.iter_mut().enumerate() {
        definition.location.line = line + 1;
    }
    let steps = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given red\n  And blue\n",
        Path::new("colors.feature"),
    )
    .unwrap();
    let (_root, config) = ambiguity_only_config();
    let unlimited = [usize::MAX; 4];
    let (expected, expected_outcome) =
        limited_matching(&definitions, &steps, &config, unlimited, 1);
    let entry_bytes = std::mem::size_of::<(&str, MatchRow)>();
    let cache_limit = 2 * entry_bytes + std::mem::size_of::<usize>();
    let (actual, actual_outcome) = limited_matching(
        &definitions,
        &steps,
        &config,
        [usize::MAX, usize::MAX, cache_limit, usize::MAX],
        definitions.len(),
    );
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(actual_outcome.used, expected_outcome.used);
    assert_eq!(actual_outcome.incomplete, expected_outcome.incomplete);
    assert_eq!(
        actual_outcome.ambiguity_truncated,
        expected_outcome.ambiguity_truncated
    );
    assert_eq!(
        actual_outcome.overlap_census,
        expected_outcome.overlap_census
    );
    assert_eq!(actual.len(), 2);

    let (prefix, limited_outcome) = limited_matching(
        &definitions,
        &steps,
        &config,
        [2, usize::MAX, cache_limit, usize::MAX],
        definitions.len(),
    );
    assert_eq!(prefix.len(), 1, "one completed text batch remains visible");
    assert_eq!(prefix[0].primary.line, 3);
    assert!(limited_outcome.ambiguity_truncated);
    assert!(limited_outcome
        .incomplete
        .iter()
        .any(|message| message.contains("direct feature matching")));
}

/// A single-text cache overflow retains matcher diagnostics collected before the cutoff.
#[test]
fn matching_cache_single_text_overflow_keeps_compile_diagnostics() {
    let patterns = definitions(
        "Given(/^red$/,()=>first()); Given(/^never$/,()=>unreachable()); Given(/^red$/,()=>second());",
    );
    let mut definitions = vec![patterns[1].clone(); 66];
    definitions[0] = patterns[0].clone();
    definitions[1].matcher = "x".repeat(MAX_REGEX_PATTERN_BYTES + 1);
    definitions[65] = patterns[2].clone();
    let steps = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given red\n",
        Path::new("colors.feature"),
    )
    .unwrap();
    let (_root, config) = ambiguity_only_config();
    let cache_limit = std::mem::size_of::<(&str, MatchRow)>() + std::mem::size_of::<usize>();
    let (findings, outcome) = limited_matching(
        &definitions,
        &steps,
        &config,
        [usize::MAX, usize::MAX, cache_limit, usize::MAX],
        definitions.len(),
    );

    assert!(findings.is_empty());
    assert!(outcome
        .incomplete
        .iter()
        .any(|message| message.contains("regex resource limit")));
    assert!(outcome
        .incomplete
        .iter()
        .any(|message| message.contains("matching cache")));
}

/// Cache recomputation preserves source-order admission when texts recur across batches.
#[test]
fn matching_cache_batches_preserve_source_order_at_retention_limit() {
    let definitions = definitions(
        "Given(/^a$/,()=>a1()); Given(/^a$/,()=>a2()); Given(/^a$/,()=>a3()); Given(/^b$/,()=>b1()); Given(/^b$/,()=>b2());",
    );
    let steps = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given a\n  And b\n  And b\n  And a\n",
        Path::new("alternating.feature"),
    )
    .unwrap();
    let (_root, config) = ambiguity_only_config();
    let limits = |cache| UsageLimits {
        ambiguity: AmbiguityBudget::new(24, usize::MAX, usize::MAX),
        matching: MatchingBudget::default(),
        cache,
        row: MAX_ROW_MATCHES,
    };
    let (expected, expected_outcome) = limited_usage(
        &definitions,
        &steps,
        &config,
        limits(usize::MAX),
        definitions.len(),
    );
    let (actual, actual_outcome) = limited_usage(
        &definitions,
        &steps,
        &config,
        limits(std::mem::size_of::<(&str, MatchRow)>()),
        definitions.len(),
    );

    assert_eq!(
        expected
            .iter()
            .map(|finding| finding.primary.line)
            .collect::<Vec<_>>(),
        [3, 4, 5]
    );
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(actual_outcome.incomplete, expected_outcome.incomplete);
    assert_eq!(
        actual_outcome.ambiguity_truncated,
        expected_outcome.ambiguity_truncated
    );
}

/// Alternating between completed cache batches reuses each text without replay scans.
#[test]
fn matching_cache_replay_does_not_rescan_unrelated_texts() {
    let definitions = definitions("Given(/.*/,()=>first()); Given(/.*/,()=>second());");
    let mut source = "Feature: F\n Scenario: S\n".to_owned();
    for text in [
        "v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v0", "v4", "v0", "v4",
    ] {
        source.push_str(&format!("  Given {text}\n"));
    }
    let steps = crate::gherkin::extract(&source, Path::new("alternating.feature")).unwrap();
    let (_root, config) = ambiguity_only_config();
    let ((expected, expected_outcome), (actual, actual_outcome)) = limited_matching_with_control(
        &definitions,
        &steps,
        &config,
        [
            8,
            16,
            4 * std::mem::size_of::<(&str, MatchRow)>(),
            usize::MAX,
        ],
        definitions.len(),
    );

    assert_eq!(expected.len(), 12);
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(actual_outcome.incomplete, expected_outcome.incomplete);
    assert!(!actual_outcome.ambiguity_truncated);
}

/// Repeated text groups share one admitted member set, and refused groups are not cached.
#[test]
fn matching_cache_interns_only_admitted_ambiguity_groups() {
    let steps = [
        FeatureStep {
            keyword: "Given".to_owned(),
            text: "a".to_owned(),
            location: crate::model::SourceLocation::new("cache.feature", 3, 1, 3, 8),
        },
        FeatureStep {
            keyword: "Given".to_owned(),
            text: "b".to_owned(),
            location: crate::model::SourceLocation::new("cache.feature", 4, 1, 4, 8),
        },
        FeatureStep {
            keyword: "Given".to_owned(),
            text: "c".to_owned(),
            location: crate::model::SourceLocation::new("cache.feature", 5, 1, 5, 8),
        },
        FeatureStep {
            keyword: "Given".to_owned(),
            text: "d".to_owned(),
            location: crate::model::SourceLocation::new("cache.feature", 3, 1, 3, 8),
        },
    ];
    let texts = distinct_feature_texts(&steps);
    let mut rows = AmbiguityRows::new(texts.iter().copied(), usize::MAX, usize::MAX);
    for text in ["a", "b"] {
        for index in 0..100 {
            assert_eq!(rows.append(text, index), ScanStatus::Complete);
        }
    }
    for index in 100..200 {
        assert_eq!(rows.append("c", index), ScanStatus::Complete);
    }
    for index in 200..300 {
        assert_eq!(rows.append("d", index), ScanStatus::Complete);
    }
    let mut cache = AmbiguityCache::new(&texts);
    let mut budget = AmbiguityBudget::new(600, usize::MAX, usize::MAX);
    let mut proven = ProvenAmbiguities::new(300);
    let mut ambiguities = AmbiguityState::default();
    let mut cut = false;

    assert_eq!(
        collect_ambiguity_batch(
            &steps,
            Some(&rows),
            &mut cache,
            &mut budget,
            &mut proven,
            &mut ambiguities,
            &mut cut,
        ),
        steps.len()
    );
    assert!(ambiguities.incomplete_locations.is_empty());
    assert!(cut);
    let CachedAmbiguity::Ambiguous(a) = &cache.states[cache.offsets["a"]] else {
        panic!("first admitted group was not cached");
    };
    let CachedAmbiguity::Ambiguous(b) = &cache.states[cache.offsets["b"]] else {
        panic!("repeated admitted group was not cached");
    };
    assert!(Arc::ptr_eq(a, b));
    assert!(matches!(
        cache.states[cache.offsets["c"]],
        CachedAmbiguity::NonAmbiguous
    ));
    assert!(matches!(
        cache.states[cache.offsets["d"]],
        CachedAmbiguity::NonAmbiguous
    ));
    assert_eq!(proven.groups.len(), 1);
    assert!(Arc::ptr_eq(
        a,
        proven
            .groups
            .keys()
            .next()
            .expect("one admitted proof group")
    ));
    assert!(ambiguities
        .groups
        .values()
        .flat_map(BTreeMap::keys)
        .all(|members| Arc::ptr_eq(a, members)));
}

/// Runs one outline-location cutoff beside its unlimited final-outcome control.
fn outline_matching_cutoff_with_control(
    definitions: &[StepDefinition],
    texts: &[&str],
    config: &Config,
) -> (
    (Vec<Finding>, FeatureUsageOutcome),
    (Vec<Finding>, FeatureUsageOutcome),
) {
    let location = crate::model::SourceLocation::new("outline.feature", 3, 1, 3, 12);
    let steps = texts
        .iter()
        .map(|&text| FeatureStep {
            keyword: "Given".to_owned(),
            text: text.to_owned(),
            location: location.clone(),
        })
        .collect::<Vec<_>>();
    limited_matching_with_control(
        definitions,
        &steps,
        config,
        [
            1,
            usize::MAX,
            std::mem::size_of::<(&str, MatchRow)>(),
            usize::MAX,
        ],
        definitions.len(),
    )
}

/// A later CPU cutoff retains completed text groups but labels their outline count incomplete.
#[test]
fn matching_cutoff_marks_cached_outline_count_as_lower_bound() {
    let definitions = definitions("Given(/^red$/,()=>first()); Given(/^red$/,()=>second());");
    let (_root, config) = ambiguity_only_config();
    let ((expected, expected_outcome), (actual, actual_outcome)) =
        outline_matching_cutoff_with_control(&definitions, &["red", "blue", "red"], &config);

    assert!(expected_outcome.incomplete.is_empty());
    assert_eq!(expected.len(), 1);
    assert!(expected[0].message.contains("2 ambiguous expansions"));
    assert_eq!(actual.len(), 1);
    assert!(actual[0]
        .message
        .contains("at least 2 ambiguous expansions"));
    assert_ne!(
        crate::modes::finding_fingerprint(&actual[0]),
        crate::modes::finding_fingerprint(&expected[0])
    );
    let mut baseline_checked = actual.clone();
    let baseline =
        crate::modes::apply_reference_baseline(&mut baseline_checked, &expected, "accepted")
            .unwrap();
    assert_eq!(baseline.new_findings, 1);
    assert!(baseline_checked[0].is_active());
    assert!(actual_outcome.ambiguity_truncated);
    assert!(actual_outcome
        .incomplete
        .iter()
        .any(|message| message.contains("direct feature matching")));
}

/// Unresolved outline values never turn a retained lower bound into a complete-looking count.
#[test]
fn matching_cutoff_marks_plural_and_singular_outline_lower_bounds() {
    let definitions =
        definitions("Given(/^(red|blue)$/,()=>first()); Given(/^(red|blue)$/,()=>second());");
    let (_root, config) = ambiguity_only_config();

    for (texts, complete_count, partial_count) in [
        (&["red", "blue", "red"][..], 3, 2),
        (&["red", "blue"][..], 2, 1),
    ] {
        let ((complete, complete_outcome), (partial, partial_outcome)) =
            outline_matching_cutoff_with_control(&definitions, texts, &config);

        assert!(complete_outcome.incomplete.is_empty());
        assert_eq!(complete.len(), 1);
        assert_eq!(partial.len(), 1);
        assert!(complete[0]
            .message
            .contains(&format!("{complete_count} ambiguous expansions")));
        assert!(partial[0].message.contains(&format!(
            "at least {partial_count} ambiguous expansion{}",
            if partial_count == 1 { "" } else { "s" }
        )));
        assert!(partial_outcome.ambiguity_truncated);
        assert_ne!(
            crate::modes::finding_fingerprint(&partial[0]),
            crate::modes::finding_fingerprint(&complete[0])
        );
    }
}

/// A row too large for existing ambiguity admission cannot become a partial finding or proof.
#[test]
fn oversized_match_row_uses_existing_incomplete_contract() {
    let definitions = definitions(
        "Given(/^same$/,()=>first()); Given('same',()=>second()); Given(/same/,()=>third()); Given(/^same$/i,()=>fourth());",
    );
    let steps = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given same\n",
        Path::new("same.feature"),
    )
    .unwrap();
    let (_root, config) = ambiguity_only_config();
    let (complete, outcome) = limited_matching(
        &definitions,
        &steps,
        &config,
        [usize::MAX, usize::MAX, usize::MAX, 4],
        1,
    );
    assert!(outcome.incomplete.is_empty());
    assert_eq!(complete.len(), 1);
    assert_eq!(complete[0].related.len(), 4);

    let (partial, outcome) = limited_matching(
        &definitions,
        &steps,
        &config,
        [usize::MAX, usize::MAX, usize::MAX, 2],
        1,
    );
    assert!(partial.is_empty());
    assert!(outcome.ambiguity_truncated);
    assert_eq!(outcome.overlap_census, CandidateSourceCensus::default());
    assert!(outcome
        .incomplete
        .iter()
        .any(|message| message.contains("ambiguity retention")));
}

/// Compact rows preserve every matched index across gaps and keep the first block inline.
#[test]
fn match_rows_preserve_sparse_and_dense_blocks() {
    let mut row = MatchRow::default();
    assert_eq!(row.indices().count(), 0);
    for id in [60, 62, 63] {
        row.append(id);
    }
    assert!(row.is_inline());
    for id in 64..128 {
        row.append(id);
    }
    row.append(319);
    assert_eq!(
        row.indices().collect::<Vec<_>>(),
        [60, 62, 63]
            .into_iter()
            .chain(64..128)
            .chain([319])
            .collect::<Vec<_>>()
    );
}

/// Preserves the final ambiguity in both a large mostly-negative scan and a large singleton cache.
#[test]
fn large_feature_corpora_retain_the_only_late_ambiguity() {
    let patterns = definitions("Given(/^never$/,()=>first()); Given(/^value .*$/,()=>second()); Given(/^value zz-last$/,()=>third());");
    let template = crate::gherkin::extract(
        "Feature: F\n Scenario: S\n  Given value zz-last\n",
        Path::new("late.feature"),
    )
    .unwrap()
    .remove(0);
    let (_root, config) = ambiguity_only_config();
    for (misses, first_matcher, count, padding) in [
        (11_998, &patterns[2], 14_000, 245),
        (0, &patterns[1], 120_000, 0),
    ] {
        let mut registry = vec![patterns[0].clone(); misses];
        registry.extend([first_matcher.clone(), patterns[2].clone()]);
        for (id, def) in registry.iter_mut().enumerate() {
            def.location.line = id + 1;
        }
        let suffix = "x".repeat(padding);
        let steps: Vec<_> = (1..=count)
            .map(|line| {
                let mut step = template.clone();
                step.location.line = line;
                if line < count {
                    step.text = format!("value {line}{suffix}");
                }
                step
            })
            .collect();
        let (findings, outcome) = limited_ambiguities(
            &registry,
            &steps,
            &config,
            [usize::MAX; 3],
            DEFAULT_MATCHER_WINDOW,
        );
        assert!(outcome.incomplete.is_empty());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].primary.line, count);
        assert_eq!(
            findings[0]
                .related
                .iter()
                .map(|location| location.line)
                .collect::<Vec<_>>(),
            [misses + 1, misses + 2]
        );
    }
}

/// Isolated matches occupy one stored word each, including after a dense prefix.
#[test]
fn scattered_match_rows_preserve_indices_without_doubling_storage() {
    let width = usize::BITS as usize;
    let mut row = MatchRow::default();
    let ids: Vec<_> = (0..width).chain((1..=128).map(|i| i * 2 * width)).collect();
    for &id in &ids {
        row.append(id);
    }
    assert_eq!(row.indices().collect::<Vec<_>>(), ids);
    assert!(row.backing_bytes() >= 130 * std::mem::size_of::<usize>());
    assert!(row.backing_bytes() < 260 * std::mem::size_of::<usize>());
}
