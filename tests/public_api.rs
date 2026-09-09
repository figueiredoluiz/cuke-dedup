use cuke_dedup::analysis::{analyze, analyze_with_diagnostics};
use cuke_dedup::config::{Config, ConfigOverrides, ReporterKind};
use cuke_dedup::discovery::{SourceFile, SourceLanguage};
use cuke_dedup::model::{Rule, Severity};
use cuke_dedup::reporters::ExecutionMetrics;
use cuke_dedup::typescript;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

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
    assert_eq!(outcome.operational_errors.len(), 1);
    assert!(outcome.operational_errors[0].contains("regex resource limit"));

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
fn completeness_reporting_preserves_existing_public_struct_construction() {
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

    let metrics = ExecutionMetrics {
        definition_files: 1,
        feature_files: 2,
        files_discovered: 3,
        discovery_ms: 1.0,
        parsing_ms: 2.0,
        analysis_ms: 3.0,
    };
    assert_eq!(metrics.files_discovered, 3);
}
