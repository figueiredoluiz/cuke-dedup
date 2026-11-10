use super::{PassBudget, WorkBudget};
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
