use super::*;
use crate::display_text::DIAGNOSTIC_OMISSION;

mod properties;
use crate::typescript::TYPESCRIPT;

/// A clean finalization keeps definitions; a failed one clears them, zeroes the file count,
/// records the error, and leaves an incomplete run with no findings.
#[test]
fn finalization_failure_discards_provisional_definitions_and_reports_incompleteness() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::load(root.path(), ConfigOverrides::default()).unwrap();
    std::fs::write(root.path().join("steps.ts"), "import { Given, Then } from '@cucumber/cucumber'; Given('same', () => work()); Then('same', () => work());").unwrap();
    let files = discovery::discover(&config).unwrap();
    let mut diagnostics = discovery_diagnostics(&config, &files, &None);
    let mut corpus = extract_corpus(
        &config,
        &files,
        &None,
        &mut diagnostics,
        &mut gherkin::FeatureStepBudget::default(),
    );
    assert_eq!(corpus.definitions.len(), 2);
    let before = corpus.definitions.clone();
    collect_finalization(Ok(Default::default()), &mut corpus, &mut diagnostics);
    assert_eq!(corpus.definitions, before);
    collect_finalization(
        Err(anyhow::anyhow!("adapter unavailable")),
        &mut corpus,
        &mut diagnostics,
    );
    assert!(corpus.incomplete);
    assert!(corpus.definitions.is_empty());
    assert_eq!(corpus.definition_files_with_definitions, 0);
    assert!(diagnostics
        .errors
        .iter()
        .any(|e| e == "failed to finalize source evidence: adapter unavailable"));
    let analyzed = analyze_corpus(
        &config,
        corpus,
        &mut diagnostics,
        false,
        analysis::UNUSED_FINDING_LIMITS,
    )
    .unwrap();
    assert!(analyzed.run_incomplete);
    assert!(analyzed.result.findings.is_empty());
}

/// Color requires a TTY and is disabled by a non-empty `NO_COLOR`, but not by an empty one.
#[test]
fn terminal_color_requires_tty_and_allows_no_color_opt_out() {
    use std::ffi::OsStr;

    assert!(terminal_color_enabled(true, None));
    assert!(terminal_color_enabled(true, Some(OsStr::new(""))));
    assert!(!terminal_color_enabled(true, Some(OsStr::new("1"))));
    assert!(!terminal_color_enabled(false, None));
    assert!(!terminal_color_enabled(false, Some(OsStr::new(""))));
}

/// A known rule and severity parse; an unknown rule name is rejected.
#[test]
fn parses_rule_override() {
    assert_eq!(
        parse_rule_override("duplicate-matcher=warning"),
        Ok((Rule::DuplicateMatcher, Severity::Warning))
    );
    assert!(parse_rule_override("unknown=error").is_err());
}

/// Accepts a positive integer and rejects zero, a negative number and non-numeric text.
#[test]
fn candidate_limit_cli_values_must_be_positive() {
    assert_eq!(parse_positive_usize("7"), Ok(7));
    assert!(parse_positive_usize("0").is_err());
    assert!(parse_positive_usize("-1").is_err());
    assert!(parse_positive_usize("many").is_err());
}

/// An unparseable warning is an incomplete warning by default and an error under the flag; a
/// dynamic-matcher warning stays an incomplete warning, and an unrelated one stays complete.
#[test]
fn unparseable_diagnostics_are_tolerated_by_default_and_escalated_by_the_flag() {
    use crate::model::SourceLocation;
    use crate::source_adapter::{ExtractionDiagnostic, SourceFile};

    let directory = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: directory.path().join("steps.ts"),
        language: TYPESCRIPT,
    };
    let unparseable = || {
        ExtractionDiagnostic::new(
            ExtractionDiagnosticLevel::Warning,
            SourceLocation::new("steps.ts", 1, 1, 1, 1),
            format!(
                "{}, so analysis is incomplete",
                source_adapter::UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX
            ),
        )
    };
    let route = |config: &Config, diagnostic: ExtractionDiagnostic| {
        let mut diagnostics = Diagnostics::default();
        let mut incomplete = false;
        collect_extraction_diagnostics(
            config,
            &file,
            &None,
            vec![diagnostic],
            &mut diagnostics,
            &mut incomplete,
        );
        (diagnostics, incomplete)
    };

    let mut config =
        Config::load(directory.path(), ConfigOverrides::default()).expect("config loads");

    // Default: an unparseable file is a completeness warning, not an error.
    let (diagnostics, incomplete) = route(&config, unparseable());
    assert!(incomplete);
    assert!(diagnostics.errors.is_empty());
    assert_eq!(diagnostics.warnings.len(), 1);

    // `--fail-on-unparseable`: the same diagnostic becomes a hard error, still marking the
    // corpus incomplete.
    config.fail_on_unparseable = true;
    let (diagnostics, incomplete) = route(&config, unparseable());
    assert!(incomplete);
    assert_eq!(diagnostics.errors.len(), 1);
    assert!(diagnostics.warnings.is_empty());

    // An unresolved matcher is incomplete, but is not a syntax error. The syntax-only
    // flag must leave this diagnostic at warning severity.
    let dynamic = ExtractionDiagnostic::new(
        ExtractionDiagnosticLevel::Warning,
        SourceLocation::new("steps.ts", 2, 1, 2, 1),
        "dynamic or unsupported step matcher cannot be analyzed statically",
    );
    let (diagnostics, incomplete) = route(&config, dynamic);
    assert!(incomplete);
    assert!(diagnostics.errors.is_empty());
    assert_eq!(diagnostics.warnings.len(), 1);
    let unrelated = ExtractionDiagnostic::new(
        ExtractionDiagnosticLevel::Warning,
        SourceLocation::new("steps.ts", 2, 1, 2, 1),
        "unrelated extractor warning",
    );
    let (diagnostics, incomplete) = route(&config, unrelated);
    assert!(!incomplete);
    assert!(diagnostics.errors.is_empty());
    assert_eq!(diagnostics.warnings.len(), 1);
}

/// Routing follows the diagnostic kind, not its text: Incomplete and Unparseable kinds mark the
/// corpus incomplete, Error level always errors, and Unparseable warnings error only in strict
/// mode.
#[test]
fn typed_adapter_diagnostics_route_independently_of_language_and_wording() {
    use crate::model::SourceLocation;
    use crate::source_adapter::{
        ExtractionDiagnostic, ExtractionDiagnosticKind as Kind, SourceFile,
    };
    let directory = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: directory.path().join("steps.ts"),
        language: TYPESCRIPT,
    };
    let mut config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    for strict in [false, true] {
        config.fail_on_unparseable = strict;
        for (kind, message, incomplete, parse_error) in [
            (Kind::Other, "adapter advisory", false, false),
            (Kind::Incomplete, "external glue unavailable", true, false),
            (Kind::Unparseable, "adapter syntax failure", true, true),
            (
                Kind::Other,
                source_adapter::UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX,
                false,
                false,
            ),
            (
                Kind::Incomplete,
                source_adapter::UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX,
                true,
                false,
            ),
        ] {
            for level in [
                ExtractionDiagnosticLevel::Warning,
                ExtractionDiagnosticLevel::Error,
            ] {
                let mut diagnostics = Diagnostics::default();
                let mut actual_incomplete = false;
                collect_extraction_diagnostics(
                    &config,
                    &file,
                    &None,
                    vec![ExtractionDiagnostic::with_kind(
                        kind,
                        level,
                        SourceLocation::new(&file.path, 1, 1, 1, 1),
                        message,
                    )],
                    &mut diagnostics,
                    &mut actual_incomplete,
                );
                assert_eq!(actual_incomplete, incomplete, "{kind:?} strict={strict}");
                let error = level == ExtractionDiagnosticLevel::Error || (strict && parse_error);
                assert_eq!(diagnostics.errors.len(), usize::from(error));
                assert_eq!(diagnostics.warnings.len(), usize::from(!error));
            }
        }
    }
}

/// Names read as prose at every length.
#[test]
fn join_names_reads_as_prose() {
    for (names, joined) in [
        (&[][..], ""),
        (&["a"][..], "a"),
        (&["a", "b"][..], "a and b"),
        (&["a", "b", "c"][..], "a, b and c"),
    ] {
        assert_eq!(join_names(names), joined);
    }
}

/// Both `cuke-dedup PATH` and `cuke-dedup check PATH` parse; `check` without a path fails.
#[test]
fn supports_implicit_and_explicit_check_forms() {
    let implicit = Cli::try_parse_from(["cuke-dedup", ".", "--reporters", "json"])
        .expect("implicit invocation");
    assert!(implicit.command.is_none());
    let explicit = Cli::try_parse_from(["cuke-dedup", "check", ".", "--reporters", "json"])
        .expect("explicit invocation");
    assert!(matches!(explicit.command, Some(Command::Check { .. })));
    assert!(Cli::try_parse_from(["cuke-dedup", "check"]).is_err());
}

/// An adapter whose declarations carry no domain, as a caller's extractor leaves them.
struct Bare;

impl source_adapter::SourceAdapter for Bare {
    /// Returns the adapter name.
    fn name(&self) -> &'static str {
        "bare"
    }
    /// Returns the adapter name as its language.
    fn language(&self) -> source_adapter::SourceLanguage {
        source_adapter::SourceLanguage::new("bare")
    }
    /// Extracts one unattributed parameter-type declaration.
    fn extract(
        &self,
        _: &str,
        file: &source_adapter::SourceFile,
    ) -> anyhow::Result<source_adapter::Extraction> {
        let mut extraction = source_adapter::Extraction::new(Vec::new(), Vec::new());
        extraction.parameter_types = vec![source_adapter::SourceParameterType::new(
            Some("size".to_owned()),
            None,
            crate::model::SourceLocation::new(&file.path, 1, 1, 1, 2),
        )];
        Ok(extraction)
    }
    /// Compares in Ruby's domain, named "Ruby".
    fn analysis_policy(&self) -> source_adapter::AnalysisPolicy {
        crate::ruby::semantics::policy()
    }
}

static BARE: Bare = Bare;

/// The shared extraction loop attributes a declaration its extractor left unattributed to the
/// extracting adapter.
#[test]
fn extracted_parameter_types_are_attributed_to_the_extracting_adapter() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(".cuke-dedup.json"),
        r#"{"definitions":["*.bare"]}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("steps.bare"), "").unwrap();
    let registry =
        source_adapter::AdapterRegistry::new([source_adapter::SourceAdapterRegistration::new(
            ".bare", &BARE,
        )])
        .unwrap();
    let config =
        Config::load_with_adapters(root.path(), ConfigOverrides::default(), registry).unwrap();
    let files = discovery::discover(&config).unwrap();
    let mut diagnostics = discovery_diagnostics(&config, &files, &None);
    let corpus = extract_corpus(
        &config,
        &files,
        &None,
        &mut diagnostics,
        &mut gherkin::FeatureStepBudget::default(),
    );
    let domains: Vec<_> = corpus
        .parameter_types
        .iter()
        .map(|declaration| declaration.domain.display_name)
        .collect();
    assert_eq!(domains, ["Ruby"]);
}

/// A fully omitted operational error still fails the run and prints an error detail marker.
#[test]
fn retained_output_diagnostic_omission_preserves_failure_status() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let mut diagnostics = Diagnostics {
        errors: DiagnosticDetails::with_limits(0, 0),
        warnings: DiagnosticDetails::with_limits(1, 1024),
    };
    diagnostics.warnings.push("first warning".to_owned());
    diagnostics.warnings.push("another warning".to_owned());
    diagnostics.errors.push("cannot read input".to_owned());
    assert_eq!(
        determine_exit_code(
            &config,
            &CheckOptions::default(),
            &crate::model::AnalysisResult {
                definitions: Vec::new(),
                feature_steps: Vec::new(),
                findings: Vec::new(),
            },
            None,
            &diagnostics,
        )
        .unwrap(),
        2,
    );
    assert_eq!(
        diagnostics.warnings.iter().collect::<Vec<_>>(),
        ["first warning", DIAGNOSTIC_OMISSION],
    );
    assert_eq!(
        diagnostics.errors.iter().collect::<Vec<_>>(),
        [DIAGNOSTIC_OMISSION],
    );
}

/// If the second artifact exceeds its byte ceiling, neither staged destination is published.
#[test]
fn retained_output_oracle_overflow_keeps_both_destination_files() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let options = CheckOptions::default();
    let first = config.root.join("fingerprints.jsonl");
    let second = config.root.join("pairs.jsonl");
    std::fs::write(&first, "previous fingerprints").unwrap();

    for existing in [false, true] {
        if existing {
            std::fs::write(&second, "previous pairs").unwrap();
        }
        let fingerprints = EmitSink::open(&config, &options, &first, "fingerprint file", None)
            .unwrap()
            .fill(|out| Ok(out.write_all(b"staged fingerprints")?))
            .unwrap();
        let result = EmitSink::open(&config, &options, &second, "pair file", Some(&fingerprints))
            .unwrap()
            .fill(|out| {
                let mut bounded = crate::resource_limits::LimitedWriter {
                    inner: out,
                    remaining: 1,
                };
                bounded.write_all(b"line\n")?;
                Ok(())
            });
        assert!(
            matches!(result, Err(error) if format!("{error:#}").contains("byte limit exceeded"))
        );
        drop(fingerprints);
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "previous fingerprints"
        );
        assert_eq!(second.exists(), existing);
        if existing {
            assert_eq!(std::fs::read_to_string(&second).unwrap(), "previous pairs");
        }
    }
}

/// Unused retention marks reports incomplete only when the feature corpus permits unused analysis.
#[test]
fn retained_output_unused_limit_respects_feature_input_and_reports_incompleteness() {
    for has_feature in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("steps.ts"),
            "import { Given } from '@cucumber/cucumber'; Given('rain forecast', () => grabUmbrella()); Given('receipt sent', () => recordOrder());",
        )
        .unwrap();
        if has_feature {
            std::fs::write(root.join("empty.feature"), "Feature: Empty\n").unwrap();
        }
        let mut config = Config::load(root, ConfigOverrides::default()).unwrap();
        config
            .rules
            .extend(Rule::ALL.iter().map(|&rule| (rule, Severity::Off)));
        config
            .rules
            .insert(Rule::UnusedDefinition, Severity::Warning);
        config.fail_on_incomplete = true;
        config.reporters = vec![ReporterKind::Json, ReporterKind::Html];
        let options = CheckOptions {
            baseline: Some(PathBuf::from("baseline.json")),
            update_baseline: true,
            ..Default::default()
        };
        let discovery_budget = discovery::DiscoveryBudget::default();
        let files = discovery::discover_within(&config, &discovery_budget).unwrap();
        let mut diagnostics = discovery_diagnostics(&config, &files, &None);
        let mut feature_budget = gherkin::FeatureStepBudget::default();
        let extracted = extract_corpus(
            &config,
            &files,
            &None,
            &mut diagnostics,
            &mut feature_budget,
        );
        assert_eq!(extracted.definitions.len(), 2);
        let mut analyzed =
            analyze_corpus(&config, extracted, &mut diagnostics, false, (1, usize::MAX)).unwrap();
        assert_eq!(analyzed.result.findings.len(), usize::from(has_feature));
        assert_eq!(analyzed.run_incomplete, has_feature);
        assert!(!analyzed.corpus_incomplete);
        assert_eq!(analyzed.census.skipped_candidate_comparisons, 0);
        let outcome = apply_baseline_mode(
            &config,
            &options,
            &mut analyzed.result,
            analyzed.run_incomplete,
            &mut diagnostics,
            &mut feature_budget,
            &discovery_budget,
        )
        .unwrap();
        assert_eq!(outcome.is_some(), !has_feature);
        assert_eq!(config.root.join("baseline.json").is_file(), !has_feature);
        assert_eq!(
            determine_exit_code(&config, &options, &analyzed.result, outcome, &diagnostics)
                .unwrap(),
            if has_feature { 2 } else { 0 },
        );
        write_reports(
            &config,
            &files,
            &analyzed,
            &diagnostics,
            PhaseTimings {
                discovery_ms: 0.0,
                parsing_ms: 0.0,
                analysis_ms: 0.0,
            },
        )
        .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config.output_path("cuke-dedup.json")).unwrap())
                .unwrap();
        assert_eq!(json["analysis"]["truncated"], has_feature);
        assert_eq!(json["corpus"]["incomplete"], false);
        let html = std::fs::read_to_string(config.output_path("cuke-dedup.html")).unwrap();
        assert_eq!(
            html.contains("Analysis is incomplete: a safety limit"),
            has_feature
        );
    }
}
