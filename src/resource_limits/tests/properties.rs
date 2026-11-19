use super::{omission_cut, PassBudget, WorkBudget};
use proptest::prelude::*;
use proptest::test_runner::TestCaseResult;

use crate::tests::property_runner::generated;
use std::fmt::Debug;

#[derive(Clone, Copy, Debug)]
enum WorkOp {
    Charge(usize),
    Admit(usize),
    Fits(usize),
}

#[derive(Clone, Copy, Debug)]
enum PassOp {
    Run(usize),
    Pass(usize),
    Begin,
    Limit(usize),
}

/// Models target-width saturation with wider mathematical arithmetic, without budget helpers.
fn capped_sum(spent: u128, amount: usize) -> u128 {
    (spent + amount as u128).min(usize::MAX as u128)
}

/// Compares every production decision and retained counter with independent admission transitions.
fn check_work((limit, ops): &(usize, Vec<WorkOp>)) -> TestCaseResult {
    let mut actual = WorkBudget::new(*limit);
    let mut spent = 0u128;
    prop_assert_eq!(actual.spent(), 0);
    for op in ops {
        let (amount, observed) = match op {
            WorkOp::Charge(n) => (*n, actual.charge(*n)),
            WorkOp::Admit(n) => (*n, actual.admit(*n)),
            WorkOp::Fits(n) => (*n, actual.fits(*n)),
        };
        let next = capped_sum(spent, amount);
        let fits = next <= *limit as u128;
        let expected = match op {
            WorkOp::Charge(_) => {
                spent = next;
                !fits
            }
            WorkOp::Admit(_) => {
                if fits {
                    spent = next;
                }
                fits
            }
            WorkOp::Fits(_) => fits,
        };
        prop_assert_eq!(observed, expected, "operation {:?}", op);
        prop_assert_eq!(actual.spent() as u128, spent, "operation {:?}", op);
        prop_assert_eq!(actual.limit(), *limit);
    }
    Ok(())
}

/// Models separate run/pass totals and a permanent exhaustion latch, including limit changes.
fn check_pass((initial_limit, ops): &(usize, Vec<PassOp>)) -> TestCaseResult {
    let mut actual = PassBudget::new(*initial_limit);
    let (mut limit, mut run, mut pass, mut exhausted) = (*initial_limit, 0u128, 0u128, false);
    prop_assert_eq!(actual.spent(), (0, 0));
    prop_assert!(!actual.exhausted());
    for op in ops {
        let observed = match op {
            PassOp::Run(n) => {
                run = capped_sum(run, *n);
                actual.charge_run(*n)
            }
            PassOp::Pass(n) => {
                pass = capped_sum(pass, *n);
                actual.charge_pass(*n)
            }
            PassOp::Begin => {
                pass = 0;
                actual.begin_pass()
            }
            PassOp::Limit(n) => {
                limit = *n;
                actual.set_limit(*n);
                actual.exhausted()
            }
        };
        exhausted = exhausted || run > limit as u128 || pass > limit as u128;
        prop_assert_eq!(observed, exhausted, "operation {:?}", op);
        prop_assert_eq!(actual.exhausted(), exhausted);
        let (actual_run, actual_pass) = actual.spent();
        prop_assert_eq!((actual_run as u128, actual_pass as u128), (run, pass));
    }
    Ok(())
}

/// Mixes ordinary small values with allocation-free target-width overflow neighbors.
fn amount() -> impl Strategy<Value = usize> {
    prop_oneof![8 => 0usize..=16, 1 => Just(usize::MAX - 1), 1 => Just(usize::MAX)]
}

/// Exercises admission, charging and observation in mixed sequences against the mathematical model.
#[test]
fn generated_work_budget_sequences_match_model() {
    let operation = prop_oneof![
        amount().prop_map(WorkOp::Charge),
        amount().prop_map(WorkOp::Admit),
        amount().prop_map(WorkOp::Fits),
    ];
    generated(
        (amount(), proptest::collection::vec(operation, 0..=32)),
        check_work,
    );
}

/// Exercises pass resets, limit changes and both counters against a permanent-latch model.
#[test]
fn generated_pass_budget_sequences_match_model() {
    let operation = prop_oneof![
        amount().prop_map(PassOp::Run),
        amount().prop_map(PassOp::Pass),
        Just(PassOp::Begin),
        amount().prop_map(PassOp::Limit),
    ];
    generated(
        (amount(), proptest::collection::vec(operation, 0..=32)),
        check_pass,
    );
}

/// Fully enumerates length-zero-to-two traces for limits 0..=2 over a declared operation alphabet.
fn enumerate<T: Copy + Debug>(
    ops: &[T],
    check: impl Fn(&(usize, Vec<T>)) -> TestCaseResult,
) -> usize {
    let mut cases = 0;
    for limit in 0..=2 {
        let traces = std::iter::once(vec![]).chain(ops.iter().flat_map(|first| {
            std::iter::once(vec![*first]).chain(ops.iter().map(|second| vec![*first, *second]))
        }));
        for trace in traces {
            let input = (limit, trace);
            let result = check(&input);
            assert!(result.is_ok(), "concrete input {input:?}: {result:?}");
            cases += 1;
        }
    }
    cases
}

/// Complements random search with every short trace in small admission and pass-state domains.
#[test]
fn small_budget_sequences_match_models_exhaustively() {
    let work: Vec<_> = (0..=2)
        .flat_map(|n| [WorkOp::Charge(n), WorkOp::Admit(n), WorkOp::Fits(n)])
        .collect();
    let pass: Vec<_> = (0..=2)
        .flat_map(|n| [PassOp::Run(n), PassOp::Pass(n), PassOp::Limit(n)])
        .chain([PassOp::Begin])
        .collect();
    assert_eq!(enumerate(&work, check_work), 273);
    assert_eq!(enumerate(&pass, check_pass), 333);
}

/// Independent oracle: the largest cut at or below `end` that no single valid character crosses.
fn oracle_cut(bytes: &[u8], end: usize) -> usize {
    let crosses = |cut: usize| {
        (cut.saturating_sub(3)..cut).any(|first| {
            (cut + 1..=bytes.len().min(first + 4)).any(|last| {
                std::str::from_utf8(&bytes[first..last]).is_ok_and(|text| text.chars().count() == 1)
            })
        })
    };
    (0..=end).rev().find(|&cut| !crosses(cut)).unwrap()
}

/// Cuts mixed valid and invalid UTF-8 bytes like the brute-force oracle and counts every byte past
/// the cut; inputs within the limit are never cut. Asserts each stratum was generated.
#[test]
fn generated_omission_cuts_match_the_character_oracle() {
    let piece = prop_oneof![
        any::<char>().prop_map(|value| value.to_string().into_bytes()),
        any::<u8>().prop_map(|byte| vec![byte]),
        (0x80u8..=0xBF).prop_map(|byte| vec![byte]),
    ];
    // Strata: within the limit, cut in place, cut moved back before a split character.
    let seen = std::cell::Cell::new([false; 3]);
    generated(
        (64usize..=72, proptest::collection::vec(piece, 16..=96)),
        |(limit, pieces)| {
            let bytes = pieces.concat();
            let cut = omission_cut(&bytes, bytes.len(), *limit);
            let mut strata = seen.get();
            if bytes.len() <= *limit {
                strata[0] = true;
                prop_assert_eq!(cut, None);
            } else {
                let end = oracle_cut(&bytes, limit - 64);
                strata[if end == limit - 64 { 1 } else { 2 }] = true;
                let marker = format!("… [{} bytes omitted]", bytes.len() - end);
                prop_assert_eq!(cut, Some((end, marker)));
            }
            seen.set(strata);
            Ok(())
        },
    );
    assert_eq!(seen.get(), [true; 3], "unreached stratum");
}
