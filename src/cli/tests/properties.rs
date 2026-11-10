use super::*;
use crate::tests::{feature_model, property_runner};
use proptest::prelude::*;

type Input = ([usize; 3], Vec<feature_model::FeatureCase>);

/// Replays independently modeled limits and concrete feature seeds through real loading.
fn check((limits, cases): &Input) -> proptest::test_runner::TestCaseResult {
    let root = tempfile::tempdir().unwrap();
    let config = Config::load(root.path(), ConfigOverrides::default()).unwrap();
    let paths: Vec<_> = cases
        .iter()
        .enumerate()
        .map(|(i, case)| {
            config.root.join(format!(
                "{i}日本.feature{}",
                if case.markdown { ".md" } else { "" }
            ))
        })
        .collect();
    for (case, path) in cases.iter().zip(&paths) {
        std::fs::write(path, case.source()).unwrap();
    }
    let files = discovery::discover(&config).unwrap();
    prop_assert_eq!(
        files.features.iter().map(|f| &f.path).collect::<Vec<_>>(),
        paths.iter().collect::<Vec<_>>()
    );
    let mut ledger = feature_model::Ledger::default();
    let mut expected = Vec::new();
    let mut parsed = 0;
    for (case, path) in cases.iter().zip(&paths) {
        if let Ok(steps) = ledger.admit(case, path, *limits) {
            parsed += 1;
            expected.extend(steps);
        }
    }
    let mut budget = gherkin::FeatureStepBudget::testing(*limits);
    let mut diagnostics = Diagnostics::default();
    let observed = extract_corpus(&config, &files, &None, &mut diagnostics, &mut budget);
    prop_assert_eq!(observed.feature_steps, expected);
    prop_assert_eq!(observed.parsed_feature_files, parsed);
    prop_assert_eq!(observed.incomplete, parsed != cases.len());
    prop_assert_eq!(diagnostics.errors.len(), cases.len() - parsed);
    // A fresh session admits every valid file even after this run refused one.
    let mut diagnostics = Diagnostics::default();
    let fresh = extract_corpus(
        &config,
        &files,
        &None,
        &mut diagnostics,
        &mut gherkin::FeatureStepBudget::default(),
    );
    let complete: Vec<_> = cases
        .iter()
        .zip(&paths)
        .filter(|(case, _)| case.mode != 3)
        .flat_map(|(case, path)| case.complete(path))
        .collect();
    prop_assert_eq!(fresh.feature_steps, complete);
    Ok(())
}

/// Exercises real discovery/file loading against a shared independently modeled extraction allowance.
#[test]
fn generated_cli_feature_sessions_share_limits_and_preserve_complete_files() {
    property_runner::generated(
        (
            proptest::array::uniform3(0usize..=512),
            proptest::collection::vec(feature_model::cases(), 1..=4),
        ),
        check,
    );
}
