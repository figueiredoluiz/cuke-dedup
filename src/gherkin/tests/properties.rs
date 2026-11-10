use super::*;
use crate::tests::{feature_model, property_runner};
use proptest::prelude::*;

impl FeatureStepBudget {
    /// Supplies test-local limits to real extraction callers without process-wide overrides.
    pub(crate) fn testing(limits: [usize; 3]) -> Self {
        Self {
            steps: WorkBudget::new(limits[0]),
            bytes: WorkBudget::new(limits[1]),
            scan: WorkBudget::new(limits[2]),
        }
    }
}

type Input = ([usize; 3], Vec<feature_model::FeatureCase>, bool);

/// Checks a concrete case without generating inputs, including full records and retained state.
fn check((limits, cases, unicode): &Input) -> proptest::test_runner::TestCaseResult {
    let path = Path::new(if *unicode {
        "日本.feature"
    } else {
        "input.feature"
    });
    check_files(*limits, cases, path)
}

/// Checks concrete paths and limits without parsing expected text or source positions.
fn check_files(
    limits: [usize; 3],
    cases: &[feature_model::FeatureCase],
    path: &Path,
) -> proptest::test_runner::TestCaseResult {
    let mut actual = FeatureStepBudget::testing(limits);
    let mut model = feature_model::Ledger::default();
    for case in cases {
        let format = if case.markdown {
            FeatureFormat::GherkinMarkdown
        } else {
            FeatureFormat::Gherkin
        };
        let expected = model.admit(case, path, limits);
        let observed = extract_with_budget(&case.source(), path, format, &mut actual);
        prop_assert_eq!(observed.is_ok(), expected.is_ok());
        match (observed, expected) {
            (Ok(steps), Ok(wanted)) => prop_assert_eq!(steps, wanted),
            (Err(error), Err(reason)) => {
                let message = format!("{error:#}");
                prop_assert!(
                    message.contains(reason),
                    "expected {} in {}",
                    reason,
                    message
                );
            }
            _ => unreachable!(),
        }
        prop_assert_eq!(
            [
                actual.steps.spent(),
                actual.bytes.spent(),
                actual.scan.spent()
            ],
            model.spent
        );
        if case.mode != 3 {
            prop_assert_eq!(
                extract_with_format(&case.source(), path, format).unwrap(),
                case.complete(path)
            );
        }
    }
    Ok(())
}

/// Compares actual extraction decisions, full records and transaction state with authored visits.
#[test]
fn generated_feature_transactions_preserve_output_and_spent_work() {
    property_runner::generated(
        (
            proptest::array::uniform3(0usize..=256),
            proptest::collection::vec(feature_model::cases(), 1..=4),
            any::<bool>(),
        ),
        check,
    );
}

/// Pins exact/one-over decisions for UTF-8 keyword/text bytes and original native path bytes.
#[test]
fn feature_transaction_boundaries_preserve_unicode_and_native_payloads() {
    let paths = [std::path::PathBuf::from("日本.feature"), {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![b'a', 0xff]).into()
        }
        #[cfg(not(unix))]
        {
            std::path::PathBuf::from("input.feature")
        }
    }];
    for path in paths {
        for portuguese in [false, true] {
            for markdown in [false, true] {
                let case = feature_model::FeatureCase {
                    mode: 1,
                    rows: vec![2, 4],
                    background: true,
                    two_steps: true,
                    markdown,
                    portuguese,
                };
                let mut reference = feature_model::Ledger::default();
                assert_eq!(
                    reference
                        .admit(&case, &path, [usize::MAX; 3])
                        .unwrap()
                        .len(),
                    5
                );
                for axis in 0..3 {
                    for delta in [-1isize, 0, 1] {
                        let mut limits = reference.spent;
                        limits[axis] = limits[axis].checked_add_signed(delta).unwrap();
                        check_files(limits, std::slice::from_ref(&case), &path).unwrap();
                    }
                }
            }
        }
    }
}
