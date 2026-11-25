use super::*;
use crate::config::{CliConfigOverrides, ConfigOverrides};
use crate::source_adapter::SourceFile;
use crate::typescript::JAVASCRIPT;

/// Keeps an unobserved root inactive and refuses excess conventional features before publishing.
#[test]
fn mixed_selection_checks_observed_root_and_retained_path_limit() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load_for_cli(
        directory.path(),
        ConfigOverrides::default(),
        CliConfigOverrides::default(),
    )
    .unwrap();
    let root = config.root.join("features");
    std::fs::create_dir(&root).unwrap();
    let definitions = vec![SourceFile {
        path: config.root.join("steps.js"),
        language: JAVASCRIPT,
    }];
    let skipped_definitions = vec![SourceFile {
        path: root.join("steps.rb"),
        language: crate::ruby::RUBY,
    }];
    let mut files = DiscoveredFiles {
        definitions,
        skipped_definitions,
        ..Default::default()
    };
    let original = files.clone();
    let budget = DiscoveryBudget::default();
    let mut unseen = ConventionalCandidate::new(&config).unwrap();
    unseen.observe_directory(Path::new(""));
    assert!(!unseen.observe_feature(&root.join("steps.feature")));
    unseen.select(&config, &budget, &mut files).unwrap();
    assert_eq!(files, original);

    let mut observed = ConventionalCandidate::new(&config).unwrap();
    observed.observe_directory(&root);
    observed
        .additional_features
        .resize_with(MAX_DISCOVERY_SELECTED_FILES - 1, PathBuf::new);
    assert!(observed.observe_feature(&root.join("last.feature")));
    assert_eq!(
        observed.additional_features.len(),
        MAX_DISCOVERY_SELECTED_FILES
    );
    assert!(!observed.omitted_features);
    assert!(observed.observe_feature(&root.join("overflow.feature")));
    assert_eq!(
        observed.additional_features.len(),
        MAX_DISCOVERY_SELECTED_FILES
    );
    let error = observed
        .select(&config, &budget, &mut files)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("selected files in the conventional feature tree"),
        "{error}"
    );
    assert_eq!(files, original);
}

/// A runner case alias withdraws proof only where that platform's glob can match the alias.
#[test]
fn mixed_selection_case_aliases_are_unresolved() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (name, content) in [
        ("features/ruby.feature", "Feature: Ruby"),
        ("features/steps.rb", "Given('a step') { work }"),
        ("e2e/js.feature", "Feature: JS"),
        ("e2e/steps.js", "Given('a step', () => work());"),
        (
            "cucumber.json",
            r#"{"default":{"paths":["e2e/**/*.feature","FEATURES/**/*.feature"]}}"#,
        ),
    ] {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let config = Config::load_for_cli(
        root,
        ConfigOverrides::default(),
        CliConfigOverrides::default(),
    )
    .unwrap();
    for (check_case_aliases, expected) in [
        (false, [FeatureOwner::Framework, FeatureOwner::Conventional]),
        (true, [FeatureOwner::Unresolved, FeatureOwner::Unresolved]),
    ] {
        let files = super::super::discover_within_case_policy(
            &config,
            &DiscoveryBudget::default(),
            check_case_aliases,
        )
        .unwrap();
        assert!(files.mixed_suite.is_some());
        assert_eq!(
            files
                .features
                .iter()
                .map(|feature| feature.owner)
                .collect::<Vec<_>>(),
            expected,
            "check_case_aliases={check_case_aliases}"
        );
    }
}
