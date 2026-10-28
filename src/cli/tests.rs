use super::*;
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
    let mut corpus = extract_corpus(&config, &files, &None, &mut diagnostics);
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
    let analyzed = analyze_corpus(&config, corpus, &mut diagnostics, false).unwrap();
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
        let mut diagnostics = Diagnostics {
            warnings: Vec::new(),
            errors: Vec::new(),
        };
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
                let mut diagnostics = Diagnostics {
                    warnings: vec![],
                    errors: vec![],
                };
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
