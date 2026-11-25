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
