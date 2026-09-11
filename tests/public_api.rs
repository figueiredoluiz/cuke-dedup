use cuke_dedup::analysis::{analyze, analyze_with_diagnostics};
use cuke_dedup::config::{Config, ConfigOverrides, ReporterKind};
use cuke_dedup::discovery::{SourceFile, SourceLanguage};
use cuke_dedup::model::SourceLocation;
use cuke_dedup::model::{AnalysisResult, Rule, Severity};
use cuke_dedup::modes::{BaselineFile, BASELINE_SCHEMA_VERSION};
use cuke_dedup::reporters::{
    render_html, render_json, render_jsonl, render_sarif, write_terminal, ExecutionMetrics,
    ReportContext,
};
use cuke_dedup::source_adapter::{
    Extraction, ExtractionDiagnostic, ExtractionDiagnosticLevel, SourceAdapterRegistration,
    SOURCE_ADAPTER_REGISTRY,
};
use cuke_dedup::typescript;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn reporter_name(reporter: ReporterKind) -> &'static str {
    match reporter {
        ReporterKind::Terminal => "terminal",
        ReporterKind::Json => "json",
        ReporterKind::Jsonl => "jsonl",
        ReporterKind::Html => "html",
        ReporterKind::Sarif => "sarif",
        _ => "future reporter",
    }
}

#[test]
fn external_callers_can_preserve_partial_results_and_operational_diagnostics() {
    let definitions = typescript::extract(
        "Given(/a{1000000}/, () => work());",
        &SourceFile {
            path: PathBuf::from("steps.ts"),
            language: SourceLanguage::TypeScript,
        },
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();

    let outcome = analyze_with_diagnostics(definitions.clone(), Vec::new(), &config).unwrap();
    assert_eq!(outcome.result.definitions.len(), 1);
    assert_eq!(outcome.incomplete.len(), 1);
    assert!(outcome.incomplete[0].contains("regex resource limit"));

    let error = analyze(definitions, Vec::new(), &config).unwrap_err();
    assert!(error.to_string().contains("regex resource limit"));
}

#[test]
fn sessionless_file_extraction_uses_the_nearest_package_boundary() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("support.ts"),
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();
    let steps = directory.path().join("nested/deeper/steps.ts");
    fs::create_dir_all(steps.parent().unwrap()).unwrap();
    fs::write(
        &steps,
        "import { Given } from '../../support';\nGiven('nested public API step', () => work());\n",
    )
    .unwrap();

    let definitions = typescript::extract_file(&SourceFile {
        path: steps,
        language: SourceLanguage::TypeScript,
    })
    .unwrap();

    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].matcher, "nested public API step");
}

#[test]
fn extensible_public_outputs_use_stable_constructors() {
    let overrides = ConfigOverrides {
        config_file: None,
        definitions: None,
        features: None,
        exclude: None,
        exclude_defaults: None,
        include_hidden: None,
        reporters: Some(vec![ReporterKind::Json]),
        output: None,
        threshold: None,
        require_features: None,
        no_metrics: None,
        rules: BTreeMap::<Rule, Severity>::new(),
    };
    assert_eq!(overrides.reporters, Some(vec![ReporterKind::Json]));

    let metrics = ExecutionMetrics::new(1, 2, 1.0, 2.0, 3.0);
    assert_eq!(metrics.files_discovered, 3);

    let diagnostic = ExtractionDiagnostic::new(
        ExtractionDiagnosticLevel::Warning,
        SourceLocation::new("steps.ts", 1, 1, 1, 2),
        "dynamic matcher",
    );
    let extraction = Extraction::new(Vec::new(), vec![diagnostic]);
    assert_eq!(extraction.diagnostics.len(), 1);

    let baseline = BaselineFile::new(BTreeMap::from([("finding".to_owned(), 1)]));
    assert_eq!(baseline.fingerprints["finding"], 1);

    let default_baseline = BaselineFile::default();
    assert_eq!(default_baseline.schema_version, BASELINE_SCHEMA_VERSION);
    assert!(default_baseline.fingerprints.is_empty());

    let registration =
        SourceAdapterRegistration::new(".custom", SOURCE_ADAPTER_REGISTRY[0].adapter);
    assert_eq!(registration.suffix, ".custom");

    let all_rules: &[Rule] = Rule::ALL;
    let duplication_rules: &[Rule] = Rule::DUPLICATION_THRESHOLD;
    assert!(all_rules.len() > duplication_rules.len());

    assert_eq!(reporter_name(ReporterKind::Json), "json");
}

#[test]
fn report_context_is_the_single_public_rendering_entry_point() {
    let root = PathBuf::from("/repo");
    let result = AnalysisResult::new(Vec::new(), Vec::new(), Vec::new());
    let context = ReportContext::new(&result, &root, 5.0);

    assert!(render_json(&context)
        .unwrap()
        .contains("\"threshold\": 5.0"));
    assert!(render_jsonl(&context)
        .unwrap()
        .contains("\"type\":\"summary\""));
    assert!(render_html(&context).unwrap().contains("<!doctype html>"));
    assert!(render_sarif(&context)
        .unwrap()
        .contains("\"version\": \"2.1.0\""));

    let mut terminal = Vec::new();
    write_terminal(&context, &mut terminal).unwrap();
    assert!(String::from_utf8(terminal)
        .unwrap()
        .contains("Analyzed 0 definitions"));
}
