use super::*;
use std::fs;

#[test]
fn standalone_config_overrides_package_and_cli_overrides_both() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("package.json"),
        r#"{"cukeDedup":{"reporters":["html"],"threshold":10,"rules":{"duplicate-matcher":"warning"}}}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        r#"{"reporters":["json"],"threshold":20,"rules":{"duplicate-matcher":"off"}}"#,
    )
    .unwrap();
    let mut overrides = ConfigOverrides {
        reporters: Some(vec![ReporterKind::Terminal]),
        threshold: Some(30.0),
        ..ConfigOverrides::default()
    };
    overrides
        .rules
        .insert(Rule::DuplicateMatcher, Severity::Error);

    let config = Config::load(directory.path(), overrides).unwrap();
    assert_eq!(config.reporters, vec![ReporterKind::Terminal]);
    assert_eq!(config.threshold, 30.0);
    assert_eq!(config.severity(Rule::DuplicateMatcher), Severity::Error);
}

#[test]
fn threshold_defaults_to_zero_and_rejects_out_of_range_values() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(config.threshold, 0.0);

    for threshold in [-0.1, 100.1, f64::NAN, f64::INFINITY] {
        let error = Config::load(
            directory.path(),
            ConfigOverrides {
                threshold: Some(threshold),
                ..ConfigOverrides::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("finite percentage"));
    }
}

#[test]
fn configuration_rejects_empty_or_malformed_public_selectors() {
    assert!("unknown"
        .parse::<ReporterKind>()
        .unwrap_err()
        .contains("unknown reporter"));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(".cuke-dedup.json");
    for (source, expected) in [
        (r#"{"reporters":[]}"#, "at least one reporter"),
        (r#"{"parameterTypes":{"":"x"}}"#, "parameter type"),
        (r#"{"parameterTypes":{"bad{type}":"x"}}"#, "parameter type"),
        (
            r#"{"parameterTypes":{"colour":"("}}"#,
            "invalid regular expression",
        ),
        (r#"{"assertionModules":[""]}"#, "module specifier"),
        (r#"{"assertionModules":[" vitest"]}"#, "module specifier"),
        (r#"{"registrations":[""]}"#, "JavaScript identifier"),
        (r#"{"registrations":["1step"]}"#, "JavaScript identifier"),
        (
            r#"{"registrations":["step-name"]}"#,
            "JavaScript identifier",
        ),
        (
            r#"{"suppressions":[{"rule":"duplicate-handler","reason":"because"}]}"#,
            "select a path or matcher",
        ),
    ] {
        fs::write(&path, source).unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains(expected), "{source}: {error:#}");
    }

    fs::remove_file(path).unwrap();
    let ordinary_file = directory.path().join("not-a-directory");
    fs::write(&ordinary_file, "x").unwrap();
    assert!(Config::load(&ordinary_file, ConfigOverrides::default())
        .unwrap_err()
        .to_string()
        .contains("not a directory"));
}

#[test]
fn raw_configuration_applies_every_boolean_and_collection_field() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{
          "definitions":["steps/**/*.ts"],
          "features":["specs/**/*.feature"],
          "excludeDefaults":false,
          "includeHidden":true,
          "exclude":["generated/**"],
          "reporters":["html"],
          "output":"artifacts",
          "threshold":12,
          "requireFeatures":true,
          "requireDefinitions":true,
          "failOnIncomplete":true,
          "registrations":["step"],
          "assertionModules":["./support/fixtures"],
          "parameterTypes":{"colour":"red|green"},
          "noMetrics":true,
          "rules":{"unused-definition":"off"},
          "suppressions":[{"rule":"unused-definition","matcher":"legacy","reason":"migration"}]
        }"#,
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(config.definitions, ["steps/**/*.ts"]);
    assert_eq!(config.features, ["specs/**/*.feature"]);
    assert_eq!(config.exclude, ["generated/**"]);
    assert!(!config.exclude_defaults);
    assert!(config.include_hidden);
    assert_eq!(config.reporters, [ReporterKind::Html]);
    assert_eq!(
        config.output_path("report.html"),
        config.root.join("artifacts/report.html")
    );
    assert!(config.require_features && config.require_definitions && config.fail_on_incomplete);
    assert_eq!(config.registrations, ["step"]);
    assert_eq!(config.assertion_modules, ["./support/fixtures"]);
    assert_eq!(config.parameter_types["colour"], "red|green");
    assert!(config.no_metrics);
    assert_eq!(config.severity(Rule::UnusedDefinition), Severity::Off);
    assert_eq!(config.suppressions.len(), 1);
}

#[test]
fn no_metrics_is_available_from_project_config_and_cli_override() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"noMetrics":true}"#,
    )
    .unwrap();
    let configured = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert!(configured.no_metrics);

    let overridden = Config::load(
        directory.path(),
        ConfigOverrides {
            no_metrics: Some(false),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    assert!(!overridden.no_metrics);
}

#[test]
fn require_definitions_is_available_from_project_config() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"requireDefinitions":true}"#,
    )
    .unwrap();
    let configured = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert!(configured.require_definitions);
}

#[test]
fn candidate_limits_can_be_lowered_but_cannot_bypass_hard_safety_ceilings() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"maxCandidateComparisons":42,"maxStructuralClassComparisons":7}"#,
    )
    .unwrap();
    let configured = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(configured.max_candidate_comparisons, 42);
    assert_eq!(configured.max_structural_class_comparisons, 7);

    for (invalid, expected) in [
        (
            r#"{"maxCandidateComparisons":0}"#,
            "maxCandidateComparisons must be greater than zero",
        ),
        (
            r#"{"maxStructuralClassComparisons":0}"#,
            "maxStructuralClassComparisons must be greater than zero",
        ),
        (
            r#"{"maxCandidateComparisons":2000001}"#,
            "maxCandidateComparisons must not exceed the hard safety limit of 2000000",
        ),
        (
            r#"{"maxStructuralClassComparisons":250001}"#,
            "maxStructuralClassComparisons must not exceed the hard safety limit of 250000",
        ),
    ] {
        fs::write(directory.path().join(".cuke-dedup.json"), invalid).unwrap();
        assert!(Config::load(directory.path(), ConfigOverrides::default())
            .unwrap_err()
            .to_string()
            .contains(expected));
    }

    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"maxCandidateComparisons":2000000,"maxStructuralClassComparisons":250000}"#,
    )
    .unwrap();
    let exact_limits = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(exact_limits.max_candidate_comparisons, 2_000_000);
    assert_eq!(exact_limits.max_structural_class_comparisons, 250_000);
}

#[test]
fn project_configuration_output_cannot_escape_the_analysis_root() {
    let directory = tempfile::tempdir().unwrap();
    for output in ["../outside", "nested/../../outside"] {
        fs::write(
            directory.path().join(".cuke-dedup.json"),
            format!(r#"{{"output":"{output}"}}"#),
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("must stay beneath"), "{error:#}");
    }

    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"output":"/tmp/cuke-dedup-project-controlled"}"#,
    )
    .unwrap();
    let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
    assert!(error.to_string().contains("must stay beneath"), "{error:#}");
}

#[test]
fn command_line_output_safely_overrides_an_invalid_project_value() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"output":"../../project-controlled"}"#,
    )
    .unwrap();
    let operator_output = directory.path().join("../operator-controlled");
    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            output: Some(operator_output.clone()),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    assert_eq!(config.output, operator_output);
}

#[cfg(unix)]
#[test]
fn project_configuration_output_cannot_follow_a_symlink_outside_the_root() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), directory.path().join("reports-link")).unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"output":"reports-link"}"#,
    )
    .unwrap();

    let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
    assert!(error.to_string().contains("resolves outside"), "{error:#}");
}

#[test]
fn suppression_requires_a_reason() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        r#"{"suppressions":[{"rule":"duplicate-handler","path":"steps.ts","reason":""}]}"#,
    )
    .unwrap();
    let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
    assert!(error.to_string().contains("non-empty reason"));
}

#[test]
fn suppression_reasons_have_a_bounded_payload() {
    let directory = tempfile::tempdir().unwrap();
    let reason = "x".repeat(MAX_SUPPRESSION_REASON_CHARS + 1);
    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        format!(
            r#"{{"suppressions":[{{"rule":"duplicate-handler","path":"steps.ts","reason":"{reason}"}}]}}"#
        ),
    )
    .unwrap();
    let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
    assert!(error.to_string().contains("512-character limit"));
}

#[test]
fn configured_excludes_extend_safety_defaults() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        r#"{"exclude":["dist/**"]}"#,
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert!(config.exclude.contains(&"**/node_modules/**".to_owned()));
    assert!(config.exclude.contains(&"dist/**".to_owned()));
}

#[test]
fn exclude_layers_replace_custom_patterns_but_preserve_default_policy() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("package.json"),
        r#"{"cukeDedup":{"exclude":["package-only/**"]}}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        r#"{"exclude":["standalone-only/**"]}"#,
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert!(config.exclude.contains(&"**/node_modules/**".to_owned()));
    assert!(config.exclude.contains(&"standalone-only/**".to_owned()));
    assert!(!config.exclude.contains(&"package-only/**".to_owned()));

    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            exclude: Some(vec!["cli-only/**".to_owned()]),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    assert!(config.exclude.contains(&"cli-only/**".to_owned()));
    assert!(!config.exclude.contains(&"standalone-only/**".to_owned()));
}

#[test]
fn default_excludes_and_hidden_directories_have_explicit_escape_hatches() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        r#"{"excludeDefaults":false,"includeHidden":true}"#,
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert!(!config.exclude_defaults);
    assert!(config.exclude.is_empty());
    assert!(config.include_hidden);
}

#[test]
fn jsonl_is_configurable_but_cannot_be_mixed_with_terminal_output() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"reporters":["jsonl","json"]}"#,
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(
        config.reporters,
        vec![ReporterKind::Jsonl, ReporterKind::Json]
    );

    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"reporters":["terminal","jsonl"]}"#,
    )
    .unwrap();
    let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("terminal and jsonl reporters cannot share stdout"));
}

#[test]
fn invalid_suppression_glob_is_rejected_during_config_load() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        r#"{"suppressions":[{"rule":"duplicate-handler","path":"[","reason":"test"}]}"#,
    )
    .unwrap();
    let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
    assert!(error.to_string().contains("invalid suppression path glob"));
}

#[test]
fn cucumber_config_supplies_defaults_below_cuke_dedup_configuration() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("cucumber.json"),
        r#"{"default":{"paths":["acceptance"]}}"#,
    )
    .unwrap();
    let framework = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(
        framework.features,
        vec!["acceptance/**/*.{feature,feature.md}"]
    );
    assert!(framework.feature_pattern_origin.starts_with("Cucumber.js"));

    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        r#"{"features":["custom/**/*.spec"]}"#,
    )
    .unwrap();
    let own_config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(own_config.features, vec!["custom/**/*.spec"]);
    assert_eq!(own_config.feature_pattern_origin, "cuke-dedup.config.json");

    let cli = Config::load(
        directory.path(),
        ConfigOverrides {
            features: Some(vec!["cli/**/*.feature".to_owned()]),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    assert_eq!(cli.features, vec!["cli/**/*.feature"]);
    assert_eq!(cli.feature_pattern_origin, "--features");
}

#[test]
fn playwright_bdd_literal_features_are_detected_without_executing_config() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("playwright.config.ts"),
        "const testDir = defineBddConfig({ features: ['specs/**/*.spec'] });",
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(config.features, vec!["specs/**/*.spec"]);
    assert!(config.feature_pattern_origin.starts_with("Playwright-BDD"));
}

#[test]
fn cucumber_yaml_paths_are_detected() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("cucumber.yml"),
        "default:\n  paths:\n    - acceptance\n    - docs/**/*.feature.md\n",
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(
        config.features,
        vec![
            "acceptance/**/*.{feature,feature.md}",
            "docs/**/*.feature.md"
        ]
    );
}

#[test]
fn explicit_features_skip_invalid_lower_precedence_framework_config() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cucumber.json"), "not json").unwrap();
    fs::write(
        directory.path().join("cuke-dedup.config.json"),
        r#"{"features":["custom/**/*.spec"]}"#,
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(config.features, vec!["custom/**/*.spec"]);
    assert!(config.config_warnings.is_empty());
}

#[test]
fn jscpd_style_config_discovery_uses_one_source_in_documented_order() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join(".config")).unwrap();
    fs::write(
        directory.path().join("package.json"),
        r#"{"cukeDedup":{"threshold":10}}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join(".config/cuke-dedup.json"),
        r#"{"threshold":20}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"threshold":30}"#,
    )
    .unwrap();

    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(config.threshold, 30.0);
}

#[test]
fn explicit_config_is_fatal_and_bypasses_auto_discovery() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".cuke-dedup.json"),
        r#"{"threshold":10}"#,
    )
    .unwrap();
    let explicit = directory.path().join("team-policy.json");
    fs::write(&explicit, r#"{"threshold":40}"#).unwrap();
    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            config_file: Some(explicit),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    assert_eq!(config.threshold, 40.0);

    let relative = Config::load(
        directory.path(),
        ConfigOverrides {
            config_file: Some(PathBuf::from("team-policy.json")),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    assert_eq!(relative.threshold, 40.0);

    let error = Config::load(
        directory.path(),
        ConfigOverrides {
            config_file: Some(directory.path().join("missing.json")),
            ..ConfigOverrides::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("failed to read"));
}

#[test]
fn malformed_auto_config_warns_and_falls_through_to_package_config() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join(".cuke-dedup.json"), "not json").unwrap();
    fs::write(
        directory.path().join("package.json"),
        r#"{"cukeDedup":{"threshold":25}}"#,
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(config.threshold, 25.0);
    assert_eq!(config.config_warnings.len(), 1);
}
